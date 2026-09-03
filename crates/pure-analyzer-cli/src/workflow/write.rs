//! Staged replacement of analyzed source files.
//!
//! Each destination is replaced by one atomic path exchange: the exchange
//! and the fsync of its containing directory that makes it durable both
//! happen, or neither does. A software error caught partway through a
//! multi-file run rolls back every file already installed in that run. A
//! crash (`SIGKILL`, power loss) between two files' exchanges is not rolled
//! back or rolled forward — there is no cross-file journal — so it can leave
//! an earlier file replaced and a later one untouched; every file on disk is
//! still exactly its old or its new content, never a torn write. See
//! `docs/pure-analyzer.md`'s "Safe file updates" section for the guarantee
//! this delivers in full.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::Failure;

const MAX_UNIQUE_NAME_ATTEMPTS: usize = 128;
const WRITER_MARKER: &str = "pure-analyzer";
const STAGING_ROLE: &str = "stage";

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// One exact analyzed snapshot and its complete replacement text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Replacement {
    pub(super) path: PathBuf,
    pub(super) before: String,
    pub(super) after: String,
}

/// Replace every destination only after all replacement buffers have been staged safely.
pub(super) fn replace_all(replacements: Vec<Replacement>) -> Result<(), Failure> {
    let mut operations = NativeFileOperations;
    replace_all_with_operations(replacements, &mut operations)
}

fn replace_all_with_operations<O: FileOperations>(
    replacements: Vec<Replacement>,
    operations: &mut O,
) -> Result<(), Failure> {
    if replacements.is_empty() {
        return Ok(());
    }

    surface_orphaned_stage_artifacts(&replacements);

    let mut destinations = BTreeSet::new();
    let mut staged = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        if let Err(error) = ensure_regular_file(&replacement.path) {
            return Err(clean_staged_replacements(error, &mut staged));
        }
        let destination = match canonical_destination(&replacement.path) {
            Ok(destination) => destination,
            Err(error) => return Err(clean_staged_replacements(error, &mut staged)),
        };
        if !destinations.insert(destination) {
            return Err(clean_staged_replacements(
                Failure::internal(format!(
                    "output path {} was selected more than once",
                    replacement.path.display()
                )),
                &mut staged,
            ));
        }
        match StagedReplacement::stage(replacement) {
            Ok(replacement) => staged.push(replacement),
            Err(error) => return Err(clean_staged_replacements(error, &mut staged)),
        }
    }

    commit(&mut staged, operations)
}

/// Warn about any leftover stage artifact beside a destination this run is
/// about to touch.
///
/// A crash between installing a staged file and removing its now-stale
/// backup (or between staging a file and installing it) leaves a
/// `.<name>.pure-analyzer-stage-<pid>-<n>` file with no in-memory record of
/// which run created it. Deleting it automatically would risk destroying a
/// concurrently running invocation's own live staging file — a reused PID or
/// a second legitimate `pure-analyzer` process touching the same tree — so
/// this only reports it; an operator confirms no such process is running
/// before removing it by hand.
fn surface_orphaned_stage_artifacts(replacements: &[Replacement]) {
    for path in orphaned_stage_artifacts(replacements) {
        tracing::warn!(
            path = %path.display(),
            "leftover pure-analyzer stage artifact from an interrupted run; remove it once no \
             pure-analyzer process is using it"
        );
    }
}

/// Every stage-artifact-named entry beside a destination this run touches.
fn orphaned_stage_artifacts(replacements: &[Replacement]) -> Vec<PathBuf> {
    let mut scanned = BTreeSet::new();
    let mut found = Vec::new();
    for replacement in replacements {
        let parent = parent_or_cwd(&replacement.path);
        if !scanned.insert(parent.to_path_buf()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            if is_stage_artifact(&entry.file_name()) {
                found.push(entry.path());
            }
        }
    }
    found
}

/// Whether a file name matches this writer's own staging-artifact pattern
/// (`unique_sibling` with [`STAGING_ROLE`]), the single naming convention
/// production code ever creates on disk.
fn is_stage_artifact(name: &OsStr) -> bool {
    name.to_string_lossy().contains(&stage_artifact_marker())
}

fn stage_artifact_marker() -> String {
    format!("{WRITER_MARKER}-{STAGING_ROLE}-")
}

fn canonical_destination(path: &Path) -> Result<PathBuf, Failure> {
    path.canonicalize().map_err(|error| {
        Failure::internal(format!(
            "could not resolve output destination {}: {error}",
            path.display()
        ))
    })
}

fn commit<O: FileOperations>(
    staged: &mut [StagedReplacement],
    operations: &mut O,
) -> Result<(), Failure> {
    let mut committed = Vec::with_capacity(staged.len());
    for index in 0..staged.len() {
        match staged[index].switch(index, operations) {
            Ok(applied) => committed.push(applied),
            Err(error) => return fail_commit(error, &committed, staged, operations),
        }
    }
    clean_transaction_artifacts(&committed)
}

fn fail_commit<O: FileOperations>(
    failure: Failure,
    committed: &[CommittedReplacement],
    staged: &mut [StagedReplacement],
    operations: &mut O,
) -> Result<(), Failure> {
    let failure = match rollback(committed, operations) {
        Ok(()) => failure,
        Err(rollback_failure) => {
            Failure::internal(format!("{failure}; additionally, {rollback_failure}"))
        }
    };
    Err(clean_staged_replacements(failure, staged))
}

/// Explicitly remove every unconsumed staged file and retain cleanup failures.
///
/// This runs on every handled transaction failure rather than relying on
/// [`Drop`], which cannot report an artifact that it was unable to remove.
fn clean_staged_replacements(failure: Failure, staged: &mut [StagedReplacement]) -> Failure {
    let mut cleanup_failures = Vec::new();
    for replacement in staged {
        if let Err(error) = replacement.cleanup() {
            cleanup_failures.push(error.to_string());
        }
    }
    if cleanup_failures.is_empty() {
        failure
    } else {
        Failure::internal(format!(
            "{failure}; additionally, could not remove staged outputs: {}",
            cleanup_failures.join("; ")
        ))
    }
}

fn clean_transaction_artifacts(committed: &[CommittedReplacement]) -> Result<(), Failure> {
    let mut failures = Vec::new();
    for replacement in committed {
        if let Err(error) = remove_owned_file(&replacement.backup) {
            failures.push(format!("{}: {error}", replacement.backup.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Failure::internal(format!(
            "all replacements were installed, but could not remove transactional artifacts: {}",
            failures.join("; ")
        )))
    }
}

fn rollback<O: FileOperations>(
    committed: &[CommittedReplacement],
    operations: &mut O,
) -> Result<(), Failure> {
    let mut failures = Vec::new();
    for replacement in committed.iter().rev() {
        if let Err(error) = rollback_one(replacement, operations) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Failure::internal(format!(
            "could not roll back every replacement: {}",
            failures.join("; ")
        )))
    }
}

fn rollback_one<O: FileOperations>(
    replacement: &CommittedReplacement,
    operations: &mut O,
) -> Result<(), Failure> {
    operations.before_rollback(replacement.index, &replacement.path)?;
    if let Err(error) = verify_installed_snapshot(&replacement.path, &replacement.after) {
        return Err(clean_backup_after_failure(error, &replacement.backup));
    }
    operations.before_rollback_exchange(replacement.index, &replacement.path)?;
    operations
        .exchange(
            ExchangeOperation::RestoreRolledBack,
            replacement.index,
            &replacement.backup,
            &replacement.path,
        )
        .map_err(|error| {
            Failure::internal(format!(
                "could not atomically restore original output {} while rolling back: {error}",
                replacement.path.display()
            ))
        })?;
    let displaced = verify_rollback_displaced_snapshot(
        &replacement.backup,
        &replacement.after,
        &replacement.path,
    );
    let restored = verify_rolled_back_snapshot(&replacement.path, &replacement.before);
    match (displaced, restored) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(error)) => {
            return Err(restore_rollback_after_mismatch(
                error,
                replacement,
                operations,
                Some(replacement.after.clone()),
            ));
        }
        (Err(error), _) => {
            return Err(restore_rollback_after_mismatch(
                error,
                replacement,
                operations,
                None,
            ));
        }
    }
    remove_owned_file(&replacement.backup).map_err(|error| {
        Failure::internal(format!(
            "could not remove transactional backup {} after rolling back: {error}",
            replacement.backup.display()
        ))
    })
}

#[derive(Debug)]
struct StagedReplacement {
    replacement: Replacement,
    temporary: Option<PathBuf>,
}

impl StagedReplacement {
    fn stage(replacement: Replacement) -> Result<Self, Failure> {
        let permissions = verify_snapshot(&replacement.path, &replacement.before)?;
        let (temporary, mut file) = create_staging_file(&replacement.path)?;
        let write_result = write_staged(&mut file, &replacement.after, permissions, &temporary);
        drop(file);
        if let Err(error) = write_result {
            return Err(clean_staged_after_failure(error, &temporary));
        }

        Ok(Self {
            replacement,
            temporary: Some(temporary),
        })
    }

    fn switch<O: FileOperations>(
        &mut self,
        index: usize,
        operations: &mut O,
    ) -> Result<CommittedReplacement, Failure> {
        operations.before_late_validation(index, &self.replacement.path)?;
        verify_snapshot(&self.replacement.path, &self.replacement.before)?;
        let temporary = self.temporary_path()?.to_owned();
        operations.before_install(index, &self.replacement.path, &temporary)?;

        operations
            .exchange(
                ExchangeOperation::InstallStaged,
                index,
                &temporary,
                &self.replacement.path,
            )
            .map_err(|error| {
                Failure::internal(format!(
                    "could not atomically install staged output {}: {error}",
                    self.replacement.path.display()
                ))
            })?;
        let displaced =
            verify_displaced_snapshot(&temporary, &self.replacement.before, &self.replacement.path);
        let installed = verify_installed_snapshot(&self.replacement.path, &self.replacement.after);
        match (displaced, installed) {
            (Ok(()), Ok(())) => {}
            (Ok(()), Err(error)) => {
                return Err(self.restore_invalid_install(
                    error,
                    index,
                    operations,
                    &temporary,
                    Some(self.replacement.before.clone()),
                ));
            }
            (Err(error), _) => {
                return Err(
                    self.restore_invalid_install(error, index, operations, &temporary, None)
                );
            }
        }

        self.temporary = None;
        Ok(CommittedReplacement {
            index,
            path: self.replacement.path.clone(),
            backup: temporary,
            before: self.replacement.before.clone(),
            after: self.replacement.after.clone(),
        })
    }

    /// Return an invalid-install error only after atomically putting the
    /// displaced entry back at its original path.
    ///
    /// The exchange leaves the staged path holding exactly the entry that was
    /// at the destination at the commit point. Either the displaced snapshot
    /// or the installed output can fail verification: the latter also catches
    /// tampering with the closed staging pathname before the exchange.
    fn restore_invalid_install<O: FileOperations>(
        &mut self,
        failure: Failure,
        index: usize,
        operations: &mut O,
        temporary: &Path,
        expected_destination: Option<String>,
    ) -> Failure {
        if let Err(error) = operations.exchange(
            ExchangeOperation::RestoreStale,
            index,
            temporary,
            &self.replacement.path,
        ) {
            // The temporary path now contains the external edit. It is the
            // only recoverable copy, so deliberately relinquish cleanup.
            self.temporary = None;
            return Failure::internal(format!(
                "{failure}; additionally, could not restore the external edit to {}: {error}; recovery content remains at {}",
                self.replacement.path.display(),
                temporary.display()
            ));
        }

        if let Err(error) = verify_snapshot(temporary, &self.replacement.after) {
            // A second writer raced the recovery exchange. Its content may
            // now be reachable only through the staging path, so retain it.
            self.temporary = None;
            return Failure::internal(format!(
                "{failure}; additionally, recovery of {} raced another edit: {error}; recovery content remains at {}",
                self.replacement.path.display(),
                temporary.display()
            ));
        }
        if let Some(expected) = expected_destination
            && let Err(error) = verify_snapshot(&self.replacement.path, &expected)
        {
            self.temporary = None;
            return Failure::internal(format!(
                "{failure}; additionally, recovery of {} raced another edit: {error}; recovery content remains at {}",
                self.replacement.path.display(),
                temporary.display()
            ));
        }

        failure
    }

    fn temporary_path(&self) -> Result<&Path, Failure> {
        self.temporary.as_deref().ok_or_else(|| {
            Failure::internal(format!(
                "staged output for {} was already consumed",
                self.replacement.path.display()
            ))
        })
    }

    /// Remove the still-owned staging file, reporting a failed cleanup.
    fn cleanup(&mut self) -> Result<(), Failure> {
        let Some(temporary) = self.temporary.take() else {
            return Ok(());
        };
        remove_owned_file(&temporary).map_err(|error| {
            Failure::internal(format!(
                "could not remove staged output {}: {error}",
                temporary.display()
            ))
        })
    }
}

impl Drop for StagedReplacement {
    fn drop(&mut self) {
        if let Some(temporary) = &self.temporary {
            let _ = remove_owned_file(temporary);
        }
    }
}

#[derive(Debug)]
struct CommittedReplacement {
    index: usize,
    path: PathBuf,
    backup: PathBuf,
    before: String,
    after: String,
}

trait FileOperations {
    fn before_late_validation(&mut self, _index: usize, _path: &Path) -> Result<(), Failure> {
        Ok(())
    }

    fn before_install(
        &mut self,
        _index: usize,
        _path: &Path,
        _temporary: &Path,
    ) -> Result<(), Failure> {
        Ok(())
    }

    fn before_rollback(&mut self, _index: usize, _path: &Path) -> Result<(), Failure> {
        Ok(())
    }

    fn before_rollback_exchange(&mut self, _index: usize, _path: &Path) -> Result<(), Failure> {
        Ok(())
    }

    fn exchange(
        &mut self,
        _operation: ExchangeOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()>;
}

struct NativeFileOperations;

impl FileOperations for NativeFileOperations {
    fn exchange(
        &mut self,
        _operation: ExchangeOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()> {
        atomic_exchange(source, destination)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExchangeOperation {
    InstallStaged,
    RestoreStale,
    RestoreRolledBack,
}

/// Atomically swap two existing sibling paths, preserving the displaced entry
/// for stale-snapshot verification and rollback, and fsync the containing
/// directory so the swap itself — not just the file content already fsynced
/// while staging — survives a crash immediately after this call returns.
///
/// A rename is only durable once its directory entry is: POSIX does not
/// guarantee a completed `rename`/`renameat2` survives a crash until the
/// directory holding it has itself been fsynced. Without this, a crash right
/// after a successful exchange could still lose it on reboot even though
/// `write_staged` had already fsynced the staged file's contents.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn atomic_exchange(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, source, CWD, destination, RenameFlags::EXCHANGE)
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    fsync_parent_directory(destination)
}

/// Fsync `path`'s containing directory. Exchanged paths are siblings, so
/// syncing either one's parent covers the directory entries of both.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn fsync_parent_directory(path: &Path) -> io::Result<()> {
    let directory = File::open(parent_or_cwd(path))?;
    rustix::fs::fsync(&directory)
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))
}

/// A plain rename cannot preserve a non-cooperating writer that races the
/// final validation, so unsupported platforms fail closed instead of silently
/// weakening the transactional-write contract.
#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn atomic_exchange(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic file exchange is unavailable on this platform",
    ))
}

fn ensure_regular_file(path: &Path) -> Result<(), Failure> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Failure::internal(format!(
            "could not inspect output {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(Failure::usage(format!(
            "refusing to replace symlink {}",
            path.display()
        )));
    }
    if !metadata.file_type().is_file() {
        return Err(Failure::usage(format!(
            "refusing to replace non-regular file {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_snapshot(path: &Path, expected: &str) -> Result<Permissions, Failure> {
    ensure_regular_file(path)?;
    let actual = fs::read(path).map_err(|error| {
        Failure::internal(format!(
            "could not re-read output {}: {error}",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Failure::internal(format!(
            "could not re-inspect output {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(Failure::usage(format!(
            "refusing to replace symlink {}",
            path.display()
        )));
    }
    if !metadata.file_type().is_file() {
        return Err(Failure::usage(format!(
            "refusing to replace non-regular file {}",
            path.display()
        )));
    }
    if actual != expected.as_bytes() {
        return Err(Failure::usage(format!(
            "{} changed after it was analyzed; no replacement was installed",
            path.display()
        )));
    }
    Ok(metadata.permissions())
}

fn verify_installed_snapshot(path: &Path, expected: &str) -> Result<(), Failure> {
    ensure_regular_file(path)?;
    let actual = fs::read(path).map_err(|error| {
        Failure::internal(format!(
            "could not inspect replacement output {} before rollback: {error}",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Failure::internal(format!(
            "could not re-inspect replacement output {} before rollback: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Failure::usage(format!(
            "{} changed after automatic replacement; preserving external edit",
            path.display()
        )));
    }
    if actual != expected.as_bytes() {
        return Err(Failure::usage(format!(
            "{} changed after automatic replacement; preserving external edit",
            path.display()
        )));
    }
    Ok(())
}

/// Verify the entry displaced by an installation exchange. A mismatch is a
/// stale destination, not a corrupt staging file: the caller will swap the
/// displaced external edit back into place before returning this error.
fn verify_displaced_snapshot(
    displaced: &Path,
    expected: &str,
    destination: &Path,
) -> Result<(), Failure> {
    match verify_snapshot(displaced, expected) {
        Ok(_) => Ok(()),
        Err(error) if error.exit_code() == super::EXIT_USAGE => Err(Failure::usage(format!(
            "{} changed after it was analyzed; no replacement was installed",
            destination.display()
        ))),
        Err(error) => Err(error),
    }
}

/// Verify the entry displaced while undoing a prior installation. A mismatch
/// means another writer won before the rollback exchange, so its edit must be
/// restored rather than overwritten by the rollback.
fn verify_rollback_displaced_snapshot(
    displaced: &Path,
    expected: &str,
    destination: &Path,
) -> Result<(), Failure> {
    match verify_snapshot(displaced, expected) {
        Ok(_) => Ok(()),
        Err(error) if error.exit_code() == super::EXIT_USAGE => Err(Failure::usage(format!(
            "{} changed after automatic replacement; preserving external edit",
            destination.display()
        ))),
        Err(error) => Err(error),
    }
}

/// Verify that the rollback exchange restored the analyzed input at its
/// destination. This catches a tampered backup path before it is allowed to
/// replace the formatter output.
fn verify_rolled_back_snapshot(path: &Path, expected: &str) -> Result<(), Failure> {
    match verify_snapshot(path, expected) {
        Ok(_) => Ok(()),
        Err(error) if error.exit_code() == super::EXIT_USAGE => Err(Failure::usage(format!(
            "{} changed while rolling back; preserving external edit",
            path.display()
        ))),
        Err(error) => Err(error),
    }
}

/// Undo a rollback exchange that displaced an external edit. If the second
/// exchange races too, retain the staging path so no external content is
/// discarded while reporting a recoverable artifact location.
fn restore_rollback_after_mismatch<O: FileOperations>(
    failure: Failure,
    replacement: &CommittedReplacement,
    operations: &mut O,
    expected_destination: Option<String>,
) -> Failure {
    if let Err(error) = operations.exchange(
        ExchangeOperation::RestoreStale,
        replacement.index,
        &replacement.backup,
        &replacement.path,
    ) {
        return Failure::internal(format!(
            "{failure}; additionally, could not restore the external edit to {}: {error}; recovery content remains at {}",
            replacement.path.display(),
            replacement.backup.display()
        ));
    }

    if let Err(error) = verify_snapshot(&replacement.backup, &replacement.before) {
        return Failure::internal(format!(
            "{failure}; additionally, recovery of {} raced another edit: {error}; recovery content remains at {}",
            replacement.path.display(),
            replacement.backup.display()
        ));
    }
    if let Some(expected) = expected_destination
        && let Err(error) = verify_installed_snapshot(&replacement.path, &expected)
    {
        return Failure::internal(format!(
            "{failure}; additionally, recovery of {} raced another edit: {error}; recovery content remains at {}",
            replacement.path.display(),
            replacement.backup.display()
        ));
    }

    clean_backup_after_failure(failure, &replacement.backup)
}

fn create_staging_file(path: &Path) -> Result<(PathBuf, File), Failure> {
    for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
        let temporary = unique_sibling(path, STAGING_ROLE)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(Failure::internal(format!(
                    "could not create staged output {}: {error}",
                    temporary.display()
                )));
            }
        }
    }
    Err(Failure::internal(format!(
        "could not reserve a staged output path beside {}",
        path.display()
    )))
}

/// A path's parent directory, or `.` for a bare relative file name.
fn parent_or_cwd(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn unique_sibling(path: &Path, role: &str) -> Result<PathBuf, Failure> {
    let parent = parent_or_cwd(path);
    let file_name = path.file_name().ok_or_else(|| {
        Failure::internal(format!("output path has no file name: {}", path.display()))
    })?;
    let nonce = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(file_name);
    name.push(OsStr::new("."));
    name.push(WRITER_MARKER);
    name.push(OsStr::new("-"));
    name.push(role);
    name.push(OsStr::new("-"));
    name.push(std::process::id().to_string());
    name.push(OsStr::new("-"));
    name.push(nonce.to_string());
    Ok(parent.join(name))
}

fn write_staged(
    file: &mut File,
    text: &str,
    permissions: Permissions,
    path: &Path,
) -> Result<(), Failure> {
    file.write_all(text.as_bytes()).map_err(|error| {
        Failure::internal(format!(
            "could not write staged output {}: {error}",
            path.display()
        ))
    })?;
    file.set_permissions(permissions).map_err(|error| {
        Failure::internal(format!(
            "could not preserve permissions on staged output {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        Failure::internal(format!(
            "could not sync staged output {}: {error}",
            path.display()
        ))
    })
}

fn clean_staged_after_failure(error: Failure, temporary: &Path) -> Failure {
    match remove_owned_file(temporary) {
        Ok(()) => error,
        Err(cleanup_error) => Failure::internal(format!(
            "{error}; additionally, could not remove staged output {}: {cleanup_error}",
            temporary.display()
        )),
    }
}

fn clean_backup_after_failure(error: Failure, backup: &Path) -> Failure {
    match remove_owned_file(backup) {
        Ok(()) => error,
        Err(cleanup_error) => Failure::internal(format!(
            "{error}; additionally, could not remove transactional backup {}: {cleanup_error}",
            backup.display()
        )),
    }
}

fn remove_owned_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const TEST_DIRECTORY_ATTEMPTS: usize = 128;
    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn replaces_every_file_after_staging() {
        let fixture = Fixture::new("success");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");

        replace_all(vec![
            replacement(&first, "before one", "after one"),
            replacement(&second, "before two", "after two"),
        ])
        .expect("replace staged fixtures");

        assert_eq!(fixture.read(&first), "after one");
        assert_eq!(fixture.read(&second), "after two");
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    #[test]
    fn native_writes_fail_closed_without_atomic_exchange() {
        let fixture = Fixture::new("unsupported-exchange");
        let source = fixture.write("source.pure", "before");

        let error = replace_all(vec![replacement(&source, "before", "after")])
            .expect_err("unsupported platforms must not fall back to plain rename");

        assert_eq!(error.exit_code(), super::super::EXIT_INTERNAL);
        assert_eq!(fixture.read(&source), "before");
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn late_stale_destination_rolls_back_prior_replacements_and_cleans_staging() {
        let fixture = Fixture::new("late-stale");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");
        let changed_elsewhere = "changed elsewhere";
        let mut operations = TestOperations::late_stale(1, second.clone(), changed_elsewhere);

        let error = replace_all_with_operations(
            vec![
                replacement(&first, "before one", "after one"),
                replacement(&second, "before two", "after two"),
            ],
            &mut operations,
        )
        .expect_err("late stale source must fail");

        assert_eq!(error.exit_code(), super::super::EXIT_USAGE);
        assert_eq!(fixture.read(&first), "before one");
        assert_eq!(fixture.read(&second), changed_elsewhere);
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn exchange_restores_an_edit_that_races_the_final_validation() {
        let fixture = Fixture::new("install-race");
        let source = fixture.write("source.pure", "before");
        let external_change = "external edit";
        let mut operations = TestOperations::install_change(0, source.clone(), external_change);

        let error = replace_all_with_operations(
            vec![replacement(&source, "before", "after")],
            &mut operations,
        )
        .expect_err("an edit immediately before the exchange must remain stale");

        assert_eq!(error.exit_code(), super::super::EXIT_USAGE);
        assert_eq!(fixture.read(&source), external_change);
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn exchange_rejects_tampered_staged_content() {
        let fixture = Fixture::new("staging-tamper");
        let source = fixture.write("source.pure", "before");
        let tampered = "tampered staged content";
        let mut operations = TestOperations::tamper_staging(0, tampered);

        replace_all_with_operations(
            vec![replacement(&source, "before", "after")],
            &mut operations,
        )
        .expect_err("a staged file changed before exchange must be rejected");

        assert_eq!(fixture.read(&source), "before");
        let artifacts: Vec<_> = fs::read_dir(&fixture.root)
            .expect("list fixture entries")
            .map(|entry| entry.expect("read fixture entry").path())
            .filter(|path| path.file_name().is_some_and(is_stage_artifact))
            .collect();
        assert_eq!(artifacts.len(), 1, "retain the unknown staged artifact");
        assert_eq!(
            fs::read(&artifacts[0]).expect("read staged artifact"),
            tampered.as_bytes()
        );
    }

    #[test]
    fn later_switch_failure_rolls_back_prior_replacements_and_cleans_staging() {
        let fixture = Fixture::new("switch-failure");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");
        let mut operations = TestOperations::failing_install(1);

        let error = replace_all_with_operations(
            vec![
                replacement(&first, "before one", "after one"),
                replacement(&second, "before two", "after two"),
            ],
            &mut operations,
        )
        .expect_err("second installation must fail");

        assert_eq!(error.exit_code(), super::super::EXIT_INTERNAL);
        assert_eq!(fixture.read(&first), "before one");
        assert_eq!(fixture.read(&second), "before two");
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn rollback_preserves_external_edits_after_later_switch_failure() {
        let fixture = Fixture::new("rollback-external-edit");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");
        let external_change = "external change";
        let mut operations = TestOperations::failing_install_with_rollback_change(
            1,
            0,
            first.clone(),
            external_change,
        );

        let error = replace_all_with_operations(
            vec![
                replacement(&first, "before one", "after one"),
                replacement(&second, "before two", "after two"),
            ],
            &mut operations,
        )
        .expect_err("rollback must preserve an external edit");

        assert_eq!(error.exit_code(), super::super::EXIT_INTERNAL);
        assert_eq!(fixture.read(&first), external_change);
        assert_eq!(fixture.read(&second), "before two");
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn rollback_restores_an_edit_that_races_its_final_validation() {
        let fixture = Fixture::new("rollback-exchange-race");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");
        let external_change = "external change";
        let mut operations = TestOperations::failing_install_with_rollback_exchange_change(
            1,
            0,
            first.clone(),
            external_change,
        );

        let error = replace_all_with_operations(
            vec![
                replacement(&first, "before one", "after one"),
                replacement(&second, "before two", "after two"),
            ],
            &mut operations,
        )
        .expect_err("the second installation must force a rollback");

        assert_eq!(error.exit_code(), super::super::EXIT_INTERNAL);
        assert_eq!(fixture.read(&first), external_change);
        assert_eq!(fixture.read(&second), "before two");
        fixture.assert_no_writer_artifacts();
    }

    #[test]
    fn refuses_non_regular_destinations_before_staging_any_output() {
        let fixture = Fixture::new("non-regular");
        let file = fixture.write("file.pure", "before file");
        let directory = fixture.root.join("directory.pure");
        fs::create_dir(&directory).expect("create directory fixture");

        let error = replace_all(vec![
            replacement(&file, "before file", "after file"),
            replacement(&directory, "before directory", "after directory"),
        ])
        .expect_err("directories cannot be replaced");

        assert_eq!(error.exit_code(), super::super::EXIT_USAGE);
        assert_eq!(fixture.read(&file), "before file");
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_destinations() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new("symlink");
        let target = fixture.write("target.pure", "before target");
        let link = fixture.root.join("link.pure");
        symlink(&target, &link).expect("create symlink fixture");

        let error = replace_all(vec![replacement(&link, "before target", "after target")])
            .expect_err("symlinks cannot be replaced");

        assert_eq!(error.exit_code(), super::super::EXIT_USAGE);
        assert_eq!(fixture.read(&target), "before target");
        fixture.assert_no_writer_artifacts();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn preserves_destination_permissions() {
        use std::os::unix::fs::PermissionsExt;

        const FIXTURE_MODE: u32 = 0o640;
        const PERMISSION_BITS: u32 = 0o777;

        let fixture = Fixture::new("permissions");
        let path = fixture.write("permissions.pure", "before");
        fs::set_permissions(&path, fs::Permissions::from_mode(FIXTURE_MODE))
            .expect("set fixture permissions");

        replace_all(vec![replacement(&path, "before", "after")])
            .expect("replace permission fixture");

        let mode = fs::metadata(&path)
            .expect("read replacement metadata")
            .permissions()
            .mode()
            & PERMISSION_BITS;
        assert_eq!(mode, FIXTURE_MODE);
    }

    #[test]
    fn orphaned_stage_artifacts_finds_only_this_writers_own_naming_pattern() {
        let fixture = Fixture::new("orphan-scan");
        let target = fixture.write("target.pure", "before");
        let leftover = fixture.write(
            &format!(".target.pure.{WRITER_MARKER}-{STAGING_ROLE}-99999-7"),
            "leftover after content",
        );
        fixture.write(
            &format!(".target.pure.{WRITER_MARKER}-other-99999-7"),
            "a different role must not match",
        );
        fixture.write(".unrelated-hidden-file", "not ours");

        let found = orphaned_stage_artifacts(&[replacement(&target, "before", "after")]);

        assert_eq!(found, vec![leftover]);
    }

    /// A process killed between two files' exchanges (`SIGKILL`, power loss)
    /// never runs a destructor, so `std::mem::forget` on the not-yet-switched
    /// [`StagedReplacement`] is the faithful way to reproduce that on disk
    /// inside a test: it skips exactly the `Drop` cleanup a real crash would
    /// also skip, leaving precisely the artifacts a crash leaves.
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn crash_between_exchanges_leaves_partial_application_that_a_later_run_surfaces_and_recovers() {
        let fixture = Fixture::new("crash-recovery");
        let first = fixture.write("first.pure", "before one");
        let second = fixture.write("second.pure", "before two");

        let mut first_staged =
            StagedReplacement::stage(replacement(&first, "before one", "after one"))
                .expect("stage first replacement");
        let second_staged =
            StagedReplacement::stage(replacement(&second, "before two", "after two"))
                .expect("stage second replacement");

        first_staged
            .switch(0, &mut NativeFileOperations)
            .expect("install the first file before the simulated crash");
        // The crash lands here, between the first and second file's exchange:
        // nothing further ever runs, `commit`'s loop included.
        std::mem::forget(second_staged);

        assert_eq!(fixture.read(&first), "after one");
        assert_eq!(fixture.read(&second), "before two");

        let mut leftovers = orphaned_stage_artifacts(&[
            replacement(&first, "after one", "irrelevant"),
            replacement(&second, "before two", "irrelevant"),
        ]);
        leftovers.sort();
        let mut expected: Vec<PathBuf> = fs::read_dir(&fixture.root)
            .expect("list fixture entries")
            .map(|entry| entry.expect("read fixture entry").path())
            .filter(|path| path.file_name().is_some_and(is_stage_artifact))
            .collect();
        expected.sort();
        assert_eq!(
            leftovers.len(),
            2,
            "the crash must leave exactly one backup and one uninstalled staging file"
        );
        assert_eq!(leftovers, expected);

        // A later invocation over the same, now half-updated tree must still
        // succeed, and must not silently delete what it did not create.
        replace_all(vec![
            replacement(&first, "after one", "final one"),
            replacement(&second, "before two", "final two"),
        ])
        .expect("a later run must not be blocked by the crash's leftovers");
        assert_eq!(fixture.read(&first), "final one");
        assert_eq!(fixture.read(&second), "final two");
        let still_present = orphaned_stage_artifacts(&[
            replacement(&first, "final one", "irrelevant"),
            replacement(&second, "final two", "irrelevant"),
        ]);
        assert_eq!(
            still_present, expected,
            "leftovers from the crash are surfaced, never silently removed by a later run"
        );
    }

    fn replacement(path: &Path, before: &str, after: &str) -> Replacement {
        Replacement {
            path: path.to_path_buf(),
            before: before.to_owned(),
            after: after.to_owned(),
        }
    }

    struct TestOperations {
        late_stale: Option<LateStale>,
        install_change: Option<LateStale>,
        staging_tamper: Option<StagingTamper>,
        rollback_change: Option<LateStale>,
        rollback_exchange_change: Option<LateStale>,
        failing_install: Option<usize>,
        install_failure_injected: bool,
    }

    impl TestOperations {
        fn late_stale(index: usize, path: PathBuf, text: &str) -> Self {
            Self {
                late_stale: Some(LateStale {
                    index,
                    path,
                    text: text.to_owned(),
                }),
                install_change: None,
                staging_tamper: None,
                rollback_change: None,
                rollback_exchange_change: None,
                failing_install: None,
                install_failure_injected: false,
            }
        }

        fn install_change(index: usize, path: PathBuf, text: &str) -> Self {
            Self {
                late_stale: None,
                install_change: Some(LateStale {
                    index,
                    path,
                    text: text.to_owned(),
                }),
                staging_tamper: None,
                rollback_change: None,
                rollback_exchange_change: None,
                failing_install: None,
                install_failure_injected: false,
            }
        }

        fn failing_install(index: usize) -> Self {
            Self {
                late_stale: None,
                install_change: None,
                staging_tamper: None,
                rollback_change: None,
                rollback_exchange_change: None,
                failing_install: Some(index),
                install_failure_injected: false,
            }
        }

        fn failing_install_with_rollback_change(
            failing_index: usize,
            rollback_index: usize,
            path: PathBuf,
            text: &str,
        ) -> Self {
            Self {
                late_stale: None,
                install_change: None,
                staging_tamper: None,
                rollback_change: Some(LateStale {
                    index: rollback_index,
                    path,
                    text: text.to_owned(),
                }),
                rollback_exchange_change: None,
                failing_install: Some(failing_index),
                install_failure_injected: false,
            }
        }

        fn failing_install_with_rollback_exchange_change(
            failing_index: usize,
            rollback_index: usize,
            path: PathBuf,
            text: &str,
        ) -> Self {
            Self {
                late_stale: None,
                install_change: None,
                staging_tamper: None,
                rollback_change: None,
                rollback_exchange_change: Some(LateStale {
                    index: rollback_index,
                    path,
                    text: text.to_owned(),
                }),
                failing_install: Some(failing_index),
                install_failure_injected: false,
            }
        }

        fn tamper_staging(index: usize, text: &str) -> Self {
            Self {
                late_stale: None,
                install_change: None,
                staging_tamper: Some(StagingTamper {
                    index,
                    text: text.to_owned(),
                }),
                rollback_change: None,
                rollback_exchange_change: None,
                failing_install: None,
                install_failure_injected: false,
            }
        }
    }

    impl FileOperations for TestOperations {
        fn before_late_validation(&mut self, index: usize, path: &Path) -> Result<(), Failure> {
            if let Some(stale) = &self.late_stale
                && stale.index == index
                && stale.path == path
            {
                fs::write(path, &stale.text).map_err(|error| {
                    Failure::internal(format!(
                        "could not update late-stale fixture {}: {error}",
                        path.display()
                    ))
                })?;
            }
            Ok(())
        }

        fn before_rollback(&mut self, index: usize, path: &Path) -> Result<(), Failure> {
            if let Some(change) = &self.rollback_change
                && change.index == index
                && change.path == path
            {
                fs::write(path, &change.text).map_err(|error| {
                    Failure::internal(format!(
                        "could not update rollback fixture {}: {error}",
                        path.display()
                    ))
                })?;
            }
            Ok(())
        }

        fn before_rollback_exchange(&mut self, index: usize, path: &Path) -> Result<(), Failure> {
            if let Some(change) = &self.rollback_exchange_change
                && change.index == index
                && change.path == path
            {
                fs::write(path, &change.text).map_err(|error| {
                    Failure::internal(format!(
                        "could not update rollback-exchange fixture {}: {error}",
                        path.display()
                    ))
                })?;
            }
            Ok(())
        }

        fn before_install(
            &mut self,
            index: usize,
            path: &Path,
            temporary: &Path,
        ) -> Result<(), Failure> {
            if let Some(change) = &self.install_change
                && change.index == index
                && change.path == path
            {
                fs::write(path, &change.text).map_err(|error| {
                    Failure::internal(format!(
                        "could not update before-install fixture {}: {error}",
                        path.display()
                    ))
                })?;
            }
            if let Some(tamper) = &self.staging_tamper
                && tamper.index == index
            {
                fs::write(temporary, &tamper.text).map_err(|error| {
                    Failure::internal(format!(
                        "could not tamper staged fixture {}: {error}",
                        temporary.display()
                    ))
                })?;
            }
            Ok(())
        }

        fn exchange(
            &mut self,
            operation: ExchangeOperation,
            index: usize,
            source: &Path,
            destination: &Path,
        ) -> io::Result<()> {
            if !self.install_failure_injected
                && self.failing_install == Some(index)
                && operation == ExchangeOperation::InstallStaged
            {
                self.install_failure_injected = true;
                return Err(io::Error::other("injected staged-install failure"));
            }
            exchange_for_test(source, destination)
        }
    }

    /// A deterministic stand-in for the platform exchange primitive. No test
    /// introduces a concurrent mutation within this three-rename sequence; the
    /// `before_install` seam controls the exact race boundary under test.
    fn exchange_for_test(source: &Path, destination: &Path) -> io::Result<()> {
        let intermediate = unique_sibling(source, "test-exchange")
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::rename(source, &intermediate)?;
        if let Err(error) = fs::rename(destination, source) {
            let _ = fs::rename(&intermediate, source);
            return Err(error);
        }
        if let Err(error) = fs::rename(&intermediate, destination) {
            let _ = fs::rename(source, destination);
            let _ = fs::rename(&intermediate, source);
            return Err(error);
        }
        Ok(())
    }

    struct LateStale {
        index: usize,
        path: PathBuf,
        text: String,
    }

    struct StagingTamper {
        index: usize,
        text: String,
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            for _ in 0..TEST_DIRECTORY_ATTEMPTS {
                let nonce = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let root = std::env::temp_dir().join(format!(
                    "pure-analyzer-writer-{label}-{}-{nonce}",
                    std::process::id()
                ));
                match fs::create_dir(&root) {
                    Ok(()) => return Self { root },
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create test directory {}: {error}", root.display()),
                }
            }
            panic!("reserve test directory for {label}");
        }

        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, text).expect("write fixture");
            path
        }

        fn read(&self, path: &Path) -> String {
            fs::read_to_string(path).expect("read fixture")
        }

        fn assert_no_writer_artifacts(&self) {
            let entries = fs::read_dir(&self.root).expect("list fixture directory");
            for entry in entries {
                let entry = entry.expect("read fixture entry");
                assert!(
                    !is_stage_artifact(&entry.file_name()),
                    "writer artifact remained at {}",
                    entry.path().display()
                );
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
