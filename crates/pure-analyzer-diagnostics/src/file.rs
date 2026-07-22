//! Identifies a source file within a single analysis run.

use std::fmt;

/// A source file, identified by its position in the set of files loaded for
/// one analysis run (a CLI invocation or one LSP document sync).
///
/// `FileId` is deliberately an opaque index rather than a path: paths are
/// owned by whichever front-end loaded the file (CLI walks the filesystem,
/// LSP tracks open buffers), and keeping spans path-free lets a `Diagnostic`
/// stay `Copy`-cheap and stable across a rename.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct FileId(u32);

impl FileId {
    /// Construct a `FileId` from a raw index.
    ///
    /// Callers (the CLI's file-loading loop, the LSP's document store) own
    /// the mapping from index back to path/URI; this type only carries the
    /// index through the analysis passes.
    #[must_use]
    pub fn new(index: u32) -> Self {
        Self(index)
    }

    /// The raw index this `FileId` wraps.
    #[must_use]
    pub fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file#{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_raw_index() {
        assert_eq!(FileId::new(7).index(), 7);
    }

    #[test]
    fn orders_by_index() {
        assert!(FileId::new(0) < FileId::new(1));
    }

    #[test]
    fn displays_with_a_stable_prefix() {
        assert_eq!(FileId::new(3).to_string(), "file#3");
    }
}
