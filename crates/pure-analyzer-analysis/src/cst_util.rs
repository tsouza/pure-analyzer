//! Shared conservative CST-walking helpers.
//!
//! `lowering`, `local`, `column_selectors`, and `validate` each need to strip
//! trivia, walk immediate child nodes, and reject unparseable syntax before
//! treating it as analyzable. These helpers previously existed as four (in
//! the case of [`is_trivia`]) or two (in the case of [`direct_nodes`] and
//! [`element_is_trivia`]) independently declared copies across those
//! modules; [`contains_error_node`] itself had drifted into two different
//! shapes before this module existed. They carry no pass-specific semantics,
//! so this module is their one shared home.

use pure_analyzer_syntax::{GreenElement, GreenNode, SyntaxKind};

/// Longest [`GreenNode`] nesting this crate's syntax-tree walks will descend
/// before conservatively reporting an error, independent of
/// [`MAX_RELATIONAL_RECURSION_DEPTH`](crate::relational::MAX_RELATIONAL_RECURSION_DEPTH).
///
/// The two budgets protect the same hazard class (unbounded recursion over
/// untrusted input) but cannot share a value: a syntax tree is structurally
/// much deeper than the relational IR it lowers to even for ordinary,
/// non-adversarial source — parenthesized-expression grammar layering alone
/// measures roughly one `GreenNode` level per paren, and
/// `pure_analyzer_parser::m3::MAX_PARSE_DEPTH` (256) already lets a query
/// nest that deep before the parser itself refuses to go further. This
/// budget must clear that ceiling with margin, or it would reject ordinary
/// parser-accepted queries; `MAX_RELATIONAL_RECURSION_DEPTH` (32) is instead
/// sized to the relational walks' own, far more expensive, per-frame stack
/// cost. Each frame here is a handful of field reads, so a much larger budget
/// is still cheap on the smallest worker stack in the workspace.
pub(crate) const MAX_SYNTAX_TREE_DEPTH: usize = 512;

/// Report whether `node` or any descendant is an error node or carries an
/// error token.
///
/// Shared by every pass in this crate that needs to reject unparseable syntax
/// before treating it as analyzable (lowering, local navigation analysis,
/// column-selector extraction). Depth is bounded by
/// [`MAX_SYNTAX_TREE_DEPTH`]: exceeding the budget reports `true` (contains
/// an error) rather than guessing the tree is clean — the same fail-closed
/// default every other bounded walk in this crate uses.
pub(crate) fn contains_error_node(node: &GreenNode) -> bool {
    contains_error_node_at_depth(node, 0)
}

fn contains_error_node_at_depth(node: &GreenNode, depth: usize) -> bool {
    if depth >= MAX_SYNTAX_TREE_DEPTH {
        return true;
    }
    node.kind() == SyntaxKind::ERROR_NODE
        || node.tokens().any(|token| token.kind() == SyntaxKind::ERROR)
        || direct_nodes(node)
            .iter()
            .any(|child| contains_error_node_at_depth(child, depth + 1))
}

/// Return `node`'s immediate child nodes, in order, skipping tokens.
pub(crate) fn direct_nodes(node: &GreenNode) -> Vec<GreenNode> {
    node.children()
        .iter()
        .filter_map(GreenElement::as_node)
        .cloned()
        .collect()
}

/// Report whether `element` is a trivia token (whitespace or a comment).
pub(crate) fn element_is_trivia(element: &GreenElement) -> bool {
    element
        .as_token()
        .is_some_and(|token| is_trivia(token.kind()))
}

/// Report whether `kind` is a trivia token kind (whitespace or a comment).
pub(crate) const fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
    )
}
