//! A byte trie of legal completion strings, and the prefix-walk that makes L2
//! narrowing BPE-aware (`docs/spec/schema.md` §6.5).
//!
//! The whole-lexeme narrower kept a vocab token only if its *entire* bytes
//! classified to a name in the schema-legal set. Under byte-level BPE a schema
//! identifier arrives in fragments (`countryName` → `country` + `Name`), so its
//! leading sub-token equals no whole name and was wrongly cleared — masking a
//! token the model must emit (adversarial-review B1). This trie replaces the
//! equality test with reachability: a token is kept while it can still *extend
//! some* legal name from the bytes emitted so far.
//!
//! Pure `std` — built from the [`Schema`](crate::schema::Schema) alone, no new
//! dependency and no `unsafe` (constitution §1).

use crate::grammar::pda::is_ident_tail;

/// A byte trie over a set of legal completion strings (member names, source
/// classpaths, quoted column strings). Node `0` is the root.
#[derive(Debug, Clone)]
pub(crate) struct Trie {
    nodes: Vec<Node>,
}

/// One trie node: its outgoing edges (sorted by byte for binary search — dense
/// alphabets never blow up to a 256-wide array) and whether a legal name ends
/// here.
#[derive(Debug, Default, Clone)]
struct Node {
    next: Vec<(u8, u32)>,
    terminal: bool,
}

/// The outcome of walking a token's bytes from a cursor node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Walk {
    /// The bytes are a live prefix ending at this node — keep the token and
    /// advance the cursor here.
    Stay(u32),
    /// The bytes completed a legal name and continued with a boundary byte (a
    /// non-identifier byte the byte-PDA will re-vet) — keep the token; the name is
    /// done.
    Complete,
    /// The bytes cannot extend any legal name — clear the token.
    Diverge,
}

impl Trie {
    /// Build a trie from a set of legal completion byte-strings.
    pub(crate) fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<[u8]>,
    {
        let mut trie = Self {
            nodes: vec![Node::default()],
        };
        for name in names {
            trie.insert(name.as_ref());
        }
        trie
    }

    /// The root node id (the cursor start before any byte is emitted).
    pub(crate) fn root(&self) -> u32 {
        0
    }

    fn insert(&mut self, bytes: &[u8]) {
        let mut node = 0usize;
        for &byte in bytes {
            node = match self.child(node as u32, byte) {
                Some(child) => child as usize,
                None => {
                    let child = self.nodes.len() as u32;
                    self.nodes.push(Node::default());
                    let edges = &mut self.nodes[node].next;
                    // This arm runs only when `byte` is absent from `edges`, so the
                    // search never hits `Ok`; the `Err` index is the sorted
                    // insertion point. Reusing the same key projection as `child`
                    // keeps insert and lookup ordering in lockstep (DRY).
                    let at = edges
                        .binary_search_by_key(&byte, |&(b, _)| b)
                        .unwrap_or_else(|at| at);
                    edges.insert(at, (byte, child));
                    child as usize
                }
            };
        }
        self.nodes[node].terminal = true;
    }

    fn child(&self, node: u32, byte: u8) -> Option<u32> {
        let edges = &self.nodes[node as usize].next;
        edges
            .binary_search_by_key(&byte, |&(b, _)| b)
            .ok()
            .map(|i| edges[i].1)
    }

    fn is_terminal(&self, node: u32) -> bool {
        self.nodes[node as usize].terminal
    }
}

/// Walk `bytes` from cursor `node`, deciding whether the token stays on a path to
/// some legal name.
///
/// Descent prefers a trie edge over a terminal, so a name that is a prefix of a
/// longer one (`country` ⊂ `countryName`) keeps walking rather than stopping
/// short. Only when no edge continues does the terminal decide: a boundary byte
/// after a complete name is [`Complete`](Walk::Complete) (the name is done, the
/// tail is the byte-PDA's to vet), an identifier byte past the name is a phantom
/// extension ([`Diverge`](Walk::Diverge)).
pub(crate) fn walk(trie: &Trie, mut node: u32, bytes: &[u8]) -> Walk {
    for &byte in bytes {
        match trie.child(node, byte) {
            Some(child) => node = child,
            None => {
                return if trie.is_terminal(node) && !is_ident_tail(byte) {
                    Walk::Complete
                } else {
                    Walk::Diverge
                };
            }
        }
    }
    Walk::Stay(node)
}

#[cfg(test)]
mod tests {
    use super::{Trie, Walk, walk};

    fn member_trie() -> Trie {
        Trie::from_names(["country", "countryName", "countryId", "id"])
    }

    #[test]
    fn a_whole_name_walks_to_a_terminal_and_stays() {
        let trie = member_trie();
        assert!(matches!(walk(&trie, trie.root(), b"id"), Walk::Stay(_)));
        assert!(matches!(
            walk(&trie, trie.root(), b"countryName"),
            Walk::Stay(_)
        ));
    }

    #[test]
    fn a_leading_prefix_stays_alive() {
        // The exact B1 case: the leading BPE sub-token of a multi-token name.
        let trie = member_trie();
        assert!(matches!(walk(&trie, trie.root(), b"count"), Walk::Stay(_)));
        // …and a whole-name prefix that is *also* a shorter name still descends to
        // the longer one when more bytes arrive (child preferred over terminal).
        let Walk::Stay(node) = walk(&trie, trie.root(), b"country") else {
            panic!("prefix stays");
        };
        assert!(matches!(walk(&trie, node, b"Name"), Walk::Stay(_)));
        assert!(matches!(walk(&trie, node, b"Id"), Walk::Stay(_)));
    }

    #[test]
    fn a_completed_name_then_a_boundary_byte_completes() {
        let trie = member_trie();
        // `id` is a name; a following `.` (a boundary byte) means the name is done.
        assert_eq!(walk(&trie, trie.root(), b"id."), Walk::Complete);
        assert_eq!(walk(&trie, trie.root(), b"id("), Walk::Complete);
    }

    #[test]
    fn a_strict_prefix_then_a_boundary_byte_diverges_not_completes() {
        // `count` is a strict prefix of `country*` but is *not* itself a legal name
        // (a non-terminal node). A following boundary byte `.` must therefore
        // Diverge — the name never completed. This pins `is_terminal` reporting the
        // real terminal flag: were it to always answer `true`, this boundary byte
        // would wrongly read as `Complete`.
        let trie = member_trie();
        assert_eq!(walk(&trie, trie.root(), b"count."), Walk::Diverge);
        assert_eq!(walk(&trie, trie.root(), b"countr("), Walk::Diverge);
    }

    #[test]
    fn a_phantom_extension_diverges() {
        let trie = member_trie();
        // `idx` extends the complete name `id` with an identifier byte — a phantom.
        assert_eq!(walk(&trie, trie.root(), b"idx"), Walk::Diverge);
        // A first byte off any name diverges immediately.
        assert_eq!(walk(&trie, trie.root(), b"z"), Walk::Diverge);
        // A prefix that then leaves every name diverges.
        assert_eq!(walk(&trie, trie.root(), b"countX"), Walk::Diverge);
    }

    #[test]
    fn a_quoted_column_string_walks_around_its_quotes() {
        let trie = Trie::from_names(["'Name'", "'Result'"]);
        // The opening quote alone is a live prefix (the B1 leading `'`).
        let Walk::Stay(node) = walk(&trie, trie.root(), b"'") else {
            panic!("opening quote stays");
        };
        let Walk::Stay(node) = walk(&trie, node, b"Na") else {
            panic!("inner prefix stays");
        };
        assert!(matches!(walk(&trie, node, b"me'"), Walk::Stay(_)));
        // An unlisted column diverges once its bytes leave every entry.
        assert_eq!(walk(&trie, trie.root(), b"'Ghost"), Walk::Diverge);
    }

    #[test]
    fn an_empty_trie_diverges_on_any_byte() {
        let trie = Trie::from_names(Vec::<&str>::new());
        assert_eq!(walk(&trie, trie.root(), b"x"), Walk::Diverge);
        // An empty token stays at the cursor (no byte can diverge).
        assert!(matches!(walk(&trie, trie.root(), b""), Walk::Stay(_)));
    }
}
