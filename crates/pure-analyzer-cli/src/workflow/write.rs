//! Transactional, staged replacement of analyzed source files.

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
const BACKUP_ROLE: &str = "backup";

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
        return Err(clean_link_after_failure(error, &replacement.backup));
    }
    operations
        .replace(
            ReplaceOperation::RestoreRolledBack,
            replacement.index,
            &replacement.backup,
            &replacement.path,
        )
        .map_err(|error| {
            Failure::internal(format!(
                "could not atomically restore original output {} while rolling back: {error}",
                replacement.path.display()
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
        let backup = self.link_original_to_backup(index, operations)?;
        if let Err(error) = verify_snapshot(&self.replacement.path, &self.replacement.before) {
            return Err(clean_link_after_failure(error, &backup));
        }
        let temporary = self.temporary_path()?.to_owned();

        match operations.replace(
            ReplaceOperation::InstallStaged,
            index,
            &temporary,
            &self.replacement.path,
        ) {
            Ok(()) => {
                self.temporary = None;
                Ok(CommittedReplacement {
                    index,
                    path: self.replacement.path.clone(),
                    backup,
                    after: self.replacement.after.clone(),
                })
            }
            Err(error) => Err(clean_link_after_failure(
                Failure::internal(format!(
                    "could not atomically install staged output {}: {error}",
                    self.replacement.path.display()
                )),
                &backup,
            )),
        }
    }

    fn link_original_to_backup<O: FileOperations>(
        &self,
        index: usize,
        operations: &mut O,
    ) -> Result<PathBuf, Failure> {
        let backup = link_to_unique_sibling(
            operations,
            LinkOperation::PreserveOriginal,
            index,
            &self.replacement.path,
            BACKUP_ROLE,
        )?;
        match verify_snapshot(&backup, &self.replacement.before) {
            Ok(_) => Ok(backup),
            Err(error) => Err(clean_link_after_failure(error, &backup)),
        }
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
    after: String,
}

trait FileOperations {
    fn before_late_validation(&mut self, _index: usize, _path: &Path) -> Result<(), Failure> {
        Ok(())
    }

    fn before_rollback(&mut self, _index: usize, _path: &Path) -> Result<(), Failure> {
        Ok(())
    }

    fn hard_link(
        &mut self,
        _operation: LinkOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()>;

    fn replace(
        &mut self,
        _operation: ReplaceOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()>;
}

struct NativeFileOperations;

impl FileOperations for NativeFileOperations {
    fn hard_link(
        &mut self,
        _operation: LinkOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()> {
        fs::hard_link(source, destination)
    }

    fn replace(
        &mut self,
        _operation: ReplaceOperation,
        _index: usize,
        source: &Path,
        destination: &Path,
    ) -> io::Result<()> {
        fs::rename(source, destination)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkOperation {
    PreserveOriginal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplaceOperation {
    InstallStaged,
    RestoreRolledBack,
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

fn link_to_unique_sibling<O: FileOperations>(
    operations: &mut O,
    operation: LinkOperation,
    index: usize,
    source: &Path,
    role: &str,
) -> Result<PathBuf, Failure> {
    for _ in 0..MAX_UNIQUE_NAME_ATTEMPTS {
        let candidate = unique_sibling(source, role)?;
        match operations.hard_link(operation, index, source, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(Failure::internal(format!(
                    "could not create transactional link {}: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(Failure::internal(format!(
        "could not reserve a transactional link beside {}",
        source.display()
    )))
}

fn unique_sibling(path: &Path, role: &str) -> Result<PathBuf, Failure> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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

fn clean_link_after_failure(error: Failure, link: &Path) -> Failure {
    match remove_owned_file(link) {
        Ok(()) => error,
        Err(cleanup_error) => Failure::internal(format!(
            "{error}; additionally, could not remove transactional link {}: {cleanup_error}",
            link.display()
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

    #[cfg(unix)]
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

    fn replacement(path: &Path, before: &str, after: &str) -> Replacement {
        Replacement {
            path: path.to_path_buf(),
            before: before.to_owned(),
            after: after.to_owned(),
        }
    }

    struct TestOperations {
        late_stale: Option<LateStale>,
        rollback_change: Option<LateStale>,
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
                rollback_change: None,
                failing_install: None,
                install_failure_injected: false,
            }
        }

        fn failing_install(index: usize) -> Self {
            Self {
                late_stale: None,
                rollback_change: None,
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
                rollback_change: Some(LateStale {
                    index: rollback_index,
                    path,
                    text: text.to_owned(),
                }),
                failing_install: Some(failing_index),
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

        fn hard_link(
            &mut self,
            _operation: LinkOperation,
            _index: usize,
            source: &Path,
            destination: &Path,
        ) -> io::Result<()> {
            fs::hard_link(source, destination)
        }

        fn replace(
            &mut self,
            operation: ReplaceOperation,
            index: usize,
            source: &Path,
            destination: &Path,
        ) -> io::Result<()> {
            if !self.install_failure_injected
                && self.failing_install == Some(index)
                && operation == ReplaceOperation::InstallStaged
            {
                self.install_failure_injected = true;
                return Err(io::Error::other("injected staged-install failure"));
            }
            fs::rename(source, destination)
        }
    }

    struct LateStale {
        index: usize,
        path: PathBuf,
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
                    !entry.file_name().to_string_lossy().contains(WRITER_MARKER),
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
