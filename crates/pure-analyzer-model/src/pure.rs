use std::collections::BTreeMap;

use pure_analyzer_diagnostics::{Diagnostic, TextRange};
use pure_analyzer_parser::{DomainCoverageGap, DomainCoverageGapKind, parse_domain};
use pure_analyzer_syntax::{GreenElement, GreenNode, GreenToken, SyntaxKind};

use crate::error::ModelError;
use crate::loader::{AssocDraft, FragmentElement, PureFragment};
use crate::{
    ClassInfo, Multiplicity, Name, PropInfo, Provenance, QName, QpInfo, QpKind, SourceId, Temporal,
    TypeRef,
};

const GENERATED_MILESTONING_PROPERTY: &str = "generatedmilestoningproperty";
const ALL_VERSIONS_SUFFIX: &str = "AllVersions";
const ALL_VERSIONS_IN_RANGE_SUFFIX: &str = "AllVersionsInRange";

/// Parses a resilient Domain source and lowers only its confirmed model facts.
pub(super) fn parse_pure_document(
    source_name: &str,
    text: &str,
    source: SourceId,
) -> Result<PureFragment, ModelError> {
    let parsed = parse_domain(text, source.file_id()).map_err(|error| ModelError::PureParse {
        source_name: source_name.to_owned(),
        source: error,
    })?;
    let elements = lower_domain(
        &parsed.green,
        &parsed.coverage_gaps,
        &parsed.diagnostics,
        source,
    );
    Ok(PureFragment {
        elements,
        diagnostics: parsed.diagnostics,
    })
}

fn lower_domain(
    root: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
    source: SourceId,
) -> BTreeMap<QName, FragmentElement> {
    let Some(file) = find_domain_file(root) else {
        return BTreeMap::new();
    };
    let classes = direct_nodes(file, SyntaxKind::DOMAIN_CLASS_DECL).collect::<Vec<_>>();
    let associations = direct_nodes(file, SyntaxKind::DOMAIN_ASSOCIATION_DECL).collect::<Vec<_>>();

    let mut source_wide_gap = gaps
        .iter()
        .any(|gap| gap.kind == DomainCoverageGapKind::UnsupportedTopLevel);
    let mut association_entries = Vec::new();
    for association in associations {
        let lowered = lower_association(association, gaps, diagnostics, source);
        source_wide_gap |= lowered.uncertain;
        if let Some(association) = lowered.value {
            association_entries.push(association);
        }
    }

    let mut class_entries = Vec::new();
    for class in classes {
        match lower_class(class, gaps, diagnostics, source) {
            Some(class) => class_entries.push(class),
            None => source_wide_gap = true,
        }
    }
    source_wide_gap |= has_duplicate_paths(&class_entries, &association_entries);

    let mut elements = BTreeMap::new();
    for mut class in class_entries {
        if source_wide_gap {
            class.mark_coverage_gap();
        }
        let path = class.path().clone();
        elements.insert(path, FragmentElement::Class(class));
    }
    for association in association_entries {
        elements.insert(
            association.path.clone(),
            FragmentElement::Association(association),
        );
    }
    elements
}

fn has_duplicate_paths(classes: &[ClassInfo], associations: &[AssocDraft]) -> bool {
    let mut paths = BTreeMap::new();
    for class in classes {
        if paths.insert(class.path().clone(), ()).is_some() {
            return true;
        }
    }
    for association in associations {
        if paths.insert(association.path.clone(), ()).is_some() {
            return true;
        }
    }
    false
}

fn lower_class(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
    source: SourceId,
) -> Option<ClassInfo> {
    let (path, name_index) = declaration_path(node)?;
    let annotations = annotations_before(node, name_index);
    let (supertypes, supertype_gap) = lower_supertypes(node, gaps, diagnostics);
    let (properties, qualified_properties, member_gap) =
        lower_class_members(node, name_index, gaps, diagnostics);
    let coverage_gap = node_has_coverage_gap(node, gaps, diagnostics)
        || annotations.temporal_uncertain
        || supertype_gap
        || member_gap;
    let mut class = ClassInfo::from_pure(
        path,
        supertypes,
        annotations.temporal,
        properties,
        qualified_properties,
        source,
    );
    if coverage_gap {
        class.mark_coverage_gap();
    }
    Some(class)
}

fn lower_supertypes(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> (Vec<QName>, bool) {
    let Some(extends) = direct_nodes(node, SyntaxKind::DOMAIN_EXTENDS_CLAUSE).next() else {
        return (Vec::new(), false);
    };
    if node_has_coverage_gap(extends, gaps, diagnostics) {
        return (Vec::new(), true);
    }
    let mut supertypes = Vec::new();
    for name in direct_nodes(extends, SyntaxKind::DOMAIN_QUALIFIED_NAME) {
        let Some(name) = qname_from_node(name) else {
            return (Vec::new(), true);
        };
        supertypes.push(name);
    }
    if supertypes.is_empty() {
        (Vec::new(), true)
    } else {
        (supertypes, false)
    }
}

fn lower_class_members(
    node: &GreenNode,
    name_index: usize,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> (BTreeMap<Name, PropInfo>, BTreeMap<Name, QpInfo>, bool) {
    let mut properties = BTreeMap::new();
    let mut qualified_properties = BTreeMap::new();
    let mut pending_annotations = AnnotationFacts::default();
    let mut coverage_gap = false;

    for element in node.children().iter().skip(name_index.saturating_add(1)) {
        let Some(member) = element.as_node() else {
            continue;
        };
        match member.kind() {
            SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS => {
                pending_annotations.merge(annotation_facts(member));
            }
            SyntaxKind::DOMAIN_PROPERTY_DECL => {
                coverage_gap |= !insert_property(&mut properties, member, gaps, diagnostics);
                pending_annotations = AnnotationFacts::default();
            }
            SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL => {
                coverage_gap |= !insert_qualified_property(
                    &mut qualified_properties,
                    member,
                    pending_annotations,
                    gaps,
                    diagnostics,
                );
                pending_annotations = AnnotationFacts::default();
            }
            SyntaxKind::DOMAIN_OPAQUE_NODE | SyntaxKind::ERROR_NODE => {
                coverage_gap = true;
                pending_annotations = AnnotationFacts::default();
            }
            _ => {}
        }
    }
    (properties, qualified_properties, coverage_gap)
}

fn insert_property(
    properties: &mut BTreeMap<Name, PropInfo>,
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> bool {
    if node_is_unconfirmed(node, gaps, diagnostics) {
        return false;
    }
    let Some(property) = lower_property(node) else {
        return false;
    };
    properties
        .insert(property.name().clone(), property)
        .is_none()
}

fn insert_qualified_property(
    properties: &mut BTreeMap<Name, QpInfo>,
    node: &GreenNode,
    annotations: AnnotationFacts,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> bool {
    if node_is_unconfirmed(node, gaps, diagnostics) {
        return false;
    }
    let Some(property) = lower_qualified_property(node, annotations, gaps, diagnostics) else {
        return false;
    };
    properties
        .insert(property.name().clone(), property)
        .is_none()
}

struct LoweredAssociation {
    value: Option<AssocDraft>,
    uncertain: bool,
}

fn lower_association(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
    source: SourceId,
) -> LoweredAssociation {
    if node_has_coverage_gap(node, gaps, diagnostics) {
        return LoweredAssociation {
            value: None,
            uncertain: true,
        };
    }
    let Some((path, name_index)) = declaration_path(node) else {
        return LoweredAssociation {
            value: None,
            uncertain: true,
        };
    };
    let annotations = annotations_before(node, name_index);
    if annotations.temporal_uncertain {
        return LoweredAssociation {
            value: None,
            uncertain: true,
        };
    }
    let mut ends = Vec::new();
    for property in direct_nodes(node, SyntaxKind::DOMAIN_PROPERTY_DECL) {
        if node_is_unconfirmed(property, gaps, diagnostics) {
            return LoweredAssociation {
                value: None,
                uncertain: true,
            };
        }
        let Some(property) = lower_property(property) else {
            return LoweredAssociation {
                value: None,
                uncertain: true,
            };
        };
        ends.push(property);
    }
    let mut ends = ends.into_iter();
    let (Some(first), Some(second), None) = (ends.next(), ends.next(), ends.next()) else {
        return LoweredAssociation {
            value: None,
            uncertain: true,
        };
    };
    LoweredAssociation {
        value: Some(AssocDraft {
            path,
            first,
            second,
            temporal: annotations.temporal,
            provenance: Provenance::PureFile,
            source,
        }),
        uncertain: false,
    }
}

fn lower_property(node: &GreenNode) -> Option<PropInfo> {
    let name = direct_name(node)?;
    let target = direct_nodes(node, SyntaxKind::DOMAIN_TYPE_REF)
        .next()
        .and_then(type_from_node)?;
    let multiplicity = direct_nodes(node, SyntaxKind::DOMAIN_MULTIPLICITY)
        .next()
        .and_then(multiplicity_from_node)?;
    Some(PropInfo::declared(name, target, multiplicity))
}

fn lower_qualified_property(
    node: &GreenNode,
    annotations: AnnotationFacts,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> Option<QpInfo> {
    let name = direct_name(node)?;
    let target = direct_nodes(node, SyntaxKind::DOMAIN_TYPE_REF)
        .next()
        .and_then(type_from_node)?;
    let multiplicity = direct_nodes(node, SyntaxKind::DOMAIN_MULTIPLICITY)
        .next()
        .and_then(multiplicity_from_node)?;
    let kind = classify_pure_qualified_property(&name, multiplicity, annotations.generated);
    let signature = if kind == QpKind::UserQualified {
        Some(lower_signature(node, gaps, diagnostics)?)
    } else {
        None
    };
    Some(QpInfo::new(name, target, multiplicity, kind, signature))
}

fn lower_signature(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> Option<Vec<TypeRef>> {
    let mut signature = Vec::new();
    for parameter in direct_nodes(node, SyntaxKind::DOMAIN_PARAMETER_DECL) {
        if node_is_unconfirmed(parameter, gaps, diagnostics) {
            return None;
        }
        let ty = direct_nodes(parameter, SyntaxKind::DOMAIN_TYPE_REF)
            .next()
            .and_then(type_from_node)?;
        signature.push(ty);
    }
    Some(signature)
}

fn classify_pure_qualified_property(
    name: &Name,
    multiplicity: Multiplicity,
    generated: bool,
) -> QpKind {
    if generated && name.as_str().ends_with(ALL_VERSIONS_IN_RANGE_SUFFIX) {
        QpKind::AllVersionsInRange
    } else if generated && name.as_str().ends_with(ALL_VERSIONS_SUFFIX) {
        QpKind::AllVersions
    } else if generated && multiplicity.is_unbounded() {
        QpKind::EdgePoint
    } else if generated {
        QpKind::MilestonedPoint
    } else {
        QpKind::UserQualified
    }
}

fn declaration_path(node: &GreenNode) -> Option<(QName, usize)> {
    node.children()
        .iter()
        .enumerate()
        .find_map(|(index, element)| {
            let child = element.as_node()?;
            (child.kind() == SyntaxKind::DOMAIN_QUALIFIED_NAME)
                .then(|| qname_from_node(child).map(|path| (path, index)))
                .flatten()
        })
}

fn annotations_before(node: &GreenNode, end: usize) -> AnnotationFacts {
    node.children()
        .iter()
        .take(end)
        .filter_map(GreenElement::as_node)
        .filter(|child| child.kind() == SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS)
        .fold(AnnotationFacts::default(), |mut facts, child| {
            facts.merge(annotation_facts(child));
            facts
        })
}

#[derive(Clone, Copy, Default)]
struct AnnotationFacts {
    temporal: Option<Temporal>,
    temporal_uncertain: bool,
    generated: bool,
}

impl AnnotationFacts {
    fn merge(&mut self, next: Self) {
        self.generated |= next.generated;
        if next.temporal_uncertain {
            self.temporal = None;
            self.temporal_uncertain = true;
        }
        if let Some(temporal) = next.temporal {
            self.note_temporal(temporal);
        }
    }

    fn note_temporal(&mut self, temporal: Temporal) {
        if self.temporal.is_some() || self.temporal_uncertain {
            self.temporal = None;
            self.temporal_uncertain = true;
        } else {
            self.temporal = Some(temporal);
        }
    }

    fn mark_temporal_uncertain(&mut self) {
        self.temporal = None;
        self.temporal_uncertain = true;
    }
}

fn annotation_facts(node: &GreenNode) -> AnnotationFacts {
    let text = compact_text(node).to_ascii_lowercase();
    let mut facts = AnnotationFacts::default();
    for atom in annotation_atoms(&text) {
        if let Some(value) = temporal_value(atom) {
            match value {
                "bitemporal" => facts.note_temporal(Temporal::Bitemporal),
                "businesstemporal" => facts.note_temporal(Temporal::BusinessTemporal),
                "processingtemporal" => facts.note_temporal(Temporal::ProcessingTemporal),
                _ => facts.mark_temporal_uncertain(),
            }
        }
        if milestoning_value(atom) == Some(GENERATED_MILESTONING_PROPERTY) {
            facts.generated = true;
        }
    }
    facts
}

fn annotation_atoms(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character| {
        matches!(
            character,
            '{' | '}' | '<' | '>' | ',' | '=' | '(' | ')' | '[' | ']'
        )
    })
    .filter(|atom| !atom.is_empty())
}

fn temporal_value(atom: &str) -> Option<&str> {
    atom.strip_prefix("temporal.")
        .or_else(|| atom.strip_prefix("meta::pure::profiles::temporal."))
}

fn milestoning_value(atom: &str) -> Option<&str> {
    atom.strip_prefix("milestoning.")
        .or_else(|| atom.strip_prefix("meta::pure::profiles::milestoning."))
}

fn direct_name(node: &GreenNode) -> Option<Name> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_token)
        .map(GreenToken::text)
        .find(|text| is_simple_name(text))
        .and_then(|text| Name::new(text).ok())
}

fn qname_from_node(node: &GreenNode) -> Option<QName> {
    let text = compact_text(node);
    is_qualified_name(&text)
        .then(|| QName::new(text).ok())
        .flatten()
}

fn type_from_node(node: &GreenNode) -> Option<TypeRef> {
    TypeTextParser::new(&compact_text(node)).parse()
}

fn multiplicity_from_node(node: &GreenNode) -> Option<Multiplicity> {
    let text = compact_text(node);
    let body = text.strip_prefix('[')?.strip_suffix(']')?;
    if body == "*" {
        return Multiplicity::new(0, None).ok();
    }
    if let Some((lower, upper)) = body.split_once("..") {
        if upper.contains("..") {
            return None;
        }
        let lower = parse_bound(lower)?;
        let upper = if upper == "*" {
            None
        } else {
            Some(parse_bound(upper)?)
        };
        return Multiplicity::new(lower, upper).ok();
    }
    let bound = parse_bound(body)?;
    Multiplicity::new(bound, Some(bound)).ok()
}

fn parse_bound(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

struct TypeTextParser<'text> {
    text: &'text str,
    offset: usize,
}

impl<'text> TypeTextParser<'text> {
    const fn new(text: &'text str) -> Self {
        Self { text, offset: 0 }
    }

    fn parse(mut self) -> Option<TypeRef> {
        let ty = self.parse_type()?;
        (self.offset == self.text.len()).then_some(ty)
    }

    fn parse_type(&mut self) -> Option<TypeRef> {
        let start = self.offset;
        while let Some(byte) = self.byte_at_offset() {
            if matches!(byte, b'<' | b'>' | b',') {
                break;
            }
            self.offset = self.offset.saturating_add(1);
        }
        let path = self.text.get(start..self.offset)?;
        if !is_qualified_name(path) {
            return None;
        }
        let raw_type = QName::new(path).ok()?;
        let mut type_arguments = Vec::new();
        if self.consume(b'<') {
            type_arguments.push(self.parse_type()?);
            while self.consume(b',') {
                type_arguments.push(self.parse_type()?);
            }
            if !self.consume(b'>') {
                return None;
            }
        }
        Some(TypeRef::new(raw_type, type_arguments))
    }

    fn byte_at_offset(&self) -> Option<u8> {
        self.text.as_bytes().get(self.offset).copied()
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.byte_at_offset() == Some(expected) {
            self.offset = self.offset.saturating_add(1);
            true
        } else {
            false
        }
    }
}

fn is_qualified_name(text: &str) -> bool {
    text.split("::").all(is_simple_name)
}

fn is_simple_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn compact_text(node: &GreenNode) -> String {
    node.tokens()
        .filter(|token| !is_trivia(token.kind()))
        .map(GreenToken::text)
        .collect()
}

fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}

fn node_is_unconfirmed(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> bool {
    gaps.iter().any(|gap| {
        gap.kind == DomainCoverageGapKind::MalformedDeclaration
            && range_start(gap.span) == range_start(node.text_range())
    }) || diagnostics
        .iter()
        .any(|diagnostic| ranges_touch_or_overlap(diagnostic.primary.span, node.text_range()))
}

fn node_has_coverage_gap(
    node: &GreenNode,
    gaps: &[DomainCoverageGap],
    diagnostics: &[Diagnostic],
) -> bool {
    gaps.iter()
        .any(|gap| ranges_touch_or_overlap(gap.span, node.text_range()))
        || diagnostics
            .iter()
            .any(|diagnostic| ranges_touch_or_overlap(diagnostic.primary.span, node.text_range()))
}

fn ranges_touch_or_overlap(left: TextRange, right: TextRange) -> bool {
    range_start(left) <= range_end(right) && range_start(right) <= range_end(left)
}

fn range_start(range: TextRange) -> usize {
    usize::from(range.start())
}

fn range_end(range: TextRange) -> usize {
    usize::from(range.end())
}

fn find_domain_file(node: &GreenNode) -> Option<&GreenNode> {
    if node.kind() == SyntaxKind::DOMAIN_FILE {
        return Some(node);
    }
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .find_map(find_domain_file)
}

fn direct_nodes(node: &GreenNode, kind: SyntaxKind) -> impl Iterator<Item = &GreenNode> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .filter(move |child| child.kind() == kind)
}
