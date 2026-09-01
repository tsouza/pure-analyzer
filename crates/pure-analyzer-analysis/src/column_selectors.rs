//! Conservative extraction and resolution of relational column selector syntax.

use std::collections::BTreeSet;

use pure_analyzer_diagnostics::FileId;
use pure_analyzer_model::Name;
use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

use crate::{ColumnId, RelationSchema, SourceSpan};

/// A column name as it was written in a relation selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnSelectorName {
    /// A bare identifier selector such as `name`.
    Bare(Name),
    /// A quoted selector such as `'Total Revenue'`.
    Quoted(Name),
}

impl ColumnSelectorName {
    /// Return the exact decoded name used for schema lookup.
    #[must_use]
    pub const fn name(&self) -> &Name {
        match self {
            Self::Bare(name) | Self::Quoted(name) => name,
        }
    }

    /// Return whether source wrote this selector with string quotes.
    #[must_use]
    pub const fn is_quoted(&self) -> bool {
        matches!(self, Self::Quoted(_))
    }
}

/// One source-order relation column selector with exact syntax spans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSelector {
    name: ColumnSelectorName,
    source: SourceSpan,
    name_source: SourceSpan,
}

impl ColumnSelector {
    fn new(name: ColumnSelectorName, source: SourceSpan, name_source: SourceSpan) -> Self {
        Self {
            name,
            source,
            name_source,
        }
    }

    /// Return the selector spelling category and decoded lookup name.
    #[must_use]
    pub const fn name(&self) -> &ColumnSelectorName {
        &self.name
    }

    /// Return the exact selector span, including its leading `~` when present.
    #[must_use]
    pub const fn source(&self) -> SourceSpan {
        self.source
    }

    /// Return the exact name-token span, excluding selector punctuation and trivia.
    #[must_use]
    pub const fn name_source(&self) -> SourceSpan {
        self.name_source
    }
}

/// An ordered collection of extracted relation column selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSelectors {
    source: SourceSpan,
    selectors: Vec<ColumnSelector>,
}

impl ColumnSelectors {
    fn new(source: SourceSpan, selectors: Vec<ColumnSelector>) -> Self {
        Self { source, selectors }
    }

    /// Return the exact span of the single selector or selector array.
    #[must_use]
    pub const fn source(&self) -> SourceSpan {
        self.source
    }

    /// Return selectors in source order.
    #[must_use]
    pub fn selectors(&self) -> &[ColumnSelector] {
        &self.selectors
    }
}

/// One selector resolved to a stable relational schema column identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumnSelector {
    selector: ColumnSelector,
    column: ColumnId,
}

impl ResolvedColumnSelector {
    fn new(selector: ColumnSelector, column: ColumnId) -> Self {
        Self { selector, column }
    }

    /// Return the source selector that resolved successfully.
    #[must_use]
    pub const fn selector(&self) -> &ColumnSelector {
        &self.selector
    }

    /// Return the stable identity from the resolved relation schema.
    #[must_use]
    pub const fn column(&self) -> ColumnId {
        self.column
    }
}

/// Ordered successful relation-column selector resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumnSelectors {
    source: SourceSpan,
    selectors: Vec<ResolvedColumnSelector>,
}

impl ResolvedColumnSelectors {
    fn new(source: SourceSpan, selectors: Vec<ResolvedColumnSelector>) -> Self {
        Self { source, selectors }
    }

    /// Return the exact span of the resolved single selector or selector array.
    #[must_use]
    pub const fn source(&self) -> SourceSpan {
        self.source
    }

    /// Return resolved selectors in source order.
    #[must_use]
    pub fn selectors(&self) -> &[ResolvedColumnSelector] {
        &self.selectors
    }
}

/// The explicit reason conservative selector processing could not resolve safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnSelectorOpaqueReason {
    /// The supplied CST node is not a supported selector form.
    UnsupportedForm,
    /// Parser recovery or an invalid selector structure prevents exact extraction.
    Malformed,
    /// A typed or lambda selector body is outside this extractor's supported subset.
    UnsupportedBody,
    /// No relation-schema column has the requested exact name.
    Missing(Name),
    /// More than one schema column has the requested exact name.
    DuplicateSchemaName(Name),
    /// More than one selector requests the same exact decoded name.
    DuplicateSelector(Name),
}

/// A typed opaque selector outcome with the precise source span that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSelectorOpaque {
    reason: ColumnSelectorOpaqueReason,
    source: SourceSpan,
}

impl ColumnSelectorOpaque {
    fn new(reason: ColumnSelectorOpaqueReason, source: SourceSpan) -> Self {
        Self { reason, source }
    }

    /// Return the conservative opaque reason.
    #[must_use]
    pub const fn reason(&self) -> &ColumnSelectorOpaqueReason {
        &self.reason
    }

    /// Return the precise malformed, unsupported, or unresolved source span.
    #[must_use]
    pub const fn source(&self) -> SourceSpan {
        self.source
    }
}

/// The result of resolving one existing relation-column selector CST form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnSelectorOutcome {
    /// Every selector resolved exactly and in source order.
    Resolved(ResolvedColumnSelectors),
    /// No safe partial resolution is available.
    Opaque(ColumnSelectorOpaque),
}

/// Extract a single `COLUMN_SPEC` or `COLUMN_SPEC_ARRAY` into source-order selectors.
///
/// Typed/lambda bodies, parser recovery, and malformed separator structure return a
/// [`ColumnSelectorOpaque`] instead of a partial selector list.
pub fn extract_relation_column_selectors(
    file: FileId,
    node: &GreenNode,
) -> Result<ColumnSelectors, ColumnSelectorOpaque> {
    if contains_error_node(node) {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    }

    let source = significant_source(file, node);
    let selectors = match node.kind() {
        SyntaxKind::COLUMN_SPEC => vec![extract_selector(file, node, true)?],
        SyntaxKind::COLUMN_SPEC_ARRAY => extract_array(file, node)?,
        _ => {
            return Err(opaque(
                file,
                node,
                ColumnSelectorOpaqueReason::UnsupportedForm,
            ));
        }
    };
    Ok(ColumnSelectors::new(source, selectors))
}

/// Resolve a relation column selector CST form against an ordered schema.
///
/// Resolution preserves selector source order and returns stable [`ColumnId`] values.
/// Missing, ambiguous, duplicate, malformed, and unsupported forms never produce a
/// partially resolved list.
#[must_use]
pub fn resolve_relation_column_selectors(
    file: FileId,
    node: &GreenNode,
    schema: &RelationSchema,
) -> ColumnSelectorOutcome {
    let selectors = match extract_relation_column_selectors(file, node) {
        Ok(selectors) => selectors,
        Err(opaque) => return ColumnSelectorOutcome::Opaque(opaque),
    };
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(selectors.selectors.len());

    for selector in &selectors.selectors {
        let name = selector.name.name();
        if !seen.insert(name.clone()) {
            return ColumnSelectorOutcome::Opaque(ColumnSelectorOpaque::new(
                ColumnSelectorOpaqueReason::DuplicateSelector(name.clone()),
                selector.source(),
            ));
        }

        let matches = schema
            .columns()
            .iter()
            .filter(|column| column.name() == name)
            .collect::<Vec<_>>();
        let column = match matches.as_slice() {
            [] => {
                return ColumnSelectorOutcome::Opaque(ColumnSelectorOpaque::new(
                    ColumnSelectorOpaqueReason::Missing(name.clone()),
                    selector.name_source(),
                ));
            }
            [column] => column.id(),
            _ => {
                return ColumnSelectorOutcome::Opaque(ColumnSelectorOpaque::new(
                    ColumnSelectorOpaqueReason::DuplicateSchemaName(name.clone()),
                    selector.name_source(),
                ));
            }
        };
        resolved.push(ResolvedColumnSelector::new(selector.clone(), column));
    }

    ColumnSelectorOutcome::Resolved(ResolvedColumnSelectors::new(selectors.source, resolved))
}

fn extract_array(
    file: FileId,
    node: &GreenNode,
) -> Result<Vec<ColumnSelector>, ColumnSelectorOpaque> {
    let elements = node
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let mut index = 0;
    if !takes_token(elements.get(index), SyntaxKind::TILDE) {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    }
    index += 1;
    if !takes_token(elements.get(index), SyntaxKind::BRACKET_OPEN) {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    }
    index += 1;

    let mut selectors = Vec::new();
    loop {
        let Some(element) = elements.get(index) else {
            return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
        };
        if takes_token(Some(element), SyntaxKind::BRACKET_CLOSE) {
            return if selectors.is_empty() {
                Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed))
            } else {
                Ok(selectors)
            };
        }
        let Some(selector) = element.as_node() else {
            return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
        };
        if selector.kind() != SyntaxKind::COLUMN_SPEC {
            return Err(opaque(
                file,
                selector,
                ColumnSelectorOpaqueReason::Malformed,
            ));
        }
        selectors.push(extract_selector(file, selector, false)?);
        index += 1;

        if takes_token(elements.get(index), SyntaxKind::BRACKET_CLOSE) {
            return Ok(selectors);
        }
        if !takes_token(elements.get(index), SyntaxKind::COMMA) {
            return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
        }
        index += 1;
        if takes_token(elements.get(index), SyntaxKind::BRACKET_CLOSE) {
            return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
        }
    }
}

fn extract_selector(
    file: FileId,
    node: &GreenNode,
    requires_tilde: bool,
) -> Result<ColumnSelector, ColumnSelectorOpaque> {
    if contains_error_node(node) {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    }

    let children = node
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let names = children
        .iter()
        .filter_map(|element| element.as_node())
        .filter(|child| child.kind() == SyntaxKind::COLUMN_NAME)
        .collect::<Vec<_>>();
    let has_unsupported_body = children
        .iter()
        .filter_map(|element| element.as_node())
        .any(|child| child.kind() != SyntaxKind::COLUMN_NAME);
    if has_unsupported_body
        || children
            .iter()
            .filter_map(|element| element.as_token())
            .any(|token| token.kind() == SyntaxKind::COLON)
    {
        return Err(opaque(
            file,
            node,
            ColumnSelectorOpaqueReason::UnsupportedBody,
        ));
    }
    let [name] = names.as_slice() else {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    };

    let tokens = children
        .iter()
        .filter_map(|element| element.as_token())
        .collect::<Vec<_>>();
    let valid_tokens = if requires_tilde {
        matches!(tokens.as_slice(), [token] if token.kind() == SyntaxKind::TILDE)
    } else {
        tokens.is_empty()
    };
    if !valid_tokens {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    }

    let (name, name_source) = extract_name(file, name)?;
    Ok(ColumnSelector::new(
        name,
        significant_source(file, node),
        name_source,
    ))
}

fn extract_name(
    file: FileId,
    node: &GreenNode,
) -> Result<(ColumnSelectorName, SourceSpan), ColumnSelectorOpaque> {
    let elements = node
        .children()
        .iter()
        .filter(|element| !element_is_trivia(element))
        .collect::<Vec<_>>();
    let [GreenElement::Token(token)] = elements.as_slice() else {
        return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed));
    };
    let source = SourceSpan::new(file, token.text_range());
    let name = match token.kind() {
        SyntaxKind::IDENT => Name::new(token.text())
            .map(ColumnSelectorName::Bare)
            .map_err(|_| opaque(file, node, ColumnSelectorOpaqueReason::Malformed))?,
        SyntaxKind::STRING => quoted_name(token.text())
            .and_then(|text| Name::new(text).ok())
            .map(ColumnSelectorName::Quoted)
            .ok_or_else(|| opaque(file, node, ColumnSelectorOpaqueReason::Malformed))?,
        _ => return Err(opaque(file, node, ColumnSelectorOpaqueReason::Malformed)),
    };
    Ok((name, source))
}

fn quoted_name(text: &str) -> Option<String> {
    let value = text.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(value.replace("''", "'"))
}

fn opaque(
    file: FileId,
    node: &GreenNode,
    reason: ColumnSelectorOpaqueReason,
) -> ColumnSelectorOpaque {
    ColumnSelectorOpaque::new(reason, significant_source(file, node))
}

fn significant_source(file: FileId, node: &GreenNode) -> SourceSpan {
    let mut tokens = node.tokens().filter(|token| !is_trivia(token.kind()));
    let Some(first) = tokens.next() else {
        return SourceSpan::new(file, node.text_range());
    };
    let range = tokens.fold(first.text_range(), |range, token| {
        pure_analyzer_syntax::TextRange::new(range.start(), token.text_range().end())
    });
    SourceSpan::new(file, range)
}

fn takes_token(element: Option<&&GreenElement>, kind: SyntaxKind) -> bool {
    matches!(element, Some(GreenElement::Token(token)) if token.kind() == kind)
}

fn element_is_trivia(element: &GreenElement) -> bool {
    element
        .as_token()
        .is_some_and(|token| is_trivia(token.kind()))
}

fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}

fn contains_error_node(node: &GreenNode) -> bool {
    node.kind() == SyntaxKind::ERROR_NODE
        || node.tokens().any(|token| token.kind() == SyntaxKind::ERROR)
        || node
            .children()
            .iter()
            .filter_map(GreenElement::as_node)
            .any(contains_error_node)
}
