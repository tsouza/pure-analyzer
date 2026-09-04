//! Snapshot-backed source ownership for analyzer front ends.

use std::path::PathBuf;

use pure_analyzer_diagnostics::{FileId, TextSize};
use thiserror::Error;

const FIRST_FILE_ID: u32 = 0;
const FIRST_LINE: usize = 1;
const FIRST_COLUMN: usize = 1;
const STDIN_NAME: &str = "<stdin>";

/// One source input accepted by the renderer-independent analysis facade.
///
/// `Stdin` contains already-read bytes deliberately: the facade never reads
/// process stdin, so callers cannot accidentally analyze one snapshot and
/// render or apply fixes to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    /// Read UTF-8 source from a filesystem path exactly once.
    File {
        /// The path to read.
        path: PathBuf,
    },
    /// Analyze a caller-owned source snapshot while retaining its file origin.
    ///
    /// The path is used only as the diagnostic display name and
    /// [`SourceOrigin::File`] value. It is never read by [`SourceStore`].
    FileSnapshot {
        /// The filesystem path from which the caller obtained the snapshot.
        path: PathBuf,
        /// The exact UTF-8 source bytes captured by the caller.
        text: String,
    },
    /// Analyze caller-owned source text under a stable display name.
    InMemory {
        /// The name used by diagnostics and renderers.
        name: String,
        /// The exact UTF-8 source bytes to analyze.
        text: String,
    },
    /// Analyze a caller-owned snapshot that originated from standard input.
    Stdin {
        /// The exact UTF-8 standard-input bytes to analyze.
        text: String,
    },
}

impl SourceInput {
    /// Construct a filesystem-backed input.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into() }
    }

    /// Construct a caller-owned snapshot that retains a filesystem origin.
    ///
    /// Unlike [`SourceInput::file`], this constructor does not cause
    /// [`SourceStore`] to access `path`.
    #[must_use]
    pub fn file_snapshot(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self::FileSnapshot {
            path: path.into(),
            text: text.into(),
        }
    }

    /// Construct an in-memory input with an explicit diagnostic name.
    #[must_use]
    pub fn in_memory(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self::InMemory {
            name: name.into(),
            text: text.into(),
        }
    }

    /// Construct an input from bytes already read from standard input.
    #[must_use]
    pub fn stdin(text: impl Into<String>) -> Self {
        Self::Stdin { text: text.into() }
    }
}

/// The origin category of one retained source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOrigin {
    /// The source originated from this path.
    ///
    /// It was either read by [`SourceStore`] or supplied as a caller-owned
    /// [`SourceInput::FileSnapshot`].
    File {
        /// The path associated with the source snapshot.
        path: PathBuf,
    },
    /// The source was supplied directly by an API caller.
    InMemory,
    /// The source was supplied from an already-read standard-input snapshot.
    Stdin,
}

/// A one-based line and byte-column location in a UTF-8 source snapshot.
///
/// This is the column the human and JSON renderers emit. SARIF output emits
/// a separately computed Unicode code-point column instead, to match its own
/// declared `columnKind`; see `pure-analyzer-render`'s `code_point_column`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineColumn {
    /// One-based source line number.
    pub line: usize,
    /// One-based byte column within `line`.
    pub column: usize,
}

/// One source snapshot retained for an analysis request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    id: FileId,
    name: String,
    origin: SourceOrigin,
    text: String,
    lines: LineIndex,
}

impl SourceFile {
    /// Return the request-local identity used by diagnostic labels.
    #[must_use]
    pub const fn id(&self) -> FileId {
        self.id
    }

    /// Return the stable display name for diagnostics and renderers.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return how this source entered the request.
    #[must_use]
    pub const fn origin(&self) -> &SourceOrigin {
        &self.origin
    }

    /// Return the exact UTF-8 bytes retained for parsing, rendering, and fixes.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Convert a valid byte offset into one-based line and byte-column values.
    #[must_use]
    pub fn line_column(&self, offset: TextSize) -> Option<LineColumn> {
        self.lines.locate(&self.text, offset)
    }
}

/// Per-request storage that prevents source rereads and stale-source drift.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceStore {
    files: Vec<SourceFile>,
}

impl SourceStore {
    /// Load each input once and retain its exact source snapshot.
    ///
    /// The supplied order determines stable [`FileId`] allocation.
    ///
    /// # Errors
    ///
    /// Returns [`SourceStoreError`] when a path cannot be read, a display name
    /// is unusable, or a source set cannot be represented by [`FileId`].
    pub fn load(inputs: impl IntoIterator<Item = SourceInput>) -> Result<Self, SourceStoreError> {
        Self::load_from(FIRST_FILE_ID, inputs)
    }

    /// Return source files in stable request order.
    pub fn files(&self) -> impl ExactSizeIterator<Item = &SourceFile> + DoubleEndedIterator {
        self.files.iter()
    }

    /// Return the retained source associated with `file`.
    #[must_use]
    pub fn get(&self, file: FileId) -> Option<&SourceFile> {
        self.files.iter().find(|source| source.id == file)
    }

    /// Return the number of retained source files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Return whether this store has no source files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub(crate) fn load_from(
        first_id: u32,
        inputs: impl IntoIterator<Item = SourceInput>,
    ) -> Result<Self, SourceStoreError> {
        let mut files = Vec::new();
        for (index, input) in inputs.into_iter().enumerate() {
            let id = file_id(first_id, index)?;
            files.push(SourceFile::load(id, input)?);
        }
        Ok(Self { files })
    }

    pub(crate) fn append(mut self, other: Self) -> Self {
        self.files.extend(other.files);
        self
    }
}

impl SourceFile {
    fn load(id: FileId, input: SourceInput) -> Result<Self, SourceStoreError> {
        let (name, origin, text) = match input {
            SourceInput::File { path } => {
                let text =
                    std::fs::read_to_string(&path).map_err(|source| SourceStoreError::Read {
                        path: path.clone(),
                        source,
                    })?;
                (
                    path.display().to_string(),
                    SourceOrigin::File { path },
                    text,
                )
            }
            SourceInput::FileSnapshot { path, text } => (
                path.display().to_string(),
                SourceOrigin::File { path },
                text,
            ),
            SourceInput::InMemory { name, text } => (name, SourceOrigin::InMemory, text),
            SourceInput::Stdin { text } => (STDIN_NAME.to_owned(), SourceOrigin::Stdin, text),
        };
        if name.is_empty() {
            return Err(SourceStoreError::EmptyName);
        }
        if u32::try_from(text.len()).is_err() {
            return Err(SourceStoreError::SourceTooLong {
                name,
                length: text.len(),
            });
        }
        Ok(Self {
            id,
            name,
            origin,
            lines: LineIndex::new(&text),
            text,
        })
    }
}

fn file_id(first_id: u32, index: usize) -> Result<FileId, SourceStoreError> {
    let Some(index) = u32::try_from(index).ok() else {
        return Err(SourceStoreError::TooManySources);
    };
    let Some(raw) = first_id.checked_add(index) else {
        return Err(SourceStoreError::TooManySources);
    };
    Ok(FileId::new(raw))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(offset.saturating_add(1));
            }
        }
        Self { starts }
    }

    fn locate(&self, text: &str, offset: TextSize) -> Option<LineColumn> {
        let offset = usize::from(offset);
        if offset > text.len() || !text.is_char_boundary(offset) {
            return None;
        }
        let line_index = match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.checked_sub(1)?,
        };
        let start = *self.starts.get(line_index)?;
        Some(LineColumn {
            line: line_index.saturating_add(FIRST_LINE),
            column: offset.saturating_sub(start).saturating_add(FIRST_COLUMN),
        })
    }
}

/// A failure while retaining frontend-owned source snapshots.
#[derive(Debug, Error)]
pub enum SourceStoreError {
    /// A filesystem source could not be read as UTF-8 text.
    #[error("could not read source `{path}`: {source}")]
    Read {
        /// The path that could not be read.
        path: PathBuf,
        /// The underlying I/O or UTF-8 failure.
        #[source]
        source: std::io::Error,
    },
    /// An in-memory display name was empty.
    #[error("an in-memory source needs a non-empty display name")]
    EmptyName,
    /// A source exceeds the byte range used by parser spans.
    #[error("source `{name}` has {length} bytes, exceeding the parser span limit")]
    SourceTooLong {
        /// The source display name.
        name: String,
        /// The unrepresentable byte length.
        length: usize,
    },
    /// The input sequence cannot be assigned distinct request-local file IDs.
    #[error("source set has too many files to assign stable IDs")]
    TooManySources,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static TEMP_FILE_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temporary_path() -> PathBuf {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pure-analyzer-libpure-source-{}-{counter}.pure",
            std::process::id()
        ))
    }

    #[test]
    fn retains_file_memory_and_stdin_snapshots_once_in_order() {
        let path = temporary_path();
        std::fs::write(&path, "file()\n").expect("write fixture");
        let store = SourceStore::load([
            SourceInput::file(&path),
            SourceInput::in_memory("memory.pure", "memory()\n"),
            SourceInput::stdin("stdin()\n"),
        ])
        .expect("load snapshots");
        std::fs::remove_file(&path).expect("remove fixture");

        let files = store.files().collect::<Vec<_>>();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].id(), FileId::new(0));
        assert_eq!(files[1].id(), FileId::new(1));
        assert_eq!(files[2].id(), FileId::new(2));
        assert_eq!(files[0].text(), "file()\n");
        assert_eq!(files[1].name(), "memory.pure");
        assert_eq!(files[2].name(), STDIN_NAME);
        assert!(matches!(files[0].origin(), SourceOrigin::File { .. }));
        assert!(matches!(files[1].origin(), SourceOrigin::InMemory));
        assert!(matches!(files[2].origin(), SourceOrigin::Stdin));
    }

    #[test]
    fn retains_caller_owned_file_snapshot_without_rereading_its_path() {
        let path = temporary_path();
        assert!(
            !path.exists(),
            "the fixture path must not exist so a disk read would fail"
        );

        let store = SourceStore::load([SourceInput::file_snapshot(&path, "captured()\n")])
            .expect("caller-owned file snapshot must not read its path");
        let source = store.get(FileId::new(0)).expect("retained source");

        assert_eq!(source.name(), path.display().to_string());
        assert_eq!(source.text(), "captured()\n");
        assert_eq!(source.origin(), &SourceOrigin::File { path });
    }

    #[test]
    fn indexes_multibyte_source_at_valid_utf8_boundaries() {
        let store = SourceStore::load([SourceInput::in_memory("unicode.pure", "aé\nβ")])
            .expect("load snapshot");
        let file = store.get(FileId::new(0)).expect("retained source");

        assert_eq!(
            file.line_column(TextSize::new(3)),
            Some(LineColumn { line: 1, column: 4 })
        );
        assert_eq!(
            file.line_column(TextSize::new(4)),
            Some(LineColumn { line: 2, column: 1 })
        );
        assert_eq!(file.line_column(TextSize::new(2)), None);
    }

    #[test]
    fn rejects_empty_in_memory_names_without_losing_error_category() {
        let error = SourceStore::load([SourceInput::in_memory("", "query()")])
            .expect_err("empty name must be rejected");
        assert!(matches!(error, SourceStoreError::EmptyName));
    }
}
