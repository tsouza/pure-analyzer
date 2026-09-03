use std::collections::BTreeMap;
use std::path::PathBuf;

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, Label, Severity, TextRange, TextSize};
use serde_json::Value;

use crate::error::{ModelError, ModelErrorKind};
use crate::raw::{
    RawAssociation, RawClass, RawGenericType, RawMultiplicity, RawProperty, RawQualifiedProperty,
    RawStereotype,
};
use crate::stereotypes::{
    BITEMPORAL, BUSINESS_TEMPORAL, GENERATED_MILESTONING_PROPERTY, MILESTONING_PROFILE,
    MILESTONING_PROFILE_PROTOCOL, PROCESSING_TEMPORAL, TEMPORAL_PROFILE, TEMPORAL_PROFILE_PROTOCOL,
    classify_qualified_property,
};
use crate::{
    AssocInfo, AssociationEndInfo, ClassId, ClassInfo, MODEL_MERGE_CONFLICT, ModelGraph,
    ModelSource, ModelSourceInfo, Multiplicity, Name, PropInfo, Provenance, QName, QpInfo, QpKind,
    SourceId, Temporal, TypeRef,
};

const DOCUMENT_TYPE: &str = "data";
const CLASS_TYPE: &str = "class";
const ASSOCIATION_TYPE: &str = "association";

/// Borrowed in-memory PMCD JSON with a stable diagnostic label.
#[derive(Debug, Clone, Copy)]
pub struct PmcdDocument<'a> {
    label: &'a str,
    json: &'a str,
}

impl<'a> PmcdDocument<'a> {
    /// Construct an in-memory PMCD input.
    #[must_use]
    pub const fn new(label: &'a str, json: &'a str) -> Self {
        Self { label, json }
    }

    /// Source label used in errors and merge diagnostics.
    #[must_use]
    pub const fn label(self) -> &'a str {
        self.label
    }

    /// Borrow the PMCD JSON text.
    #[must_use]
    pub const fn json(self) -> &'a str {
        self.json
    }
}

/// Borrowed in-memory Pure Domain source with a stable diagnostic label.
#[derive(Debug, Clone, Copy)]
pub struct PureDocument<'a> {
    label: &'a str,
    source: &'a str,
}

impl<'a> PureDocument<'a> {
    /// Construct an in-memory Pure Domain input.
    #[must_use]
    pub const fn new(label: &'a str, source: &'a str) -> Self {
        Self { label, source }
    }

    /// Source label used in parser and merge diagnostics.
    #[must_use]
    pub const fn label(self) -> &'a str {
        self.label
    }

    /// Borrow the Pure Domain source text.
    #[must_use]
    pub const fn source(self) -> &'a str {
        self.source
    }
}

/// One borrowed model document in a mixed loading operation.
#[derive(Debug, Clone, Copy)]
pub enum ModelDocument<'a> {
    /// Engine-produced PMCD JSON.
    Pmcd(PmcdDocument<'a>),
    /// Engine-free Pure Domain source.
    Pure(PureDocument<'a>),
}

impl<'a> From<PmcdDocument<'a>> for ModelDocument<'a> {
    fn from(document: PmcdDocument<'a>) -> Self {
        Self::Pmcd(document)
    }
}

impl<'a> From<PureDocument<'a>> for ModelDocument<'a> {
    fn from(document: PureDocument<'a>) -> Self {
        Self::Pure(document)
    }
}

/// Load and merge heterogeneous model files in caller-supplied order.
///
/// Later class or association definitions with the same packageable path win,
/// independent of whether they came from PMCD or Pure source. Each replacement
/// adds a `PUR9000` warning to [`ModelGraph::diagnostics`].
///
/// # Errors
///
/// Returns [`ModelError`] for I/O, PMCD validation, parser infrastructure, or
/// a merged-graph invariant violation.
pub fn load_model_files(sources: &[ModelSource]) -> Result<ModelGraph, ModelError> {
    let mut merger = ModelMerger::default();
    for (index, model_source) in sources.iter().enumerate() {
        let source = source_id(index)?;
        let path = model_source.path();
        let text = std::fs::read_to_string(path).map_err(|error| ModelError::Read {
            path: path.to_path_buf(),
            source: error,
        })?;
        let label = path.display().to_string();
        match model_source {
            ModelSource::PmcdJson(_) => merger.ingest_pmcd(source, label, &text)?,
            ModelSource::PureModelFile(_) => merger.ingest_pure(source, label, &text)?,
        }
    }
    merger.finish()
}

/// Load and merge PMCD files in caller-supplied order.
///
/// This compatibility wrapper is semantically identical to passing
/// [`ModelSource::PmcdJson`] values to [`load_model_files`].
///
/// # Errors
///
/// Returns [`ModelError`] for I/O, malformed JSON, an invalid class or
/// association, or a merged-graph invariant violation.
pub fn load_pmcd_files(paths: &[PathBuf]) -> Result<ModelGraph, ModelError> {
    let sources = paths
        .iter()
        .cloned()
        .map(ModelSource::PmcdJson)
        .collect::<Vec<_>>();
    load_model_files(&sources)
}

/// Load and merge Pure Domain files in caller-supplied order.
///
/// Pure parsing is resilient: confirmed facts are retained while per-class
/// coverage gaps preserve open-world resolution where the source is incomplete
/// or contains an incomplete association.
///
/// # Errors
///
/// Returns [`ModelError`] for I/O, parser infrastructure, or a merged-graph
/// invariant violation.
pub fn load_pure_files(paths: &[PathBuf]) -> Result<ModelGraph, ModelError> {
    let sources = paths
        .iter()
        .cloned()
        .map(ModelSource::PureModelFile)
        .collect::<Vec<_>>();
    load_model_files(&sources)
}

/// Load and merge borrowed mixed model documents in caller-supplied order.
///
/// This is suitable for LSP hosts, tests, and callers that already own source
/// text. The loader itself performs no network access.
///
/// # Errors
///
/// Returns [`ModelError`] for malformed PMCD, parser infrastructure, or a
/// merged-graph invariant violation.
pub fn load_model_documents(documents: &[ModelDocument<'_>]) -> Result<ModelGraph, ModelError> {
    let mut merger = ModelMerger::default();
    for (index, document) in documents.iter().copied().enumerate() {
        let source = source_id(index)?;
        match document {
            ModelDocument::Pmcd(document) => {
                merger.ingest_pmcd(source, document.label.to_owned(), document.json)?;
            }
            ModelDocument::Pure(document) => {
                merger.ingest_pure(source, document.label.to_owned(), document.source)?;
            }
        }
    }
    merger.finish()
}

/// Load and merge borrowed PMCD documents in caller-supplied order.
///
/// This compatibility wrapper is semantically identical to passing
/// [`ModelDocument::Pmcd`] values to [`load_model_documents`].
///
/// # Errors
///
/// Returns [`ModelError`] for malformed JSON, an invalid class or association,
/// or a merged-graph invariant violation.
pub fn load_pmcd_documents(documents: &[PmcdDocument<'_>]) -> Result<ModelGraph, ModelError> {
    let documents = documents
        .iter()
        .copied()
        .map(ModelDocument::from)
        .collect::<Vec<_>>();
    load_model_documents(&documents)
}

/// Load and merge borrowed Pure Domain documents in caller-supplied order.
///
/// # Errors
///
/// Returns [`ModelError`] for parser infrastructure or a merged-graph
/// invariant violation.
pub fn load_pure_documents(documents: &[PureDocument<'_>]) -> Result<ModelGraph, ModelError> {
    let documents = documents
        .iter()
        .copied()
        .map(ModelDocument::from)
        .collect::<Vec<_>>();
    load_model_documents(&documents)
}

fn source_id(index: usize) -> Result<SourceId, ModelError> {
    u32::try_from(index)
        .map(SourceId::new)
        .map_err(|_| ModelError::TooManySources { index })
}

#[derive(Debug)]
pub(super) enum FragmentElement {
    Class(ClassInfo),
    Association(AssocDraft),
}

impl FragmentElement {
    pub(super) const fn source(&self) -> SourceId {
        match self {
            Self::Class(class) => class.source(),
            Self::Association(association) => association.source,
        }
    }
}

#[derive(Debug)]
pub(super) struct AssocDraft {
    pub(super) path: QName,
    pub(super) first: PropInfo,
    pub(super) second: PropInfo,
    pub(super) temporal: Option<Temporal>,
    pub(super) provenance: Provenance,
    pub(super) source: SourceId,
    pub(super) declaration_span: Option<TextRange>,
}

#[derive(Debug)]
pub(super) struct ModelFragment {
    pub(super) elements: BTreeMap<QName, FragmentElement>,
    pub(super) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
struct ModelMerger {
    elements: BTreeMap<QName, FragmentElement>,
    sources: Vec<ModelSourceInfo>,
    diagnostics: Vec<Diagnostic>,
}

impl ModelMerger {
    fn ingest_pmcd(
        &mut self,
        source: SourceId,
        label: String,
        json: &str,
    ) -> Result<(), ModelError> {
        let fragment = ModelFragment {
            elements: parse_document(&label, json, source)?,
            diagnostics: Vec::new(),
        };
        self.ingest_fragment(source, label, Provenance::Pmcd, fragment);
        Ok(())
    }

    fn ingest_pure(
        &mut self,
        source: SourceId,
        label: String,
        text: &str,
    ) -> Result<(), ModelError> {
        let fragment = crate::pure::parse_pure_document(&label, text, source)?;
        self.ingest_fragment(source, label, Provenance::PureFile, fragment);
        Ok(())
    }

    /// Merges one source's already-lowered elements into the graph.
    ///
    /// Each element's `coverage_gap` was decided by its own source alone
    /// (see [`crate::pure::parse_pure_document`]): an open-world class from
    /// one source must never contaminate an unrelated, already-ingested or
    /// later-ingested element from a different source (issue #267).
    fn ingest_fragment(
        &mut self,
        source: SourceId,
        label: String,
        provenance: Provenance,
        fragment: ModelFragment,
    ) {
        let ModelFragment {
            elements,
            diagnostics,
        } = fragment;
        self.sources
            .push(ModelSourceInfo::new(source, label.clone(), provenance));
        self.diagnostics.extend(diagnostics);
        for (path, replacement) in elements {
            if let Some(previous) = self.elements.insert(path.clone(), replacement) {
                let diagnostic = self.merge_diagnostic(&path, previous.source(), source, &label);
                self.diagnostics.push(diagnostic);
            }
        }
    }

    fn merge_diagnostic(
        &self,
        path: &QName,
        previous: SourceId,
        replacement: SourceId,
        replacement_label: &str,
    ) -> Diagnostic {
        let previous_label = self
            .sources
            .get(previous.index() as usize)
            .map_or("<unknown source>", ModelSourceInfo::label);
        let empty = TextRange::empty(TextSize::new(0));
        Diagnostic::builder(
            MODEL_MERGE_CONFLICT,
            Severity::Warning,
            format!(
                "model element `{path}` from `{replacement_label}` replaces the definition from `{previous_label}`"
            ),
            Label::with_note(
                replacement.file_id(),
                empty,
                format!("winning definition from `{replacement_label}`"),
            ),
        )
        .secondary(Label::with_note(
            previous.file_id(),
            empty,
            format!("replaced definition from `{previous_label}`"),
        ))
        .build()
    }

    fn finish(mut self) -> Result<ModelGraph, ModelError> {
        let mut classes = BTreeMap::new();
        let mut associations = Vec::new();
        for (path, element) in self.elements {
            match element {
                FragmentElement::Class(class) => {
                    classes.insert(path, class);
                }
                FragmentElement::Association(association) => associations.push(association),
            }
        }
        let AssociationMaterialization {
            associations,
            diagnostics,
            coverage_gap,
        } = materialize_associations(&mut classes, associations)?;
        if coverage_gap {
            for class in classes.values_mut() {
                class.mark_coverage_gap();
            }
        }
        self.diagnostics.extend(diagnostics);
        let (by_path, paths_by_id) = index_classes(&classes)?;
        Ok(ModelGraph {
            classes,
            by_path,
            paths_by_id,
            associations,
            sources: self.sources,
            diagnostics: self.diagnostics,
        })
    }
}

fn parse_document(
    source_name: &str,
    json: &str,
    source: SourceId,
) -> Result<BTreeMap<QName, FragmentElement>, ModelError> {
    let document: Value = serde_json::from_str(json).map_err(|error| ModelError::Json {
        source_name: source_name.to_owned(),
        source: error,
    })?;
    let object = document
        .as_object()
        .ok_or_else(|| ModelError::InvalidDocument {
            source_name: source_name.to_owned(),
            message: "top level must be an object".to_owned(),
        })?;
    validate_document_type(source_name, object.get("_type"))?;
    let elements = object
        .get("elements")
        .and_then(Value::as_array)
        .ok_or_else(|| ModelError::InvalidDocument {
            source_name: source_name.to_owned(),
            message: "`elements` must be an array".to_owned(),
        })?;
    lower_elements(source_name, elements, source)
}

fn validate_document_type(source_name: &str, value: Option<&Value>) -> Result<(), ModelError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.as_str() == Some(DOCUMENT_TYPE) {
        return Ok(());
    }
    Err(ModelError::InvalidDocument {
        source_name: source_name.to_owned(),
        message: "`_type`, when present, must be `data`".to_owned(),
    })
}

fn lower_elements(
    source_name: &str,
    elements: &[Value],
    source: SourceId,
) -> Result<BTreeMap<QName, FragmentElement>, ModelError> {
    let mut lowered = BTreeMap::new();
    for (index, element) in elements.iter().enumerate() {
        let kind = element_kind(source_name, index, element)?;
        let normalized = match kind {
            CLASS_TYPE => Some(lower_class_element(source_name, index, element, source)?),
            ASSOCIATION_TYPE => Some(lower_association_element(
                source_name,
                index,
                element,
                source,
            )?),
            _ => None,
        };
        if let Some((path, normalized)) = normalized
            && lowered.insert(path.clone(), normalized).is_some()
        {
            return Err(invalid_element(
                source_name,
                index,
                kind,
                ModelErrorKind::DuplicateElement { path },
            ));
        }
    }
    Ok(lowered)
}

fn element_kind<'a>(
    source_name: &str,
    index: usize,
    element: &'a Value,
) -> Result<&'a str, ModelError> {
    let Some(object) = element.as_object() else {
        return Err(invalid_element(
            source_name,
            index,
            "unknown",
            ModelErrorKind::InvalidRecord("element must be an object".to_owned()),
        ));
    };
    object.get("_type").and_then(Value::as_str).ok_or_else(|| {
        invalid_element(
            source_name,
            index,
            "unknown",
            ModelErrorKind::InvalidRecord("`_type` must be a string".to_owned()),
        )
    })
}

fn lower_class_element(
    source_name: &str,
    index: usize,
    value: &Value,
    source: SourceId,
) -> Result<(QName, FragmentElement), ModelError> {
    let raw: RawClass = serde_json::from_value(value.clone()).map_err(|error| {
        invalid_element(
            source_name,
            index,
            CLASS_TYPE,
            ModelErrorKind::InvalidRecord(error.to_string()),
        )
    })?;
    let class = lower_class(raw, source)
        .map_err(|kind| invalid_element(source_name, index, CLASS_TYPE, kind))?;
    Ok((class.path().clone(), FragmentElement::Class(class)))
}

fn lower_association_element(
    source_name: &str,
    index: usize,
    value: &Value,
    source: SourceId,
) -> Result<(QName, FragmentElement), ModelError> {
    let raw: RawAssociation = serde_json::from_value(value.clone()).map_err(|error| {
        invalid_element(
            source_name,
            index,
            ASSOCIATION_TYPE,
            ModelErrorKind::InvalidRecord(error.to_string()),
        )
    })?;
    let association = lower_association(raw, source)
        .map_err(|kind| invalid_element(source_name, index, ASSOCIATION_TYPE, kind))?;
    Ok((
        association.path.clone(),
        FragmentElement::Association(association),
    ))
}

fn invalid_element(
    source_name: &str,
    element_index: usize,
    element_kind: &str,
    kind: ModelErrorKind,
) -> ModelError {
    ModelError::InvalidElement {
        source_name: source_name.to_owned(),
        element_index,
        element_kind: element_kind.to_owned(),
        kind: Box::new(kind),
    }
}

fn lower_class(raw: RawClass, source: SourceId) -> Result<ClassInfo, ModelErrorKind> {
    let path = QName::from_package_and_name(&raw.package, &raw.name)?;
    let temporal = lower_temporal(&path, &raw.stereotypes)?;
    let supertypes = raw
        .super_types
        .into_iter()
        .map(|supertype| QName::new(supertype.into_string()).map_err(ModelErrorKind::from))
        .collect::<Result<Vec<_>, _>>()?;
    let properties = lower_properties(&path, raw.properties)?;
    let qualified_properties = lower_qualified_properties(&path, raw.qualified_properties)?;
    Ok(ClassInfo::new(
        path,
        supertypes,
        temporal,
        properties,
        qualified_properties,
        source,
    ))
}

fn lower_properties(
    class: &QName,
    properties: Vec<RawProperty>,
) -> Result<BTreeMap<Name, PropInfo>, ModelErrorKind> {
    let mut lowered = BTreeMap::new();
    for property in properties {
        let property = lower_property(property)?;
        let name = property.name().clone();
        if lowered.insert(name.clone(), property).is_some() {
            return Err(ModelErrorKind::DuplicateProperty {
                class: class.clone(),
                property: name,
            });
        }
    }
    Ok(lowered)
}

fn lower_property(raw: RawProperty) -> Result<PropInfo, ModelErrorKind> {
    let name = Name::new(raw.name)?;
    let target = lower_type_ref(raw.generic_type)?;
    let multiplicity = lower_multiplicity(raw.multiplicity)?;
    Ok(PropInfo::declared(name, target, multiplicity))
}

fn lower_qualified_properties(
    class: &QName,
    properties: Vec<RawQualifiedProperty>,
) -> Result<BTreeMap<Name, QpInfo>, ModelErrorKind> {
    let mut lowered = BTreeMap::new();
    for property in properties {
        let property = lower_qualified_property(property)?;
        let name = property.name().clone();
        if lowered.insert(name.clone(), property).is_some() {
            return Err(ModelErrorKind::DuplicateQualifiedProperty {
                class: class.clone(),
                property: name,
            });
        }
    }
    Ok(lowered)
}

fn lower_qualified_property(raw: RawQualifiedProperty) -> Result<QpInfo, ModelErrorKind> {
    let name = Name::new(raw.name)?;
    let target = lower_type_ref(raw.return_generic_type)?;
    let multiplicity = lower_multiplicity(raw.return_multiplicity)?;
    let generated = is_generated_milestoning_property(&raw.stereotypes);
    let kind = classify_qualified_property(&name, generated);
    let signature = if kind == QpKind::UserQualified {
        lower_signature(raw.parameters)?
    } else {
        None
    };
    Ok(QpInfo::new(name, target, multiplicity, kind, signature))
}

fn lower_signature(
    parameters: Option<Vec<crate::raw::RawParameter>>,
) -> Result<Option<Vec<TypeRef>>, ModelErrorKind> {
    parameters
        .map(|parameters| {
            parameters
                .into_iter()
                .map(|parameter| lower_type_ref(parameter.generic_type))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
}

fn is_generated_milestoning_property(stereotypes: &[RawStereotype]) -> bool {
    stereotypes.iter().any(|stereotype| {
        is_milestoning_profile(&stereotype.profile)
            && stereotype.value == GENERATED_MILESTONING_PROPERTY
    })
}

fn is_milestoning_profile(profile: &str) -> bool {
    profile == MILESTONING_PROFILE || profile == MILESTONING_PROFILE_PROTOCOL
}

fn lower_type_ref(raw: RawGenericType) -> Result<TypeRef, ModelErrorKind> {
    let raw_type = QName::new(raw.raw_type.into_string())?;
    let type_arguments = raw
        .type_arguments
        .into_iter()
        .map(lower_type_ref)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TypeRef::new(raw_type, type_arguments))
}

fn lower_multiplicity(raw: RawMultiplicity) -> Result<Multiplicity, ModelErrorKind> {
    Multiplicity::new(raw.lower_bound, raw.upper_bound).map_err(ModelErrorKind::from)
}

fn lower_temporal(
    element: &QName,
    stereotypes: &[RawStereotype],
) -> Result<Option<Temporal>, ModelErrorKind> {
    let mut temporal = None;
    for stereotype in stereotypes.iter().filter(|stereotype| {
        matches!(
            stereotype.profile.as_str(),
            TEMPORAL_PROFILE | TEMPORAL_PROFILE_PROTOCOL
        )
    }) {
        let current = match stereotype.value.as_str() {
            BITEMPORAL => Temporal::Bitemporal,
            BUSINESS_TEMPORAL => Temporal::BusinessTemporal,
            PROCESSING_TEMPORAL => Temporal::ProcessingTemporal,
            value => {
                return Err(ModelErrorKind::UnknownTemporalStereotype {
                    element: element.clone(),
                    value: value.to_owned(),
                });
            }
        };
        if temporal.replace(current).is_some() {
            return Err(ModelErrorKind::MultipleTemporalStereotypes {
                element: element.clone(),
            });
        }
    }
    Ok(temporal)
}

fn lower_association(raw: RawAssociation, source: SourceId) -> Result<AssocDraft, ModelErrorKind> {
    let path = QName::from_package_and_name(&raw.package, &raw.name)?;
    let temporal = lower_temporal(&path, &raw.stereotypes)?;
    let actual = raw.properties.len();
    let [first, second]: [RawProperty; 2] =
        raw.properties
            .try_into()
            .map_err(|_| ModelErrorKind::AssociationArity {
                association: path.clone(),
                actual,
            })?;
    Ok(AssocDraft {
        path,
        first: lower_property(first)?,
        second: lower_property(second)?,
        temporal,
        provenance: Provenance::Pmcd,
        source,
        declaration_span: None,
    })
}

struct AssociationMaterialization {
    associations: Vec<AssocInfo>,
    diagnostics: Vec<Diagnostic>,
    coverage_gap: bool,
}

struct PreparedAssociation {
    path: QName,
    first_owner: QName,
    second_owner: QName,
    first: PropInfo,
    second: PropInfo,
    temporal: Option<Temporal>,
    provenance: Provenance,
    source: SourceId,
    declaration_span: Option<TextRange>,
}

impl PreparedAssociation {
    fn from_draft(association: AssocDraft) -> Self {
        let AssocDraft {
            path,
            first,
            second,
            temporal,
            provenance,
            source,
            declaration_span,
        } = association;
        let first_owner = second.target().raw_type().clone();
        let second_owner = first.target().raw_type().clone();
        let first = first.with_association(path.clone());
        let second = second.with_association(path.clone());
        Self {
            path,
            first_owner,
            second_owner,
            first,
            second,
            temporal,
            provenance,
            source,
            declaration_span,
        }
    }

    fn ends(&self) -> [(&QName, &PropInfo); 2] {
        [
            (&self.first_owner, &self.first),
            (&self.second_owner, &self.second),
        ]
    }
}

fn materialize_associations(
    classes: &mut BTreeMap<QName, ClassInfo>,
    associations: Vec<AssocDraft>,
) -> Result<AssociationMaterialization, ModelError> {
    let prepared = associations
        .into_iter()
        .map(PreparedAssociation::from_draft)
        .collect::<Vec<_>>();
    let failures = preflight_association_failures(classes, &prepared);

    for (association, failure) in prepared.iter().zip(&failures) {
        if association.provenance == Provenance::Pmcd
            && let Some(failure) = failure
        {
            return Err(merged_graph_error(association.source, failure.clone()));
        }
    }

    let mut materialized = Vec::with_capacity(prepared.len());
    let mut diagnostics = Vec::new();
    let mut coverage_gap = false;
    for (association, failure) in prepared.into_iter().zip(failures) {
        if let Some(failure) = failure {
            coverage_gap = true;
            diagnostics.push(pure_association_materialization_diagnostic(
                &association,
                &failure,
            ));
            continue;
        }
        materialized.push(materialize_prepared_association(classes, association)?);
    }

    Ok(AssociationMaterialization {
        associations: materialized,
        diagnostics,
        coverage_gap,
    })
}

fn preflight_association_failures(
    classes: &BTreeMap<QName, ClassInfo>,
    associations: &[PreparedAssociation],
) -> Vec<Option<ModelErrorKind>> {
    let mut failures = vec![None; associations.len()];
    for (index, association) in associations.iter().enumerate() {
        for (owner, property) in association.ends() {
            let property_name = property.name().clone();
            let failure = match classes.get(owner) {
                None => Some(ModelErrorKind::MissingAssociationOwner {
                    association: association.path.clone(),
                    property: property_name,
                    owner: owner.clone(),
                }),
                Some(class) if class.properties().contains_key(&property_name) => {
                    Some(ModelErrorKind::AssociationPropertyConflict {
                        association: association.path.clone(),
                        owner: owner.clone(),
                        property: property_name,
                    })
                }
                Some(_) => None,
            };
            if let Some(failure) = failure {
                let _ = failures[index].get_or_insert(failure);
            }
        }
    }

    let mut association_ends = BTreeMap::<(QName, Name), Vec<usize>>::new();
    for (index, association) in associations.iter().enumerate() {
        if failures[index].is_some() {
            continue;
        }
        for (owner, property) in association.ends() {
            association_ends
                .entry((owner.clone(), property.name().clone()))
                .or_default()
                .push(index);
        }
    }
    for ((owner, property), contributors) in association_ends {
        if contributors.len() < 2 {
            continue;
        }
        let pmcd_contributor_count = contributors
            .iter()
            .copied()
            .filter(|&index| associations[index].provenance == Provenance::Pmcd)
            .count();
        for index in contributors {
            // One closed-world PMCD end wins over open-world Pure candidates;
            // two PMCD ends remain an invalid closed-world graph.
            if pmcd_contributor_count == 1 && associations[index].provenance == Provenance::Pmcd {
                continue;
            }
            let _ = failures[index].get_or_insert_with(|| {
                ModelErrorKind::AssociationPropertyConflict {
                    association: associations[index].path.clone(),
                    owner: owner.clone(),
                    property: property.clone(),
                }
            });
        }
    }
    failures
}

fn materialize_prepared_association(
    classes: &mut BTreeMap<QName, ClassInfo>,
    association: PreparedAssociation,
) -> Result<AssocInfo, ModelError> {
    let declaration_span = association.declaration_span;
    let path = association.path.clone();
    insert_association_end(
        classes,
        association.source,
        &association.first_owner,
        association.first.clone(),
        &path,
    )?;
    insert_association_end(
        classes,
        association.source,
        &association.second_owner,
        association.second.clone(),
        &path,
    )?;
    let materialized = AssocInfo::from_source(
        association.path,
        AssociationEndInfo::new(association.first_owner, association.first),
        AssociationEndInfo::new(association.second_owner, association.second),
        association.temporal,
        association.provenance,
        association.source,
    );
    Ok(if let Some(span) = declaration_span {
        materialized.with_declaration_span(span)
    } else {
        materialized
    })
}

fn pure_association_materialization_diagnostic(
    association: &PreparedAssociation,
    failure: &ModelErrorKind,
) -> Diagnostic {
    let span = association
        .declaration_span
        .unwrap_or_else(|| TextRange::empty(TextSize::new(0)));
    Diagnostic::builder(
        DiagCode::UnresolvedModelAssociation,
        Severity::Error,
        format!(
            "Pure association `{}` cannot be materialized safely: {failure}",
            association.path
        ),
        Label::with_note(
            association.source.file_id(),
            span,
            "association not materialized",
        ),
    )
    .build()
}

fn insert_association_end(
    classes: &mut BTreeMap<QName, ClassInfo>,
    source: SourceId,
    owner: &QName,
    property: PropInfo,
    association: &QName,
) -> Result<(), ModelError> {
    let property_name = property.name().clone();
    let Some(class) = classes.get_mut(owner) else {
        return Err(merged_graph_error(
            source,
            ModelErrorKind::MissingAssociationOwner {
                association: association.clone(),
                property: property_name,
                owner: owner.clone(),
            },
        ));
    };
    if class
        .properties_mut()
        .insert(property_name.clone(), property)
        .is_some()
    {
        return Err(merged_graph_error(
            source,
            ModelErrorKind::AssociationPropertyConflict {
                association: association.clone(),
                owner: owner.clone(),
                property: property_name,
            },
        ));
    }
    Ok(())
}

fn merged_graph_error(source_id: SourceId, kind: ModelErrorKind) -> ModelError {
    ModelError::InvalidMergedGraph { source_id, kind }
}

fn index_classes(
    classes: &BTreeMap<QName, ClassInfo>,
) -> Result<(BTreeMap<QName, ClassId>, Vec<QName>), ModelError> {
    let mut by_path = BTreeMap::new();
    let mut paths_by_id = Vec::with_capacity(classes.len());
    for (index, path) in classes.keys().enumerate() {
        let raw = u32::try_from(index).map_err(|_| ModelError::TooManyClasses { index })?;
        by_path.insert(path.clone(), ClassId::new(raw));
        paths_by_id.push(path.clone());
    }
    Ok((by_path, paths_by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_STEREOTYPE: RawStereotype = RawStereotype {
        profile: String::new(),
        value: String::new(),
    };

    #[test]
    fn generated_milestoning_stereotype_detection_matches_profile_and_value() {
        // The name/multiplicity-driven classification precedence itself is
        // covered once, in `crate::stereotypes`, shared by both loaders.
        // This test covers only what is specific to the PMCD stereotype-list
        // shape: which profile spellings and stereotype values count as the
        // engine-asserted "generated" fact.
        let generated = [RawStereotype {
            profile: MILESTONING_PROFILE.to_owned(),
            value: GENERATED_MILESTONING_PROPERTY.to_owned(),
        }];
        assert!(is_generated_milestoning_property(&generated));

        let short_generated = [RawStereotype {
            profile: MILESTONING_PROFILE_PROTOCOL.to_owned(),
            value: GENERATED_MILESTONING_PROPERTY.to_owned(),
        }];
        assert!(is_generated_milestoning_property(&short_generated));

        let other_milestoning = [RawStereotype {
            profile: MILESTONING_PROFILE.to_owned(),
            value: "notgenerated".to_owned(),
        }];
        assert!(!is_generated_milestoning_property(&other_milestoning));

        let generated_value_in_another_profile = [RawStereotype {
            profile: "example::profile".to_owned(),
            value: GENERATED_MILESTONING_PROPERTY.to_owned(),
        }];
        assert!(!is_generated_milestoning_property(
            &generated_value_in_another_profile
        ));

        assert!(!is_generated_milestoning_property(std::slice::from_ref(
            &USER_STEREOTYPE
        )));
        assert!(!is_generated_milestoning_property(&[]));
    }

    #[test]
    fn borrowed_documents_expose_their_exact_inputs() {
        let document = PmcdDocument::new("memory:model", "{\"elements\":[]}");
        assert_eq!(document.label(), "memory:model");
        assert_eq!(document.json(), "{\"elements\":[]}");

        let document = PureDocument::new("memory:model.pure", "Class demo::Input {}");
        assert_eq!(document.label(), "memory:model.pure");
        assert_eq!(document.source(), "Class demo::Input {}");
    }
}
