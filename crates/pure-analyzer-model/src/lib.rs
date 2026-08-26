#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Deterministic Legend Pure model facts and PMCD JSON ingestion.
//!
//! This crate owns the normalized [`ModelGraph`] consumed by resolution and
//! analysis. [`load_pmcd_files`] loads engine-produced
//! `PureModelContextData` files, while [`load_pmcd_documents`] provides the
//! same complete path for callers that already hold JSON in memory. Both
//! accept multiple sources, apply documented last-source-wins merging, and
//! report each replacement as a `PUR9000` diagnostic on the returned graph.
//!
//! PMCD elements outside this crate's class/association surface are ignored:
//! profiles, mappings, runtimes, connections, and relational stores can evolve
//! independently. Class and association records are fail-closed because a
//! partially interpreted relevant record would make resolver facts unsound.
//!
//! [`Provenance::PureFile`] identifies Pure-file origin in the normalized model
//! API. This module's public loading functions accept PMCD JSON only.

mod error;
mod loader;
mod raw;
mod types;

use pure_analyzer_diagnostics::DiagCode;

pub use error::{ModelError, ModelErrorKind, MultiplicityError, NameError};
pub use loader::{PmcdDocument, load_pmcd_documents, load_pmcd_files};
pub use types::{
    AssocInfo, AssociationEndInfo, ClassId, ClassInfo, ModelGraph, ModelSource, ModelSourceInfo,
    Multiplicity, Name, PropInfo, Provenance, QName, QpInfo, QpKind, SourceId, Temporal, TypeRef,
};

/// Diagnostic code for a model element replaced by a later source.
pub const MODEL_MERGE_CONFLICT: DiagCode = DiagCode::ModelMergeConflict;

/// The crate's semantic version, as declared in `Cargo.toml`.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_workspace_version() {
        assert_eq!(version(), "0.1.0");
    }
}
