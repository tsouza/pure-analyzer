//! The L2 narrowing rules (`docs/spec/schema.md` §6.5–§6.6): given an
//! [`L2Position`] and the [`Schema`], build the **schema-legal** [`BitMask`] over
//! the model vocabulary that the per-step mask is intersected with.
//!
//! The mask is built so L2 **only clears bits, never sets** them: every token the
//! rule does not specifically constrain is *kept* (its bit set), and the reserved
//! EOS bit is always kept so a complete query stays completable (§4.3 guard).
//! Intersecting such a mask can only remove admissible tokens — the structural
//! `L2 ⊆ L1` guarantee (§6, G4) is a property of the operation, not merely a test.
//!
//! The identifier/string rules (N3, N1/N2, N6) narrow over **reachable byte
//! prefixes**, not whole classified lexemes: a token is kept while it can still
//! *extend some* legal name from the bytes emitted since the anchor (a
//! [`Trie`] walk). This is what makes the overlay sound under byte-level BPE,
//! where a schema identifier arrives in fragments (adversarial-review B1). The
//! type rule (T1) narrows by literal class, which BPE does not fragment.
//!
//! Only the shipped rules build a constraining mask (N3, N1/N2, N6, T1). Every
//! other position returns [`None`] — the mask passes through unchanged.

use std::collections::HashMap;

use crate::grammar::pda::{is_ident_start, is_ident_tail};
use crate::mask::BitMask;
use crate::schema::model::{Schema, TypeClass};
use crate::schema::scope::{L2Position, Lexeme, classify};
use crate::schema::trie::{Trie, Walk, walk};
use crate::vocab::Vocab;

/// The `let` binder keyword, legal at a block-statement source position alongside
/// a real pipeline source (§5.4). N3 admits it so a block query's `let` is not
/// mistaken for a phantom class.
const LET_KEYWORD: &str = "let";

/// The memoized schema-legal masks (`docs/spec/schema.md` §4.5). Building a
/// rule's mask scans the whole vocabulary; at the **anchor** (no bytes emitted
/// yet, the common case) that scan is a per-`(schema, rule)` constant, so it is
/// computed once and copied thereafter. Mid-identifier cursors (bytes already
/// emitted) are rarer and short, so they fall back to a live walk rather than
/// growing the key space.
#[derive(Debug, Default, Clone)]
pub(crate) struct NarrowCache {
    /// T1's operand lever — a whole-vocab literal-class mask, cursor-independent.
    operand: HashMap<CacheKey, BitMask>,
    /// The trie rules (N3, N1/N2, N6). The built trie depends only on the schema
    /// and rule; only the walk cursor moves with the emitted prefix, so the trie is
    /// built once per `(schema, rule)` and its per-cursor-node masks are memoized —
    /// a continuation sub-token re-walks an existing trie instead of rebuilding it,
    /// and a recurring cursor (the anchor most of all) copies its memoized mask
    /// instead of re-scanning the whole vocabulary (§4.5, M3-perf).
    tries: HashMap<CacheKey, RuleCache>,
}

/// A per-`(schema, rule)` built trie plus the masks memoized per cursor node. The
/// anchor mask is simply the `root` cursor's entry, so the earlier separate anchor
/// cache collapses into this one memo.
#[derive(Debug, Clone)]
struct RuleCache {
    trie: Trie,
    kind: TrieKind,
    masks: HashMap<u32, BitMask>,
}

/// The identity of an anchor mask: what determines the schema-legal set when no
/// bytes have been emitted yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CacheKey {
    /// N3 source set — a schema constant.
    Source,
    /// N1/N2 member set of a class — one per class.
    Member(String),
    /// T1 operand class — the literal-class lever (cursor-independent).
    ReValue(TypeClass),
    /// N6 column set at a given emitted-column count (monotonic within a stream,
    /// so the count pins the set exactly).
    Column(usize),
    /// N6 arm-R bare-ident column set at a given emitted-column count — the
    /// unquoted dual of [`Column`](CacheKey::Column) (a distinct trie kind, so it
    /// needs its own key).
    RelationColumn(usize),
    /// N1/N2 member set of a class narrowed over a *fused* nav-dot token (`.<member>`
    /// in one BPE token). Same trie as [`Member`](CacheKey::Member), but the
    /// candidate/keep rule differs (it strips a leading `.`), so the per-node mask
    /// memo is distinct.
    FusedMember(String),
    /// N6 bare-ident column set narrowed over a fused nav-dot token (`.<col>`) — the
    /// fused dual of [`RelationColumn`](CacheKey::RelationColumn).
    FusedRelationColumn(usize),
}

impl NarrowCache {
    /// A fresh, empty cache.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop every memoized mask (on session [`reset`](crate::DecoderSession::reset)):
    /// the emitted-column sets a `Column` key pins are stream-local.
    pub(crate) fn clear(&mut self) {
        self.operand.clear();
        self.tries.clear();
    }
}

/// Refill the caller's reused `dst` buffer with the schema-legal set for `pos`,
/// returning `true` when a constraint applied (the caller intersects `dst` into
/// the L1 mask) or `false` when the position carries no L2 constraint (the L1
/// mask passes through untouched).
///
/// `prefix` is the identifier/string bytes emitted since the anchor (empty at the
/// anchor itself); the trie rules walk it to a cursor node and narrow the
/// continuation from there, so narrowing persists across BPE sub-tokens. `dst` is
/// the session's own buffer, sized to
/// [`mask_len`](crate::grammar::compiled::CompiledGrammar::mask_len), so narrowing
/// allocates no per-step mask (§4.3). `columns` is the N6 legal column set (the
/// tracker's emitted-string superset); it is ignored for other rules.
// Each argument is a distinct, documented input to the per-step narrower (output
// buffer, memo, schema, position, emitted-prefix, column set, vocab, EOS bit);
// bundling them into a context struct would add indirection to the hot path for
// no clarity gain, so the count is accepted here rather than silenced globally.
#[allow(clippy::too_many_arguments)]
pub(crate) fn narrow_into(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    schema: &Schema,
    pos: &L2Position,
    prefix: &[u8],
    columns: &[Vec<u8>],
    vocab: &Vocab,
    eos_bit: u32,
) -> bool {
    match pos {
        L2Position::None => false,
        L2Position::ReValue(TypeClass::Boolean | TypeClass::Temporal) => {
            // Boolean/temporal operand narrowing is deferred (§6.6 T1 ships only
            // the string/numeric levers) — keep the L1 mask unchanged.
            false
        }
        L2Position::ReValue(tc) => {
            let masked_by = *tc;
            with_cache(dst, cache, CacheKey::ReValue(masked_by), |dst| {
                fill_operand(dst, vocab, eos_bit, masked_by);
            });
            true
        }
        L2Position::SourceIdent => narrow_trie(
            dst,
            cache,
            CacheKey::Source,
            prefix,
            TrieKind::Ident,
            vocab,
            eos_bit,
            || Trie::from_names(schema.source_paths().chain(std::iter::once(LET_KEYWORD))),
        ),
        L2Position::Member(class) => narrow_trie(
            dst,
            cache,
            CacheKey::Member(class.clone()),
            prefix,
            TrieKind::Ident,
            vocab,
            eos_bit,
            || Trie::from_names(schema.member_names(class)),
        ),
        L2Position::Column => narrow_trie(
            dst,
            cache,
            CacheKey::Column(columns.len()),
            prefix,
            TrieKind::Str,
            vocab,
            eos_bit,
            || Trie::from_names(columns.iter().map(|c| quote(c))),
        ),
        L2Position::RelationColumn => narrow_trie(
            dst,
            cache,
            CacheKey::RelationColumn(columns.len()),
            prefix,
            TrieKind::Ident,
            vocab,
            eos_bit,
            || Trie::from_names(columns.iter().cloned()),
        ),
    }
}

/// Refill `dst` with the schema-legal set for a **fused nav-dot** token — one where
/// byte-level BPE packs the navigation `.` and the property/column's first byte into
/// a single token (`.theme`, `.zzz`). The ordinary [`narrow_into`] mask is read at
/// the anchor *before* the dot, where the member/column position is not yet active,
/// and even where it is active its candidate rule treats the token's first byte as
/// the identifier's first byte — so a `.`-led token slips through unnarrowed
/// (`docs/spec/schema.md` §6.5). This pass closes that gap: it clears exactly the
/// `.`-led tokens whose post-dot identifier begins no legal name, and touches
/// nothing else. It is a second, purely subtractive intersect the session applies on
/// top of the ordinary narrow: it only ever *clears* bits — precisely the fused
/// phantoms the anchor-read narrow lets through — so the result stays a subset of L1
/// and can never widen it.
///
/// `pos` is the *post-dot* target a following `.` would navigate to
/// ([`Member`](L2Position::Member) or [`RelationColumn`](L2Position::RelationColumn),
/// from [`ScopeTracker::fused_nav_position`](crate::schema::scope::ScopeTracker));
/// any other position yields no fused constraint.
// Mirrors [`narrow_into`]'s per-step input signature (output buffer, memo, schema,
// position, column set, vocab, EOS bit); a context struct would add hot-path
// indirection for no clarity gain, so the count is accepted here as it is there.
#[allow(clippy::too_many_arguments)]
pub(crate) fn narrow_fused_into(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    schema: &Schema,
    pos: &L2Position,
    columns: &[Vec<u8>],
    vocab: &Vocab,
    eos_bit: u32,
) -> bool {
    match pos {
        L2Position::Member(class) => narrow_fused_trie(
            dst,
            cache,
            CacheKey::FusedMember(class.clone()),
            vocab,
            eos_bit,
            || Trie::from_names(schema.member_names(class)),
        ),
        L2Position::RelationColumn => narrow_fused_trie(
            dst,
            cache,
            CacheKey::FusedRelationColumn(columns.len()),
            vocab,
            eos_bit,
            || Trie::from_names(columns.iter().cloned()),
        ),
        _ => false,
    }
}

/// Build (or reuse) the rule's trie and fill/memoize the fused-token mask at its
/// root. The trie content matches the post-dot [`narrow_trie`] rule; only the
/// keep-rule differs ([`fill_fused_trie`]), so the memo lives under a distinct
/// `Fused*` [`CacheKey`] rather than colliding with the post-dot per-cursor masks.
fn narrow_fused_trie(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    key: CacheKey,
    vocab: &Vocab,
    eos_bit: u32,
    build: impl FnOnce() -> Trie,
) -> bool {
    let entry = cache.tries.entry(key).or_insert_with(|| RuleCache {
        trie: build(),
        kind: TrieKind::Ident,
        masks: HashMap::new(),
    });
    let root = entry.trie.root();
    if let Some(cached) = entry.masks.get(&root) {
        dst.copy_from(cached);
    } else {
        fill_fused_trie(dst, vocab, eos_bit, &entry.trie);
        entry.masks.insert(root, dst.clone());
    }
    true
}

/// Fill `dst` for a fused nav-dot pass: clear a token only when it is a fused
/// `.<ident>` (a leading `.` immediately followed by an identifier byte) whose
/// post-dot identifier walks off every legal name from the trie root. Every other
/// token — a bare `.`, a quoted member (`.'name'`), an operator, a non-`.` token —
/// is kept, so L2 never masks a token L1 admits outside the phantom class this pass
/// targets. The reserved EOS bit is always kept (§4.3).
fn fill_fused_trie(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, trie: &Trie) {
    dst.clear_all();
    let root = trie.root();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        let keep = match fused_post_dot(bytes) {
            Some(rest) => !matches!(walk(trie, root, rest), Walk::Diverge),
            None => true,
        };
        if keep {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// The post-dot bytes of a fused nav-dot identifier token — `Some(rest)` when
/// `bytes` is a leading `.` followed by an identifier-**start** byte (so `rest` is
/// the property/column the dot navigates into), else `None` (a bare `.`, a quoted or
/// whitespace-led member, or any non-navigation token, all left untouched).
///
/// The identifier-*start* gate (not merely ident-tail) is load-bearing for
/// soundness: a member/column name always starts with a letter or `_`, and after a
/// navigation `.` the byte-PDA is in `AfterDot`, which requires exactly that. A
/// *value*-position leading-dot float (`.5`) instead routes through `NeedFracDigit`
/// (its first byte is a digit), so excluding digit-led rests guarantees this pass
/// can only ever clear a genuine member navigation, never a float literal — even if
/// the pre-dot scope target were stale for a bare `$var` operand.
fn fused_post_dot(bytes: &[u8]) -> Option<&[u8]> {
    match bytes {
        [b'.', rest @ ..] if rest.first().is_some_and(|&b| is_ident_start(b)) => Some(rest),
        _ => None,
    }
}

/// Narrow `dst` by a trie rule: build (or reuse) the rule's trie, walk `prefix` to
/// its cursor node, then copy the memoized mask for that cursor or fill and
/// memoize it. The trie is cursor-independent, so it is built once per key; only
/// the cursor moves with the emitted prefix.
#[allow(clippy::too_many_arguments)]
fn narrow_trie(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    key: CacheKey,
    prefix: &[u8],
    kind: TrieKind,
    vocab: &Vocab,
    eos_bit: u32,
    build: impl FnOnce() -> Trie,
) -> bool {
    let entry = cache.tries.entry(key).or_insert_with(|| RuleCache {
        trie: build(),
        kind,
        masks: HashMap::new(),
    });
    let cursor = if prefix.is_empty() {
        entry.trie.root()
    } else {
        match walk(&entry.trie, entry.trie.root(), prefix) {
            Walk::Stay(cursor) => cursor,
            // The prefix already completed a legal name or diverged — the lexeme is
            // done (or was never legal); leave the L1 mask unchanged.
            Walk::Complete | Walk::Diverge => return false,
        }
    };
    if let Some(cached) = entry.masks.get(&cursor) {
        dst.copy_from(cached);
    } else {
        fill_trie(dst, vocab, eos_bit, &entry.trie, cursor, entry.kind);
        entry.masks.insert(cursor, dst.clone());
    }
    true
}

/// Look `key` up in the operand cache; on a hit copy the memoized mask into `dst`,
/// on a miss run `fill` and memoize it. Only T1's cursor-independent `ReValue`
/// lever reaches here; the trie rules memoize per cursor node in `narrow_trie`.
fn with_cache(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    key: CacheKey,
    fill: impl FnOnce(&mut BitMask),
) {
    if let Some(cached) = cache.operand.get(&key) {
        dst.copy_from(cached);
        return;
    }
    fill(dst);
    cache.operand.insert(key, dst.clone());
}

/// Double `'` to `''` and wrap in quotes — the raw bytes the model emits for a
/// column string (§5.5), so the N6 trie is walked byte-exact against the tracker's
/// byte-exact emitted set.
fn quote(content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 2);
    out.push(b'\'');
    for &b in content {
        if b == b'\'' {
            out.push(b'\'');
        }
        out.push(b);
    }
    out.push(b'\'');
    out
}

/// Which lexeme a trie rule governs — decides whether a vocab token is a
/// *candidate* the trie may clear, or a structural token it never touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrieKind {
    /// An identifier / classpath (N3, N1/N2): a candidate token starts with an
    /// identifier-tail byte.
    Ident,
    /// A quoted string (N6): a candidate token opens a string (`'`) or continues
    /// one already in flight.
    Str,
}

/// Whether an operand token is kept under a T1 constraint with LHS class `lhs`
/// (§6.6 T1). A literal of a *disjoint* class is cleared; everything else — a
/// matching literal, an identifier, a `$var` navExpr, a delimiter — is kept, so
/// only a genuine type mismatch is masked.
fn keeps_operand(lex: &Lexeme, lhs: TypeClass) -> bool {
    match lex {
        Lexeme::Str(_) => matches!(lhs, TypeClass::Str),
        Lexeme::Number => matches!(lhs, TypeClass::Numeric),
        Lexeme::Date => matches!(lhs, TypeClass::Temporal),
        _ => true,
    }
}

/// Refill `dst` with the T1 operand set for LHS class `masked_by`, plus EOS.
fn fill_operand(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, masked_by: TypeClass) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_operand(&classify(bytes), masked_by) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Refill `dst` from a trie walk: keep the reserved EOS bit and every vocab token
/// that is either a *non-candidate* (a structural/whitespace token the rule does
/// not govern) or a candidate that can still reach a legal name from `cursor`.
fn fill_trie(
    dst: &mut BitMask,
    vocab: &Vocab,
    eos_bit: u32,
    trie: &Trie,
    cursor: u32,
    kind: TrieKind,
) {
    dst.clear_all();
    let mid = cursor != trie.root();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        let keep = if is_candidate(bytes, kind, mid) {
            !matches!(walk(trie, cursor, bytes), Walk::Diverge)
        } else {
            // A structural token (whitespace, operator, delimiter) is not the
            // identifier/string the trie governs — it is kept exactly as the
            // whole-lexeme narrower kept every non-`Ident`/`Str` lexeme, so L2
            // never masks a token L1 would allow through a boundary.
            true
        };
        if keep {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Whether `bytes` is a token the `kind` trie may clear: an identifier-tail start
/// for an `Ident` rule, or a string opener (or any byte once a string is in
/// flight) for a `Str` rule.
fn is_candidate(bytes: &[u8], kind: TrieKind, mid_lexeme: bool) -> bool {
    match bytes.first() {
        None => false,
        Some(&first) => match kind {
            TrieKind::Ident => is_ident_tail(first),
            TrieKind::Str => first == b'\'' || mid_lexeme,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{NarrowCache, narrow_into};
    use crate::grammar::compiled::CompiledGrammar;
    use crate::mask::BitMask;
    use crate::schema::model::{Schema, TypeClass};
    use crate::schema::scope::L2Position;
    use crate::vocab::Vocab;

    const SAMPLE: &str = r#"{
      "db_id": "d", "db_path": "spider::d::Db",
      "classes": { "A": { "simple_name": "A",
        "properties": [{"name": "countryName", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
                       {"name": "country", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}] } },
      "associations": [], "enums": {}
    }"#;

    fn schema() -> Schema {
        Schema::from_json(SAMPLE).expect("parses")
    }

    /// A vocabulary whose tokens span every lexeme class the rules distinguish.
    fn vocab(tokens: &[&[u8]]) -> Vocab {
        let owned: Vec<Vec<u8>> = tokens.iter().map(|t| t.to_vec()).collect();
        Vocab::from_byte_tokens(owned, tokens.len() as u32)
    }

    fn bit(mask: &BitMask, id: u32) -> bool {
        mask.test(id)
    }

    /// Narrow a fresh buffer for `pos` over `v` at cursor `prefix`, routing the
    /// mask length and EOS bit through the grammar's single source exactly as the
    /// session does. Returns whether a constraint applied and the filled buffer.
    fn run_prefix(pos: &L2Position, cols: &[Vec<u8>], prefix: &[u8], v: Vocab) -> (bool, BitMask) {
        let grammar = CompiledGrammar::compile(v);
        let mut mask = BitMask::with_len(grammar.mask_len());
        let mut cache = NarrowCache::new();
        let applied = narrow_into(
            &mut mask,
            &mut cache,
            &schema(),
            pos,
            prefix,
            cols,
            grammar.vocab(),
            grammar.eos_bit(),
        );
        (applied, mask)
    }

    fn run(pos: &L2Position, cols: &[Vec<u8>], v: Vocab) -> (bool, BitMask) {
        run_prefix(pos, cols, b"", v)
    }

    /// Run the fused nav-dot pass for `pos` over `v`, routing mask length and EOS
    /// through the grammar exactly as the session does.
    fn run_fused(pos: &L2Position, cols: &[Vec<u8>], v: Vocab) -> (bool, BitMask) {
        let grammar = CompiledGrammar::compile(v);
        let mut mask = BitMask::with_len(grammar.mask_len());
        let mut cache = NarrowCache::new();
        let applied = super::narrow_fused_into(
            &mut mask,
            &mut cache,
            &schema(),
            pos,
            cols,
            grammar.vocab(),
            grammar.eos_bit(),
        );
        (applied, mask)
    }

    #[test]
    fn none_position_yields_no_mask() {
        assert!(!run(&L2Position::None, &[], vocab(&[b"x"])).0);
    }

    #[test]
    fn fused_member_masks_a_dotted_phantom_but_keeps_real_navigations() {
        // class A members: {country, countryName} → legal post-dot first char {c}.
        let v = vocab(&[
            b".country",   // 0: fused real member — kept
            b".c",         // 1: fused legal prefix — kept
            b".zzz",       // 2: fused phantom (no `z…` member) — masked
            b".maker",     // 3: fused phantom (no `m…` member) — masked
            b".",          // 4: bare dot (member opens next token) — kept
            b".'country'", // 5: fused *quoted* member — kept (not ident-led)
            b".5",         // 6: leading-dot float — kept (digit-led, not a member)
            b"country",    // 7: non-dot ident (refvar continuation) — kept
            b"->filter",   // 8: non-dot structural — kept
        ]);
        let (applied, mask) = run_fused(&L2Position::Member("A".to_owned()), &[], v);
        assert!(applied);
        for id in [0u32, 1, 4, 5, 6, 7, 8] {
            assert!(bit(&mask, id), "token {id} must be kept");
        }
        assert!(!bit(&mask, 2), "fused phantom `.zzz` must be masked");
        assert!(!bit(&mask, 3), "fused phantom `.maker` must be masked");
    }

    #[test]
    fn fused_relation_column_masks_a_dotted_phantom_column() {
        // arm-R dual: `$row.<col>` fused. Emitted columns = {Cyl}.
        let v = vocab(&[b".Cyl", b".Zzz", b".", b"Cyl"]);
        let cols = [b"Cyl".to_vec()];
        let (applied, mask) = run_fused(&L2Position::RelationColumn, &cols, v);
        assert!(applied);
        assert!(bit(&mask, 0), "fused emitted column `.Cyl` kept");
        assert!(!bit(&mask, 1), "fused phantom column `.Zzz` masked");
        assert!(bit(&mask, 2), "bare dot kept");
        assert!(bit(&mask, 3), "non-dot token kept");
    }

    #[test]
    fn fused_pass_is_inert_for_non_navigable_positions() {
        // Only Member / RelationColumn are fused-navigable; every other position
        // applies no fused constraint (the ordinary narrow handles them).
        for pos in [
            L2Position::None,
            L2Position::SourceIdent,
            L2Position::Column,
            L2Position::ReValue(TypeClass::Numeric),
        ] {
            assert!(
                !run_fused(&pos, &[], vocab(&[b".zzz"])).0,
                "{pos:?} must apply no fused constraint"
            );
        }
    }

    #[test]
    fn deferred_operand_classes_pass_through() {
        assert!(
            !run(
                &L2Position::ReValue(TypeClass::Boolean),
                &[],
                vocab(&[b"x"])
            )
            .0
        );
        assert!(
            !run(
                &L2Position::ReValue(TypeClass::Temporal),
                &[],
                vocab(&[b"x"])
            )
            .0
        );
    }

    #[test]
    fn source_ident_keeps_classes_the_store_and_let_masks_phantoms() {
        // ids: 0 real class, 1 store, 2 `let`, 3 phantom, 4 a non-identifier `(`.
        let v = vocab(&[b"A", b"spider::d::Db", b"let", b"Nope", b"("]);
        let eos = v.len() as u32;
        let (applied, mask) = run(&L2Position::SourceIdent, &[], v);
        assert!(applied);
        assert!(bit(&mask, 0) && bit(&mask, 1) && bit(&mask, 2));
        assert!(!bit(&mask, 3), "a phantom class is masked");
        assert!(bit(&mask, 4), "a non-identifier token is never touched");
        assert!(bit(&mask, eos), "EOS is always kept");
    }

    #[test]
    fn source_ident_keeps_a_leading_bpe_prefix() {
        // The B1 case: the leading sub-token of a fragmented classpath. `spide` is
        // a strict prefix of the store/class paths — it must survive; `Xy` (off
        // every source) must not.
        let v = vocab(&[b"spide", b"Xy"]);
        let (_applied, mask) = run(&L2Position::SourceIdent, &[], v);
        assert!(bit(&mask, 0), "a leading classpath prefix survives");
        assert!(!bit(&mask, 1), "a prefix off every source is masked");
    }

    #[test]
    fn member_masks_a_non_member_ident_but_keeps_structure() {
        let v = vocab(&[b"country", b"phantom", b"."]);
        let (applied, mask) = run(&L2Position::Member("A".to_owned()), &[], v);
        assert!(applied);
        assert!(bit(&mask, 0), "a real member survives");
        assert!(!bit(&mask, 1), "a phantom member is masked");
        assert!(bit(&mask, 2), "a non-identifier token is kept");
    }

    #[test]
    fn member_keeps_a_leading_prefix_then_narrows_the_continuation() {
        // `countryName` fragments to `country` + `Name`. From the anchor the
        // leading `count` survives; after emitting `country`, the continuation
        // `Name` still reaches `countryName`, but `Xyz` does not.
        let lead = vocab(&[b"count", b"Zzz"]);
        let (_a, mask) = run(&L2Position::Member("A".to_owned()), &[], lead);
        assert!(bit(&mask, 0), "the leading BPE prefix survives");
        assert!(!bit(&mask, 1), "a prefix off every member is masked");

        let cont = vocab(&[b"Name", b"Xyz"]);
        let (_b, mask) = run_prefix(&L2Position::Member("A".to_owned()), &[], b"country", cont);
        assert!(
            bit(&mask, 0),
            "a continuation reaching a longer member survives"
        );
        assert!(!bit(&mask, 1), "a continuation off every member is masked");
    }

    #[test]
    fn a_completed_prefix_stops_narrowing() {
        // Once the emitted bytes are a whole member followed by a boundary, the
        // lexeme is done — no further narrowing applies (pass-through).
        let v = vocab(&[b"anything"]);
        let (applied, _mask) = run_prefix(&L2Position::Member("A".to_owned()), &[], b"country.", v);
        assert!(!applied, "a completed name stops the narrower");
    }

    #[test]
    fn revalue_masks_the_disjoint_literal_class_only() {
        let (applied_n, numeric) = run(
            &L2Position::ReValue(TypeClass::Numeric),
            &[],
            vocab(&[b"'x'", b"5", b"%2018-01-01", b"foo"]),
        );
        assert!(applied_n);
        assert!(
            !bit(&numeric, 0),
            "a string literal is masked for a numeric LHS"
        );
        assert!(bit(&numeric, 1), "a number literal matches");
        assert!(
            !bit(&numeric, 2),
            "a date literal is masked for a numeric LHS"
        );
        assert!(
            bit(&numeric, 3),
            "a navExpr operand is never masked by type"
        );
        let (applied_s, string) = run(
            &L2Position::ReValue(TypeClass::Str),
            &[],
            vocab(&[b"'x'", b"5", b"%2018-01-01", b"foo"]),
        );
        assert!(applied_s);
        assert!(bit(&string, 0), "a string literal matches");
        assert!(
            !bit(&string, 1),
            "a number literal is masked for a string LHS"
        );
    }

    #[test]
    fn column_keeps_emitted_names_and_masks_the_rest() {
        let v = vocab(&[b"'cnt'", b"'ghost'", b"getInteger"]);
        let cols = [b"cnt".to_vec()];
        let (applied, mask) = run(&L2Position::Column, &cols, v);
        assert!(applied);
        assert!(bit(&mask, 0), "an emitted column survives");
        assert!(!bit(&mask, 1), "an unemitted column string is masked");
        assert!(bit(&mask, 2), "a non-string token is kept");
    }

    #[test]
    fn relation_column_keeps_emitted_bare_idents_and_masks_the_rest() {
        // The arm-R dual of `column_*`: a bare-ident column access `$row.Col` is
        // narrowed against the raw (unquoted) emitted-column universe.
        let v = vocab(&[b"Cyl", b"Zzz", b"."]);
        let cols = [b"Cyl".to_vec()];
        let (applied, mask) = run(&L2Position::RelationColumn, &cols, v);
        assert!(applied);
        assert!(bit(&mask, 0), "an emitted column survives");
        assert!(!bit(&mask, 1), "a phantom column ident is masked");
        assert!(bit(&mask, 2), "a non-identifier token is kept");
    }

    #[test]
    fn relation_column_keeps_a_leading_bpe_prefix_then_narrows() {
        // A fragmented bare-ident column `Cyl` → `Cy` / `l`: the leading sub-token
        // survives at the anchor; mid-ident the continuation is narrowed to the set.
        let cols = [b"Cyl".to_vec()];
        let (_a, mask) = run(&L2Position::RelationColumn, &cols, vocab(&[b"Cy", b"Zz"]));
        assert!(bit(&mask, 0), "a leading column prefix survives");
        assert!(!bit(&mask, 1), "a prefix off every column is masked");
        let (_b, mask) = run_prefix(
            &L2Position::RelationColumn,
            &cols,
            b"Cy",
            vocab(&[b"l", b"x"]),
        );
        assert!(bit(&mask, 0), "the emitted column body survives");
        assert!(!bit(&mask, 1), "a divergent continuation is masked");
    }

    #[test]
    fn column_keeps_a_leading_quote_then_narrows_the_body() {
        // A column string `'cnt'` fragments to `'` / `cnt` / `'`. The opening quote
        // survives at the anchor; mid-string, the body is narrowed to the emitted
        // set.
        let cols = [b"cnt".to_vec()];
        let (_a, mask) = run(&L2Position::Column, &cols, vocab(&[b"'", b"getInteger"]));
        assert!(bit(&mask, 0), "the opening quote survives");
        assert!(bit(&mask, 1), "a non-string token is untouched");
        let (_b, mask) = run_prefix(
            &L2Position::Column,
            &cols,
            b"'",
            vocab(&[b"cnt'", b"ghost'"]),
        );
        assert!(bit(&mask, 0), "the emitted column body survives");
        assert!(
            !bit(&mask, 1),
            "an unemitted column body is masked mid-string"
        );
    }

    #[test]
    fn the_anchor_mask_is_cached_and_reused() {
        // A second narrow at the same anchor key must produce the identical mask
        // (the cache copy), and a fresh cache the same result — so caching is a
        // pure memo, not a behaviour change.
        let grammar = CompiledGrammar::compile(vocab(&[b"country", b"phantom"]));
        let mut cache = NarrowCache::new();
        let mut first = BitMask::with_len(grammar.mask_len());
        let mut second = BitMask::with_len(grammar.mask_len());
        let pos = L2Position::Member("A".to_owned());
        for dst in [&mut first, &mut second] {
            narrow_into(
                dst,
                &mut cache,
                &schema(),
                &pos,
                b"",
                &[],
                grammar.vocab(),
                grammar.eos_bit(),
            );
        }
        assert_eq!(
            first, second,
            "the cached mask equals the freshly built one"
        );
        cache.clear();
        let mut fresh = BitMask::with_len(grammar.mask_len());
        narrow_into(
            &mut fresh,
            &mut cache,
            &schema(),
            &pos,
            b"",
            &[],
            grammar.vocab(),
            grammar.eos_bit(),
        );
        assert_eq!(first, fresh, "clearing the cache rebuilds the same mask");

        // A **mid-cursor** prefix reuses the same per-`(schema, rule)` trie and
        // memoizes the cursor node's mask: a second narrow at the same prefix must
        // equal the first, and equal a fresh-cache narrow — the memo is a pure
        // function of `(trie, cursor)`, behaviour-preserving, not just the anchor.
        let member = vocab(&[b"Name", b"Xyz"]);
        let cont_grammar = CompiledGrammar::compile(member);
        let mut warm = NarrowCache::new();
        let mut mid_a = BitMask::with_len(cont_grammar.mask_len());
        let mut mid_b = BitMask::with_len(cont_grammar.mask_len());
        for dst in [&mut mid_a, &mut mid_b] {
            narrow_into(
                dst,
                &mut warm,
                &schema(),
                &pos,
                b"country",
                &[],
                cont_grammar.vocab(),
                cont_grammar.eos_bit(),
            );
        }
        assert_eq!(
            mid_a, mid_b,
            "the memoized mid-cursor mask is reused verbatim"
        );
        let mut cold = NarrowCache::new();
        let mut mid_fresh = BitMask::with_len(cont_grammar.mask_len());
        narrow_into(
            &mut mid_fresh,
            &mut cold,
            &schema(),
            &pos,
            b"country",
            &[],
            cont_grammar.vocab(),
            cont_grammar.eos_bit(),
        );
        assert_eq!(
            mid_a, mid_fresh,
            "a cold-cache mid-cursor narrow equals the warm-cache one"
        );
    }

    #[test]
    fn clear_drops_stale_stream_local_column_masks() {
        // A `Column` key pins the mask on the emitted-column *set*, which is
        // stream-local (both streams below emit one column, so both hit key
        // `Column(1)`). On session reset `clear` must drop the first stream's set,
        // or the second stream's different column returns the stale mask. A no-op
        // `clear` returns the first stream's `'cnt'` mask and fails here.
        let grammar = CompiledGrammar::compile(vocab(&[b"'cnt'", b"'ghost'"]));
        let mut cache = NarrowCache::new();
        let pos = L2Position::Column;

        let mut first = BitMask::with_len(grammar.mask_len());
        narrow_into(
            &mut first,
            &mut cache,
            &schema(),
            &pos,
            b"",
            &[b"cnt".to_vec()],
            grammar.vocab(),
            grammar.eos_bit(),
        );
        assert!(
            bit(&first, 0) && !bit(&first, 1),
            "the first stream keeps 'cnt' and masks 'ghost'"
        );

        cache.clear();

        let mut second = BitMask::with_len(grammar.mask_len());
        narrow_into(
            &mut second,
            &mut cache,
            &schema(),
            &pos,
            b"",
            &[b"ghost".to_vec()],
            grammar.vocab(),
            grammar.eos_bit(),
        );
        assert!(
            bit(&second, 1) && !bit(&second, 0),
            "after clear the second stream keeps 'ghost' and masks 'cnt' (a no-op clear would return the stale 'cnt' mask)"
        );
    }
}
