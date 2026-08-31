//! Transactional staging and atomic replacement of analyzed source files.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, Permissions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::Failure;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// One exact analyzed snapshot and the complete replacement to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Replacement {
    pub(super) path: PathBuf,
    pub(super) before: String,
    pub(super) after: String,
}

/// Stage every replacement before atomically switching any destination path.
pub(super) fn replace_all(replacements: Vec<Replacement>) -> Result<(), Failure> {
    if replacements.is_empty() {
        return Ok(());
    }
    let mut canonical = BTreeSet::new();
    let mut staged = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        let key = replacement.path.canonicalize().map_err(|error| {
            Failure::internal(format!(
                "could not resolve output {}: {error}",
                replacement.path.display()
            ))
        })?;
        if !canonical.insert(key) {
            return Err(Failure::internal(format!(
                "output path {} was selected more than once",
                replacement.path.display()
            )));
        }
        staged.push(StagedReplacement::new(replacement)?);
    }

    for replacement in &staged {
        replacement.verify_snapshot()?;
    }
    commit(staged)
}

#[derive(Debug)]
struct StagedReplacement {
    replacement: Replacement,
    temporary: PathBuf,
    backup: PathBuf,
}

impl StagedReplacement {
    fn new(replacement: Replacement) -> Result<Self, Failure> {
        let metadata = std::fs::symlink_metadata(&replacement.path).map_err(|error| {
            Failure::internal(format!(
                "could not inspect output {}: {error}",
                replacement.path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(Failure::usage(format!(
                "refusing to replace symbolic link {}",
                replacement.path.display()
            )));
        }
        if !metadata.is_file() {
            return Err(Failure::usage(format!(
                "output {} is not a regular file",
                replacement.path.display()
            )));
        }
        verify_text(&replacement.path, &replacement.before)?;
        let (temporary, mut file) = create_sibling(&replacement.path, "tmp")?;
        write_staged(
            &mut file,
            &replacement.after,
            metadata.permissions(),
            &temporary,
        )?;
        let backup = unused_sibling(&replacement.path, "backup")?;
        Ok(Self {
            replacement,
            temporary,
            backup,
        })
    }

    fn verify_snapshot(&self) -> Result<(), Failure> {
        verify_text(&self.replacement.path, &self.replacement.before)
    }
}

impl Drop for StagedReplacement {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}

fn create_sibling(path: &Path, role: &str) -> Result<(PathBuf, File), Failure> {
    for _ in 0..128 {
        let candidate = unique_sibling(path, role)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(Failure::internal(format!(
                    "could not stage output beside {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Err(Failure::internal(format!(
        "could not reserve a temporary path beside {}",
        path.display()
    )))
}

fn unique_sibling(path: &Path, role: &str) -> Result<PathBuf, Failure> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            Failure::internal(format!(
                "output path has no UTF-8 file name: {}",
                path.display()
            ))
        })?;
    let nonce = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        ".{name}.pure-analyzer-{role}-{}-{nonce}",
        std::process::id()
    )))
}

fn unused_sibling(path: &Path, role: &str) -> Result<PathBuf, Failure> {
    for _ in 0..128 {
        let candidate = unique_sibling(path, role)?;
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => {
                return Err(Failure::internal(format!(
                    "could not inspect backup path {}: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(Failure::internal(format!(
        "could not reserve a backup path beside {}",
        path.display()
    )))
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
            "could not preserve permissions on {}: {error}",
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

fn verify_text(path: &Path, expected: &str) -> Result<(), Failure> {
    let actual = std::fs::read_to_string(path).map_err(|error| {
        Failure::internal(format!(
            "could not re-read output {}: {error}",
            path.display()
        ))
    })?;
    if actual != expected {
        return Err(Failure::usage(format!(
            "{} changed after it was analyzed; no files were written",
            path.display()
        )));
    }
    Ok(())
}

fn commit(mut staged: Vec<StagedReplacement>) -> Result<(), Failure> {
    let mut committed = Vec::new();
    for replacement in &mut staged {
        if let Err(error) = switch(replacement) {
            let rollback = rollback(&committed);
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(Failure::internal(format!(
                    "{error}; rollback also failed: {rollback_error}"
                ))),
            };
        }
        committed.push(Committed {
            path: replacement.replacement.path.clone(),
            backup: replacement.backup.clone(),
        });
    }
    for replacement in committed {
        std::fs::remove_file(&replacement.backup).map_err(|error| {
            Failure::internal(format!(
                "updated {} but could not remove backup {}: {error}",
                replacement.path.display(),
                replacement.backup.display()
            ))
        })?;
    }
    Ok(())
}

fn switch(replacement: &StagedReplacement) -> Result<(), Failure> {
    std::fs::rename(&replacement.replacement.path, &replacement.backup).map_err(|error| {
        Failure::internal(format!(
            "could not stage original {} for replacement: {error}",
            replacement.replacement.path.display()
        ))
    })?;
    if let Err(error) = std::fs::rename(&replacement.temporary, &replacement.replacement.path) {
        let restore = std::fs::rename(&replacement.backup, &replacement.replacement.path);
        return match restore {
            Ok(()) => Err(Failure::internal(format!(
                "could not replace {} atomically: {error}",
                replacement.replacement.path.display()
            ))),
            Err(restore_error) => Err(Failure::internal(format!(
                "could not replace {}: {error}; could not restore original: {restore_error}",
                replacement.replacement.path.display()
            ))),
        };
    }
    Ok(())
}

#[derive(Debug)]
struct Committed {
    path: PathBuf,
    backup: PathBuf,
}

fn rollback(committed: &[Committed]) -> Result<(), Failure> {
    for replacement in committed.iter().rev() {
        std::fs::remove_file(&replacement.path).map_err(|error| {
            Failure::internal(format!(
                "could not remove partial output {}: {error}",
                replacement.path.display()
            ))
        })?;
        std::fs::rename(&replacement.backup, &replacement.path).map_err(|error| {
            Failure::internal(format!(
                "could not restore {} from {}: {error}",
                replacement.path.display(),
                replacement.backup.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_all_files_after_staging() {
        let root = fixture_root("success");
        let first = root.join("first.pure");
        let second = root.join("second.pure");
        std::fs::write(&first, "before one").expect("write first fixture");
        std::fs::write(&second, "before two").expect("write second fixture");

        replace_all(vec![
            Replacement {
                path: first.clone(),
                before: "before one".to_owned(),
                after: "after one".to_owned(),
            },
            Replacement {
                path: second.clone(),
                before: "before two".to_owned(),
                after: "after two".to_owned(),
            },
        ])
        .expect("replace fixtures");

        assert_eq!(
            std::fs::read_to_string(first).expect("read first"),
            "after one"
        );
        assert_eq!(
            std::fs::read_to_string(second).expect("read second"),
            "after two"
        );
        std::fs::remove_dir_all(root).expect("remove fixtures");
    }

    #[test]
    fn stale_snapshot_prevents_every_write() {
        let root = fixture_root("stale");
        let first = root.join("first.pure");
        let second = root.join("second.pure");
        std::fs::write(&first, "current one").expect("write first fixture");
        std::fs::write(&second, "current two").expect("write second fixture");

        let error = replace_all(vec![
            Replacement {
                path: first.clone(),
                before: "current one".to_owned(),
                after: "after one".to_owned(),
            },
            Replacement {
                path: second.clone(),
                before: "stale two".to_owned(),
                after: "after two".to_owned(),
            },
        ])
        .expect_err("stale snapshot must fail");

        assert_eq!(error.exit_code(), super::super::EXIT_USAGE);
        assert_eq!(
            std::fs::read_to_string(first).expect("read first"),
            "current one"
        );
        assert_eq!(
            std::fs::read_to_string(second).expect("read second"),
            "current two"
        );
        assert!(
            std::fs::read_dir(&root)
                .expect("read fixture directory")
                .all(|entry| !entry
                    .expect("read fixture entry")
                    .file_name()
                    .to_string_lossy()
                    .contains("pure-analyzer-tmp"))
        );
        std::fs::remove_dir_all(root).expect("remove fixtures");
    }

    fn fixture_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "pure-analyzer-atomic-{label}-{}-{}",
            std::process::id(),
            super::super::test_nonce()
        ));
        std::fs::create_dir_all(&root).expect("create fixture directory");
        root
    }
}
