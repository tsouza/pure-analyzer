use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use pure_analyzer_diagnostics::{Diagnostic, FileId};

use crate::{MultiplicityError, NameError};

/// A validated Pure simple name, such as a property or packageable-element name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    /// Validate and construct a simple name.
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] for an empty name, control characters, or a
    /// qualified path supplied where a simple name is required.
    pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
        let value = value.into();
        validate_name_text(&value)?;
        if value.contains("::") {
            return Err(NameError::QualifiedSimpleName(value));
        }
        Ok(Self(value))
    }

    /// Borrow the name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Name {
    type Err = NameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// A validated Pure qualified name, including primitive names such as `Integer`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QName(String);

impl QName {
    /// Validate and construct a qualified name.
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] when the path is empty, contains a control
    /// character, or has an empty `::`-delimited segment.
    pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
        let value = value.into();
        validate_name_text(&value)?;
        if value.split("::").any(str::is_empty) {
            return Err(NameError::EmptyPathSegment(value));
        }
        Ok(Self(value))
    }

    /// Construct a packageable path from separate PMCD `package` and `name` fields.
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] if either component cannot form a valid path.
    pub fn from_package_and_name(package: &str, name: &str) -> Result<Self, NameError> {
        let simple = Name::new(name)?;
        if package.is_empty() {
            return Self::new(simple.0);
        }
        Self::new(format!("{package}::{simple}"))
    }

    /// Borrow the fully-qualified path as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return the final segment of this qualified name.
    #[must_use]
    pub fn simple_name(&self) -> &str {
        self.0.rsplit("::").next().unwrap_or(self.as_str())
    }

    /// Return the package portion, or `None` for a root-level name.
    #[must_use]
    pub fn package(&self) -> Option<&str> {
        self.0.rsplit_once("::").map(|(package, _)| package)
    }
}

impl Borrow<str> for QName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for QName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for QName {
    type Err = NameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

fn validate_name_text(value: &str) -> Result<(), NameError> {
    if value.is_empty() {
        return Err(NameError::Empty);
    }
    if value.chars().any(char::is_control) {
        return Err(NameError::ControlCharacter(value.to_owned()));
    }
    Ok(())
}

/// Stable identifier for one class within a loaded graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClassId(u32);

impl ClassId {
    pub(crate) const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Return the deterministic zero-based class index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ClassId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "class#{}", self.0)
    }
}

/// Stable identifier for one model source within a loading operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u32);

impl SourceId {
    pub(crate) const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Return the zero-based source index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// Convert to the shared diagnostic file identifier used for this source.
    #[must_use]
    pub fn file_id(self) -> FileId {
        FileId::new(self.0)
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "source#{}", self.0)
    }
}

/// Input kinds represented by the analyzer model API.
///
/// [`crate::load_model_files`] accepts either variant and normalizes both into
/// the same [`ModelGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// Engine-produced PureModelContextData JSON.
    PmcdJson(PathBuf),
    /// Engine-free Pure Domain model source.
    PureModelFile(PathBuf),
}

impl ModelSource {
    /// Borrow the source path.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::PmcdJson(path) | Self::PureModelFile(path) => path,
        }
    }

    /// Return the provenance facts loaded from this source carry.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        match self {
            Self::PmcdJson(_) => Provenance::Pmcd,
            Self::PureModelFile(_) => Provenance::PureFile,
        }
    }
}

/// Where a normalized model fact originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provenance {
    /// Engine-produced PureModelContextData JSON; treated as closed-world.
    Pmcd,
    /// Pure Domain source; its per-class coverage flag controls open-world use.
    PureFile,
}

/// Metadata for one source represented in a [`ModelGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSourceInfo {
    id: SourceId,
    label: String,
    provenance: Provenance,
}

impl ModelSourceInfo {
    pub(crate) fn new(id: SourceId, label: String, provenance: Provenance) -> Self {
        Self {
            id,
            label,
            provenance,
        }
    }

    /// Return this source's stable identifier.
    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    /// Return the caller-provided path or in-memory source label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Return the kind of model source.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }
}

/// Declared multiplicity bounds; `upper == None` represents `*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Multiplicity {
    lower: u32,
    upper: Option<u32>,
}

impl Multiplicity {
    /// Construct the conventional zero-or-more multiplicity (`[0..*]`).
    #[must_use]
    pub const fn zero_or_more() -> Self {
        Self {
            lower: 0,
            upper: None,
        }
    }

    /// Construct validated multiplicity bounds.
    ///
    /// # Errors
    ///
    /// Returns [`MultiplicityError`] when a finite upper bound is below the
    /// lower bound.
    pub const fn new(lower: u32, upper: Option<u32>) -> Result<Self, MultiplicityError> {
        if let Some(upper) = upper
            && lower > upper
        {
            return Err(MultiplicityError { lower, upper });
        }
        Ok(Self { lower, upper })
    }

    /// Lower bound.
    #[must_use]
    pub const fn lower(self) -> u32 {
        self.lower
    }

    /// Finite upper bound, or `None` for unbounded.
    #[must_use]
    pub const fn upper(self) -> Option<u32> {
        self.upper
    }

    /// Whether this multiplicity has no finite upper bound.
    #[must_use]
    pub const fn is_unbounded(self) -> bool {
        self.upper.is_none()
    }

    /// Whether this multiplicity admits at most one value.
    #[must_use]
    pub const fn is_to_one(self) -> bool {
        matches!(self.upper, Some(0 | 1))
    }
}

/// A named Pure type and any recursively declared generic type arguments.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeRef {
    raw_type: QName,
    type_arguments: Vec<TypeRef>,
}

impl TypeRef {
    /// Construct a named type reference.
    #[must_use]
    pub const fn new(raw_type: QName, type_arguments: Vec<Self>) -> Self {
        Self {
            raw_type,
            type_arguments,
        }
    }

    /// The raw named type, before generic arguments are applied.
    #[must_use]
    pub const fn raw_type(&self) -> &QName {
        &self.raw_type
    }

    /// Generic type arguments in declaration order.
    #[must_use]
    pub fn type_arguments(&self) -> &[Self] {
        &self.type_arguments
    }
}

/// A class or association's own temporal stereotype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Temporal {
    /// Business and processing temporal; point navigation takes two dates.
    Bitemporal,
    /// Business temporal; point navigation takes one date.
    BusinessTemporal,
    /// Processing temporal; point navigation takes one date.
    ProcessingTemporal,
}

impl Temporal {
    /// Number of explicit point-in-time arguments this stereotype requires.
    #[must_use]
    pub const fn arity(self) -> u8 {
        match self {
            Self::Bitemporal => 2,
            Self::BusinessTemporal | Self::ProcessingTemporal => 1,
        }
    }
}

/// A simple class property, including association-contributed navigation ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropInfo {
    name: Name,
    target: TypeRef,
    multiplicity: Multiplicity,
    association: Option<QName>,
}

impl PropInfo {
    pub(crate) const fn declared(name: Name, target: TypeRef, multiplicity: Multiplicity) -> Self {
        Self {
            name,
            target,
            multiplicity,
            association: None,
        }
    }

    pub(crate) fn with_association(mut self, association: QName) -> Self {
        self.association = Some(association);
        self
    }

    /// Property name.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Declared target type.
    #[must_use]
    pub const fn target(&self) -> &TypeRef {
        &self.target
    }

    /// Declared multiplicity.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    /// Whether the property was materialized from an association end.
    #[must_use]
    pub const fn from_assoc(&self) -> bool {
        self.association.is_some()
    }

    /// Association that contributed the property, if any.
    #[must_use]
    pub const fn association(&self) -> Option<&QName> {
        self.association.as_ref()
    }
}

/// Semantic classification of a qualified property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QpKind {
    /// User-defined qualified property; validate against its signature only.
    UserQualified,
    /// Engine-generated point-in-time navigation; temporal arity applies.
    MilestonedPoint,
    /// Generated all-versions navigation accepting no arguments.
    AllVersions,
    /// Generated range navigation accepting its two-date range signature.
    AllVersionsInRange,
    /// Generated unbounded edge navigation using target point arity.
    EdgePoint,
}

/// A normalized qualified property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QpInfo {
    name: Name,
    target: TypeRef,
    multiplicity: Multiplicity,
    kind: QpKind,
    signature: Option<Vec<TypeRef>>,
}

impl QpInfo {
    pub(crate) const fn new(
        name: Name,
        target: TypeRef,
        multiplicity: Multiplicity,
        kind: QpKind,
        signature: Option<Vec<TypeRef>>,
    ) -> Self {
        Self {
            name,
            target,
            multiplicity,
            kind,
            signature,
        }
    }

    /// Qualified-property name.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Return type.
    #[must_use]
    pub const fn target(&self) -> &TypeRef {
        &self.target
    }

    /// Return multiplicity.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    /// Milestoning/user classification used by the resolver.
    #[must_use]
    pub const fn kind(&self) -> QpKind {
        self.kind
    }

    /// Compiled argument types for a user property, or `None` when unavailable
    /// or when the property is generated.
    #[must_use]
    pub fn signature(&self) -> Option<&[TypeRef]> {
        self.signature.as_deref()
    }
}

/// Normalized facts for one Pure class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInfo {
    path: QName,
    supertypes: Vec<QName>,
    temporal: Option<Temporal>,
    properties: BTreeMap<Name, PropInfo>,
    qualified_properties: BTreeMap<Name, QpInfo>,
    provenance: Provenance,
    source: SourceId,
    coverage_gap: bool,
}

impl ClassInfo {
    pub(crate) const fn new(
        path: QName,
        supertypes: Vec<QName>,
        temporal: Option<Temporal>,
        properties: BTreeMap<Name, PropInfo>,
        qualified_properties: BTreeMap<Name, QpInfo>,
        source: SourceId,
    ) -> Self {
        Self {
            path,
            supertypes,
            temporal,
            properties,
            qualified_properties,
            provenance: Provenance::Pmcd,
            source,
            coverage_gap: false,
        }
    }

    pub(crate) const fn from_pure(
        path: QName,
        supertypes: Vec<QName>,
        temporal: Option<Temporal>,
        properties: BTreeMap<Name, PropInfo>,
        qualified_properties: BTreeMap<Name, QpInfo>,
        source: SourceId,
    ) -> Self {
        Self {
            path,
            supertypes,
            temporal,
            properties,
            qualified_properties,
            provenance: Provenance::PureFile,
            source,
            coverage_gap: false,
        }
    }

    pub(crate) fn mark_coverage_gap(&mut self) {
        self.coverage_gap = true;
    }

    pub(crate) fn properties_mut(&mut self) -> &mut BTreeMap<Name, PropInfo> {
        &mut self.properties
    }

    /// Fully-qualified class path.
    #[must_use]
    pub const fn path(&self) -> &QName {
        &self.path
    }

    /// Declared supertypes in source order.
    #[must_use]
    pub fn supertypes(&self) -> &[QName] {
        &self.supertypes
    }

    /// This class's own temporal stereotype; inherited resolution is separate.
    #[must_use]
    pub const fn temporal(&self) -> Option<Temporal> {
        self.temporal
    }

    /// Simple and association-contributed properties in lexical name order.
    #[must_use]
    pub const fn properties(&self) -> &BTreeMap<Name, PropInfo> {
        &self.properties
    }

    /// Qualified properties in lexical name order.
    #[must_use]
    pub const fn qualified_properties(&self) -> &BTreeMap<Name, QpInfo> {
        &self.qualified_properties
    }

    /// Source-kind policy for closed-world/open-world resolution.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Source that supplied the winning class definition.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// Whether this class has facts the source could not confirm.
    #[must_use]
    pub const fn coverage_gap(&self) -> bool {
        self.coverage_gap
    }
}

/// One directed, materialized end of an association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociationEndInfo {
    owner: QName,
    property: PropInfo,
}

impl AssociationEndInfo {
    pub(crate) const fn new(owner: QName, property: PropInfo) -> Self {
        Self { owner, property }
    }

    /// Class from which this end is navigable.
    #[must_use]
    pub const fn owner(&self) -> &QName {
        &self.owner
    }

    /// Property materialized on the owning class.
    #[must_use]
    pub const fn property(&self) -> &PropInfo {
        &self.property
    }
}

/// A normalized association and both directed ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssocInfo {
    path: QName,
    end_a: AssociationEndInfo,
    end_b: AssociationEndInfo,
    temporal: Option<Temporal>,
    provenance: Provenance,
    source: SourceId,
}

impl AssocInfo {
    pub(crate) const fn from_source(
        path: QName,
        end_a: AssociationEndInfo,
        end_b: AssociationEndInfo,
        temporal: Option<Temporal>,
        provenance: Provenance,
        source: SourceId,
    ) -> Self {
        Self {
            path,
            end_a,
            end_b,
            temporal,
            provenance,
            source,
        }
    }

    /// Fully-qualified association path.
    #[must_use]
    pub const fn path(&self) -> &QName {
        &self.path
    }

    /// First end in source declaration order, materialized on the opposite class.
    #[must_use]
    pub const fn end_a(&self) -> &AssociationEndInfo {
        &self.end_a
    }

    /// Second end in source declaration order, materialized on the opposite class.
    #[must_use]
    pub const fn end_b(&self) -> &AssociationEndInfo {
        &self.end_b
    }

    /// The association's own temporal stereotype.
    #[must_use]
    pub const fn temporal(&self) -> Option<Temporal> {
        self.temporal
    }

    /// Source-kind policy for this association.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Source that supplied the winning association definition.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }
}

/// Deterministic normalized model consumed by resolver and analysis passes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelGraph {
    pub(crate) classes: BTreeMap<QName, ClassInfo>,
    pub(crate) by_path: BTreeMap<QName, ClassId>,
    pub(crate) paths_by_id: Vec<QName>,
    pub(crate) associations: Vec<AssocInfo>,
    pub(crate) sources: Vec<ModelSourceInfo>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl ModelGraph {
    /// Classes keyed lexically by fully-qualified path.
    #[must_use]
    pub const fn classes(&self) -> &BTreeMap<QName, ClassInfo> {
        &self.classes
    }

    /// Look up a class by fully-qualified path.
    #[must_use]
    pub fn class(&self, path: &str) -> Option<&ClassInfo> {
        self.classes.get(path)
    }

    /// Deterministic ID for a fully-qualified class path.
    #[must_use]
    pub fn class_id(&self, path: &str) -> Option<ClassId> {
        self.by_path.get(path).copied()
    }

    /// Look up a class by deterministic ID.
    #[must_use]
    pub fn class_by_id(&self, id: ClassId) -> Option<&ClassInfo> {
        let index = usize::try_from(id.index()).ok()?;
        let path = self.paths_by_id.get(index)?;
        self.classes.get(path)
    }

    /// Associations sorted lexically by fully-qualified path.
    #[must_use]
    pub fn associations(&self) -> &[AssocInfo] {
        &self.associations
    }

    /// Source metadata in caller-supplied order.
    #[must_use]
    pub fn sources(&self) -> &[ModelSourceInfo] {
        &self.sources
    }

    /// Model-loading findings, including deterministic `PUR9000` merge warnings.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_validate_only_structural_invariants() {
        assert_eq!(
            Name::new("price in USD").expect("valid").as_str(),
            "price in USD"
        );
        assert!(matches!(Name::new(""), Err(NameError::Empty)));
        assert!(matches!(QName::new(""), Err(NameError::Empty)));
        assert!(matches!(
            Name::new("line\nbreak"),
            Err(NameError::ControlCharacter(_))
        ));
        assert!(matches!(
            QName::new("model::line\nbreak"),
            Err(NameError::ControlCharacter(_))
        ));
        assert!(matches!(
            Name::new("a::b"),
            Err(NameError::QualifiedSimpleName(_))
        ));
        assert!(matches!(
            QName::new("a::::b"),
            Err(NameError::EmptyPathSegment(_))
        ));
    }

    #[test]
    fn qname_parts_are_stable() {
        let path = QName::new("model::trade::Trade").expect("valid");
        assert_eq!(path.package(), Some("model::trade"));
        assert_eq!(path.simple_name(), "Trade");
        assert_eq!(QName::new("Integer").expect("valid").package(), None);
    }

    #[test]
    fn multiplicity_enforces_finite_bounds() {
        let optional = Multiplicity::new(0, Some(1)).expect("valid");
        assert_eq!(optional.upper(), Some(1));
        assert!(optional.is_to_one());
        assert!(Multiplicity::new(0, Some(0)).expect("valid").is_to_one());
        assert!(!Multiplicity::new(0, Some(2)).expect("valid").is_to_one());
        assert!(Multiplicity::new(2, Some(1)).is_err());
        let unbounded = Multiplicity::new(1, None).expect("valid");
        assert!(unbounded.is_unbounded());
        assert!(!unbounded.is_to_one());
    }

    #[test]
    fn temporal_arity_matches_design() {
        assert_eq!(Temporal::Bitemporal.arity(), 2);
        assert_eq!(Temporal::BusinessTemporal.arity(), 1);
        assert_eq!(Temporal::ProcessingTemporal.arity(), 1);
    }

    #[test]
    fn model_source_reports_path_and_provenance() {
        let pmcd = ModelSource::PmcdJson(PathBuf::from("model.json"));
        let pure = ModelSource::PureModelFile(PathBuf::from("model.pure"));
        assert_eq!(pmcd.path(), Path::new("model.json"));
        assert_eq!(pmcd.provenance(), Provenance::Pmcd);
        assert_eq!(pure.path(), Path::new("model.pure"));
        assert_eq!(pure.provenance(), Provenance::PureFile);
    }

    #[test]
    fn identifiers_have_unambiguous_display_forms() {
        assert_eq!(ClassId::new(7).to_string(), "class#7");
        assert_eq!(SourceId::new(11).to_string(), "source#11");
    }

    #[test]
    fn property_association_origin_is_observable() {
        let declared = PropInfo::declared(
            Name::new("orders").expect("valid"),
            TypeRef::new(QName::new("model::Order").expect("valid"), Vec::new()),
            Multiplicity::new(0, None).expect("valid"),
        );
        assert!(!declared.from_assoc());
        let contributed = declared.with_association(QName::new("model::Links").expect("valid"));
        assert!(contributed.from_assoc());
    }

    #[test]
    fn coverage_gap_accessor_preserves_open_world_state() {
        let class = ClassInfo {
            path: QName::new("model::Partial").expect("valid"),
            supertypes: Vec::new(),
            temporal: None,
            properties: BTreeMap::new(),
            qualified_properties: BTreeMap::new(),
            provenance: Provenance::PureFile,
            source: SourceId::new(0),
            coverage_gap: true,
        };
        assert!(class.coverage_gap());
    }
}
