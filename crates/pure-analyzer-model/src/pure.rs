use std::collections::{BTreeMap, BTreeSet};

use pure_analyzer_diagnostics::{DiagCode, Diagnostic, Label, Severity, TextRange};
use pure_analyzer_parser::{DomainCoverageGap, DomainCoverageGapKind, parse_domain};
use pure_analyzer_syntax::{GreenElement, GreenNode, GreenToken, SyntaxKind};

use crate::error::ModelError;
use crate::loader::{AssocDraft, FragmentElement, ModelFragment};
use crate::{
    ClassInfo, Multiplicity, Name, PropInfo, Provenance, QName, QpInfo, QpKind, SourceId, Temporal,
    TypeRef,
};

const GENERATED_MILESTONING_PROPERTY: &str = "generatedmilestoningproperty";
const ALL_VERSIONS_SUFFIX: &str = "AllVersions";
const ALL_VERSIONS_IN_RANGE_SUFFIX: &str = "AllVersionsInRange";

#[derive(Clone, Copy)]
struct LoweringContext<'a> {
    gaps: &'a [DomainCoverageGap],
    parser_diagnostics: &'a [Diagnostic],
    source: SourceId,
}

/// Parses a resilient Domain source and lowers only its confirmed model facts.
pub(super) fn parse_pure_document(
    source_name: &str,
    text: &str,
    source: SourceId,
) -> Result<ModelFragment, ModelError> {
    let parsed = parse_domain(text, source.file_id()).map_err(|error| ModelError::PureParse {
        source_name: source_name.to_owned(),
        source: error,
    })?;
    let context = LoweringContext {
        gaps: &parsed.coverage_gaps,
        parser_diagnostics: &parsed.diagnostics,
        source,
    };
    let (elements, mut lowering_diagnostics, coverage_gap) = lower_domain(&parsed.green, context);
    let mut diagnostics = parsed.diagnostics;
    diagnostics.append(&mut lowering_diagnostics);
    diagnostics.sort_by_key(|diagnostic| range_start(diagnostic.primary.span));
    Ok(ModelFragment {
        elements,
        diagnostics,
        coverage_gap,
    })
}

fn lower_domain(
    root: &GreenNode,
    context: LoweringContext<'_>,
) -> (BTreeMap<QName, FragmentElement>, Vec<Diagnostic>, bool) {
    let Some(file) = find_domain_file(root) else {
        return (BTreeMap::new(), Vec::new(), true);
    };
    let mut source_wide_gap = context
        .gaps
        .iter()
        .any(|gap| gap.kind == DomainCoverageGapKind::UnsupportedTopLevel);
    let mut class_entries = Vec::new();
    let mut association_entries = Vec::new();
    let mut top_level_declarations = Vec::new();
    let mut lowering_diagnostics = Vec::new();
    for declaration in file.children().iter().filter_map(GreenElement::as_node) {
        match declaration.kind() {
            SyntaxKind::DOMAIN_CLASS_DECL => {
                if let Some((path, _)) = declaration_path(declaration) {
                    top_level_declarations.push(TopLevelDeclaration {
                        path,
                        kind: TopLevelDeclarationKind::Class,
                        span: declaration.text_range(),
                    });
                }
                match lower_class(declaration, context) {
                    Some((class, diagnostics)) => {
                        class_entries.push(class);
                        lowering_diagnostics.extend(diagnostics);
                    }
                    None => source_wide_gap = true,
                }
            }
            SyntaxKind::DOMAIN_ASSOCIATION_DECL => {
                if let Some((path, _)) = declaration_path(declaration) {
                    top_level_declarations.push(TopLevelDeclaration {
                        path,
                        kind: TopLevelDeclarationKind::Association,
                        span: declaration.text_range(),
                    });
                }
                let lowered = lower_association(declaration, context);
                source_wide_gap |= lowered.uncertain;
                if let Some(association) = lowered.value {
                    association_entries.push(association);
                }
            }
            _ => {}
        }
    }
    let (duplicate_paths, duplicate_diagnostics) =
        duplicate_top_level_paths(&top_level_declarations, context.source);
    source_wide_gap |= !duplicate_paths.is_empty();
    lowering_diagnostics.extend(duplicate_diagnostics);

    let mut elements = BTreeMap::new();
    for mut class in class_entries {
        if duplicate_paths.contains(class.path()) {
            continue;
        }
        if source_wide_gap {
            class.mark_coverage_gap();
        }
        let path = class.path().clone();
        elements.insert(path, FragmentElement::Class(class));
    }
    for association in association_entries {
        if duplicate_paths.contains(&association.path) {
            continue;
        }
        elements.insert(
            association.path.clone(),
            FragmentElement::Association(association),
        );
    }
    (elements, lowering_diagnostics, source_wide_gap)
}

fn duplicate_top_level_paths(
    declarations: &[TopLevelDeclaration],
    source: SourceId,
) -> (BTreeSet<QName>, Vec<Diagnostic>) {
    let mut first_declarations: BTreeMap<QName, &TopLevelDeclaration> = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for declaration in declarations {
        if let Some(first) = first_declarations.get(&declaration.path) {
            duplicates.insert(declaration.path.clone());
            diagnostics.push(duplicate_top_level_diagnostic(first, declaration, source));
        } else {
            first_declarations.insert(declaration.path.clone(), declaration);
        }
    }
    (duplicates, diagnostics)
}

struct TopLevelDeclaration {
    path: QName,
    kind: TopLevelDeclarationKind,
    span: TextRange,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopLevelDeclarationKind {
    Class,
    Association,
}

fn duplicate_top_level_diagnostic(
    first: &TopLevelDeclaration,
    duplicate: &TopLevelDeclaration,
    source: SourceId,
) -> Diagnostic {
    let message = match (first.kind, duplicate.kind) {
        (TopLevelDeclarationKind::Class, TopLevelDeclarationKind::Class) => {
            format!(
                "Pure source declares class `{}` more than once",
                duplicate.path
            )
        }
        (TopLevelDeclarationKind::Association, TopLevelDeclarationKind::Association) => {
            format!(
                "Pure source declares association `{}` more than once",
                duplicate.path
            )
        }
        _ => format!(
            "Pure source declares `{}` as both a class and association",
            duplicate.path
        ),
    };
    Diagnostic::builder(
        DiagCode::DuplicateModelDeclaration,
        Severity::Error,
        message,
        Label::with_note(source.file_id(), duplicate.span, "duplicate declaration"),
    )
    .secondary(Label::with_note(
        source.file_id(),
        first.span,
        "first declaration",
    ))
    .build()
}

fn lower_class(
    node: &GreenNode,
    context: LoweringContext<'_>,
) -> Option<(ClassInfo, Vec<Diagnostic>)> {
    let (path, name_index) = declaration_path(node)?;
    let annotations = annotations_before(node, name_index);
    let (supertypes, supertype_gap) = lower_supertypes(node, context);
    let (properties, qualified_properties, member_gap, member_diagnostics) =
        lower_class_members(node, &path, name_index, context);
    let mut coverage_gap = node_has_coverage_gap(node, context);
    if annotations.temporal_uncertain {
        coverage_gap = true;
    }
    if supertype_gap {
        coverage_gap = true;
    }
    if member_gap {
        coverage_gap = true;
    }
    let mut class = ClassInfo::from_pure(
        path,
        supertypes,
        annotations.temporal,
        properties,
        qualified_properties,
        context.source,
    );
    if coverage_gap {
        class.mark_coverage_gap();
    }
    Some((class, member_diagnostics))
}

fn lower_supertypes(node: &GreenNode, context: LoweringContext<'_>) -> (Vec<QName>, bool) {
    let Some(extends) = direct_nodes(node, SyntaxKind::DOMAIN_EXTENDS_CLAUSE).next() else {
        return (Vec::new(), false);
    };
    if node_has_coverage_gap(extends, context) {
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
    class: &QName,
    name_index: usize,
    context: LoweringContext<'_>,
) -> (
    BTreeMap<Name, PropInfo>,
    BTreeMap<Name, QpInfo>,
    bool,
    Vec<Diagnostic>,
) {
    let mut members = ClassMemberFacts::default();
    let mut pending_annotations = AnnotationFacts::default();
    let mut coverage_gap = false;

    for element in node.children().iter().skip(name_index.saturating_add(1)) {
        let Some(member) = element.as_node() else {
            continue;
        };
        if matches!(
            member.kind(),
            SyntaxKind::DOMAIN_OPAQUE_NODE | SyntaxKind::ERROR_NODE
        ) {
            coverage_gap = true;
            pending_annotations = AnnotationFacts::default();
            continue;
        }
        match member.kind() {
            SyntaxKind::DOMAIN_STEREOTYPE_APPLICATIONS => {
                pending_annotations.merge(annotation_facts(member));
            }
            SyntaxKind::DOMAIN_PROPERTY_DECL => {
                if !insert_property(&mut members, member, class, context) {
                    coverage_gap = true;
                }
                pending_annotations = AnnotationFacts::default();
            }
            SyntaxKind::DOMAIN_QUALIFIED_PROPERTY_DECL => {
                if !insert_qualified_property(
                    &mut members,
                    member,
                    pending_annotations,
                    class,
                    context,
                ) {
                    coverage_gap = true;
                }
                pending_annotations = AnnotationFacts::default();
            }
            _ => {}
        }
    }
    (
        members.properties,
        members.qualified_properties,
        coverage_gap,
        members.diagnostics,
    )
}

#[derive(Default)]
struct ClassMemberFacts {
    properties: BTreeMap<Name, PropInfo>,
    qualified_properties: BTreeMap<Name, QpInfo>,
    property_declarations: BTreeMap<Name, TextRange>,
    qualified_property_declarations: BTreeMap<Name, TextRange>,
    diagnostics: Vec<Diagnostic>,
}

fn insert_property(
    members: &mut ClassMemberFacts,
    node: &GreenNode,
    class: &QName,
    context: LoweringContext<'_>,
) -> bool {
    let Some(name) = direct_name(node) else {
        return false;
    };
    if let Some(first) = members.property_declarations.get(&name) {
        let _ = members.properties.remove(&name);
        members.diagnostics.push(duplicate_member_diagnostic(
            class,
            &name,
            MemberDeclarationKind::Property,
            *first,
            node.text_range(),
            context.source,
        ));
        return false;
    }
    members
        .property_declarations
        .insert(name.clone(), node.text_range());
    if context.gaps.iter().any(|gap| {
        matches!(gap.kind, DomainCoverageGapKind::MalformedDeclaration)
            && range_start(gap.span) == range_start(node.text_range())
    }) {
        return false;
    }
    if context
        .parser_diagnostics
        .iter()
        .any(|diagnostic| ranges_touch_or_overlap(diagnostic.primary.span, node.text_range()))
    {
        return false;
    }
    let Some(property) = lower_property(node) else {
        return false;
    };
    members.properties.insert(name, property).is_none()
}

fn insert_qualified_property(
    members: &mut ClassMemberFacts,
    node: &GreenNode,
    annotations: AnnotationFacts,
    class: &QName,
    context: LoweringContext<'_>,
) -> bool {
    let Some(name) = direct_name(node) else {
        return false;
    };
    if let Some(first) = members.qualified_property_declarations.get(&name) {
        let _ = members.qualified_properties.remove(&name);
        members.diagnostics.push(duplicate_member_diagnostic(
            class,
            &name,
            MemberDeclarationKind::QualifiedProperty,
            *first,
            node.text_range(),
            context.source,
        ));
        return false;
    }
    members
        .qualified_property_declarations
        .insert(name.clone(), node.text_range());
    if context.gaps.iter().any(|gap| {
        matches!(gap.kind, DomainCoverageGapKind::MalformedDeclaration)
            && range_start(gap.span) == range_start(node.text_range())
    }) {
        return false;
    }
    if context
        .parser_diagnostics
        .iter()
        .any(|diagnostic| ranges_touch_or_overlap(diagnostic.primary.span, node.text_range()))
    {
        return false;
    }
    let Some(property) = lower_qualified_property(node, annotations, context) else {
        return false;
    };
    members
        .qualified_properties
        .insert(name, property)
        .is_none()
}

#[derive(Clone, Copy)]
enum MemberDeclarationKind {
    Property,
    QualifiedProperty,
}

impl MemberDeclarationKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Property => "property",
            Self::QualifiedProperty => "qualified property",
        }
    }
}

fn duplicate_member_diagnostic(
    class: &QName,
    name: &Name,
    kind: MemberDeclarationKind,
    first: TextRange,
    duplicate: TextRange,
    source: SourceId,
) -> Diagnostic {
    Diagnostic::builder(
        DiagCode::DuplicateModelDeclaration,
        Severity::Error,
        format!(
            "Pure class `{class}` declares {} `{name}` more than once",
            kind.label()
        ),
        Label::with_note(source.file_id(), duplicate, "duplicate declaration"),
    )
    .secondary(Label::with_note(
        source.file_id(),
        first,
        "first declaration",
    ))
    .build()
}

struct LoweredAssociation {
    value: Option<AssocDraft>,
    uncertain: bool,
}

fn lower_association(node: &GreenNode, context: LoweringContext<'_>) -> LoweredAssociation {
    if node_has_coverage_gap(node, context) {
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
        if node_is_unconfirmed(property, context) {
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
            source: context.source,
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
    context: LoweringContext<'_>,
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
        Some(lower_signature(node, context)?)
    } else {
        None
    };
    Some(QpInfo::new(name, target, multiplicity, kind, signature))
}

fn lower_signature(node: &GreenNode, context: LoweringContext<'_>) -> Option<Vec<TypeRef>> {
    let mut signature = Vec::new();
    for parameter in direct_nodes(node, SyntaxKind::DOMAIN_PARAMETER_DECL) {
        if context.gaps.iter().any(|gap| {
            matches!(gap.kind, DomainCoverageGapKind::MalformedDeclaration)
                && range_start(gap.span) == range_start(parameter.text_range())
        }) {
            return None;
        }
        if context.parser_diagnostics.iter().any(|diagnostic| {
            ranges_touch_or_overlap(diagnostic.primary.span, parameter.text_range())
        }) {
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
        if self.temporal.is_some() {
            self.temporal = None;
            self.temporal_uncertain = true;
            return;
        }
        if self.temporal_uncertain {
            self.temporal = None;
            return;
        }
        self.temporal = Some(temporal);
    }
}

fn annotation_facts(node: &GreenNode) -> AnnotationFacts {
    let text = compact_text(node).to_ascii_lowercase();
    let mut facts = AnnotationFacts::default();
    for atom in stereotype_atoms(&text) {
        if let Some(value) = temporal_value(atom) {
            match value {
                "bitemporal" => facts.note_temporal(Temporal::Bitemporal),
                "businesstemporal" => facts.note_temporal(Temporal::BusinessTemporal),
                "processingtemporal" => facts.note_temporal(Temporal::ProcessingTemporal),
                _ => {
                    facts.temporal = None;
                    facts.temporal_uncertain = true;
                }
            }
        }
        if milestoning_value(atom) == Some(GENERATED_MILESTONING_PROPERTY) {
            facts.generated = true;
        }
    }
    facts
}

fn stereotype_atoms(text: &str) -> impl Iterator<Item = &str> {
    // Braced applications are tagged values whose values are arbitrary text.
    // Only the double-angle form carries a semantic stereotype.
    text.strip_prefix("<<")
        .and_then(|contents| contents.strip_suffix(">>"))
        .into_iter()
        .flat_map(|contents| contents.split(','))
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
    qname_from_text(&compact_text(node))
}

fn qname_from_text(text: &str) -> Option<QName> {
    let text = text.strip_prefix("::").unwrap_or(text);
    is_qualified_name(text)
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
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
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
        let iteration_budget = self.text.len().saturating_sub(self.offset);
        for _ in 0..iteration_budget {
            let Some(byte) = self.byte_at_offset() else {
                break;
            };
            if matches!(byte, b'<' | b'>' | b',') {
                break;
            }
            self.offset = self.offset.saturating_add(1);
        }
        let path = self.text.get(start..self.offset)?;
        let raw_type = qname_from_text(path)?;
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
    match first {
        b'_' => {}
        _ if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|byte| {
        if byte == b'_' {
            true
        } else {
            byte.is_ascii_alphanumeric()
        }
    })
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

fn node_is_unconfirmed(node: &GreenNode, context: LoweringContext<'_>) -> bool {
    context.gaps.iter().any(|gap| {
        gap.kind == DomainCoverageGapKind::MalformedDeclaration
            && range_start(gap.span) == range_start(node.text_range())
    }) || context
        .parser_diagnostics
        .iter()
        .any(|diagnostic| ranges_touch_or_overlap(diagnostic.primary.span, node.text_range()))
}

fn node_has_coverage_gap(node: &GreenNode, context: LoweringContext<'_>) -> bool {
    if context
        .gaps
        .iter()
        .any(|gap| ranges_touch_or_overlap(gap.span, node.text_range()))
    {
        return true;
    }
    context
        .parser_diagnostics
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

#[cfg(test)]
mod tests {
    use super::is_qualified_name;

    #[test]
    fn qualified_names_reject_empty_and_non_identifier_segments() {
        assert!(is_qualified_name("demo::Valid_Name"));
        assert!(!is_qualified_name("demo::"));
        assert!(!is_qualified_name("demo::9invalid"));
        assert!(!is_qualified_name("demo::::invalid"));
    }
}
