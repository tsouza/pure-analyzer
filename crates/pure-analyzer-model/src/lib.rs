#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Deterministic Legend Pure model facts and PMCD/Pure-source ingestion.
//!
//! This crate owns the normalized [`ModelGraph`] consumed by resolution and
//! analysis. PMCD and Pure Domain sources can be loaded independently or
//! mixed through the model-loading entry points. Every path accepts multiple
//! sources, applies documented last-source-wins merging, and reports each
//! replacement as a `PUR9000` diagnostic on the returned graph.
//!
//! PMCD elements outside this crate's class/association surface are ignored:
//! profiles, mappings, runtimes, connections, and relational stores can evolve
//! independently. Class and association records are fail-closed because a
//! partially interpreted relevant record would make resolver facts unsound.
//!
//! [`Provenance::PureFile`] marks facts lowered from the resilient Domain
//! parser. Its per-class coverage flag keeps incomplete Pure files and Pure
//! association declarations open-world.

mod error;
mod loader;
mod pure;
mod raw;
mod types;

use pure_analyzer_diagnostics::DiagCode;

pub use error::{ModelError, ModelErrorKind, MultiplicityError, NameError};
pub use loader::{
    ModelDocument, PmcdDocument, PureDocument, load_model_documents, load_model_files,
    load_pmcd_documents, load_pmcd_files, load_pure_documents, load_pure_files,
};
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
