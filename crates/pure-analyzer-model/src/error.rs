use std::path::PathBuf;

use crate::{Name, QName, SourceId};

/// Why a PMCD element could not be normalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelErrorKind {
    /// A required field was missing or had the wrong JSON type.
    #[error("record shape is invalid: {0}")]
    InvalidRecord(String),
    /// A packageable element name or path was invalid.
    #[error(transparent)]
    InvalidName(#[from] NameError),
    /// A property or return multiplicity was invalid.
    #[error(transparent)]
    InvalidMultiplicity(#[from] MultiplicityError),
    /// The same packageable path occurred twice in one PMCD document.
    #[error("duplicate packageable element `{path}` in one PMCD document")]
    DuplicateElement {
        /// The duplicated fully-qualified path.
        path: QName,
    },
    /// A class declared the same simple property more than once.
    #[error("class `{class}` declares property `{property}` more than once")]
    DuplicateProperty {
        /// The containing class.
        class: QName,
        /// The duplicated property name.
        property: Name,
    },
    /// A class declared the same qualified-property name more than once.
    #[error("class `{class}` declares qualified property `{property}` more than once")]
    DuplicateQualifiedProperty {
        /// The containing class.
        class: QName,
        /// The duplicated qualified-property name.
        property: Name,
    },
    /// An association did not contain exactly two ends.
    #[error("association `{association}` has {actual} ends; exactly two are required")]
    AssociationArity {
        /// The association path.
        association: QName,
        /// Number of end records found.
        actual: usize,
    },
    /// An association end's owning class was absent after all sources merged.
    #[error(
        "association `{association}` property `{property}` is owned by missing class `{owner}`"
    )]
    MissingAssociationOwner {
        /// The association path.
        association: QName,
        /// The contributed property name.
        property: Name,
        /// The class from which the property must be navigable.
        owner: QName,
    },
    /// An association contribution collided with another simple property.
    #[error(
        "association `{association}` contributes `{owner}.{property}`, which is already declared"
    )]
    AssociationPropertyConflict {
        /// The association path.
        association: QName,
        /// The class receiving the association end.
        owner: QName,
        /// The colliding property name.
        property: Name,
    },
    /// A class carried more than one temporal stereotype.
    #[error("`{element}` has multiple temporal stereotypes")]
    MultipleTemporalStereotypes {
        /// Class or association path.
        element: QName,
    },
    /// A temporal-profile stereotype used a value the analyzer cannot classify.
    #[error("`{element}` has unknown temporal stereotype `{value}`")]
    UnknownTemporalStereotype {
        /// Class or association path.
        element: QName,
        /// Unrecognized stereotype value.
        value: String,
    },
}

/// Failure to load or normalize PMCD input.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// A PMCD file could not be read as UTF-8 text.
    #[error("failed to read PMCD file `{path}`: {source}")]
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The input was not syntactically valid JSON.
    #[error("PMCD source `{source_name}` is not valid JSON: {source}")]
    Json {
        /// Stable source label supplied by the caller.
        source_name: String,
        /// JSON syntax/deserialization error.
        #[source]
        source: serde_json::Error,
    },
    /// The PMCD document envelope was malformed.
    #[error("PMCD source `{source_name}` has an invalid document envelope: {message}")]
    InvalidDocument {
        /// Stable source label supplied by the caller.
        source_name: String,
        /// Specific envelope failure.
        message: String,
    },
    /// A relevant class or association record was malformed.
    #[error(
        "PMCD source `{source_name}` element #{element_index} ({element_kind}) is invalid: {kind}"
    )]
    InvalidElement {
        /// Stable source label supplied by the caller.
        source_name: String,
        /// Zero-based position in `elements`.
        element_index: usize,
        /// PMCD discriminator, or `unknown` when it could not be read.
        element_kind: String,
        /// Typed normalization failure.
        kind: Box<ModelErrorKind>,
    },
    /// A final-graph invariant failed after multiple sources merged.
    #[error("merged model from source {source_id} is invalid: {kind}")]
    InvalidMergedGraph {
        /// Source that introduced the failing association or class.
        source_id: SourceId,
        /// Typed graph invariant failure.
        kind: ModelErrorKind,
    },
    /// More source files were supplied than can be represented by [`SourceId`].
    #[error("too many model sources; source index {index} exceeds the supported range")]
    TooManySources {
        /// First unrepresentable zero-based source index.
        index: usize,
    },
    /// More classes were supplied than can be represented by `ClassId`.
    #[error("too many classes; class index {index} exceeds the supported range")]
    TooManyClasses {
        /// First unrepresentable zero-based class index.
        index: usize,
    },
}

/// Failure to construct a Pure name or qualified name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// The supplied name was empty.
    #[error("name must not be empty")]
    Empty,
    /// The supplied name contained a control character.
    #[error("name `{0}` contains a control character")]
    ControlCharacter(String),
    /// A simple name contained the qualified-path separator.
    #[error("simple name `{0}` must not contain `::`")]
    QualifiedSimpleName(String),
    /// A qualified path contained an empty segment.
    #[error("qualified name `{0}` contains an empty path segment")]
    EmptyPathSegment(String),
}

/// Failure to construct a valid multiplicity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("multiplicity lower bound {lower} exceeds upper bound {upper}")]
pub struct MultiplicityError {
    /// Invalid lower bound.
    pub lower: u32,
    /// Invalid upper bound.
    pub upper: u32,
}
