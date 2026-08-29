//! The L2 narrowing rules (`docs/spec/schema.md` §6.5–§6.6): given an
//! [`L2Position`] and the [`Schema`], build the **schema-legal** [`BitMask`] over
//! the model vocabulary that the per-step mask is intersected with.
//!
//! The mask is built so L2 **only clears bits, never sets** them: every token the
//! rule does not specifically constrain is *kept* (its bit set). Intersecting such
//! a mask can only remove admissible tokens — the structural `L2 ⊆ L1` guarantee
//! (§6, G4) is a property of the operation, not merely a test.
//!
//! The reserved EOS bit is kept wherever the overlay can see no reason to forbid
//! ending the stream, and cleared where it can: a trie rule with its lexeme open
//! on a **strict prefix** of a legal name, or on a whole name whose call parens
//! are mandatory ([`NameClose::MustCall`]). [`admits_eos`] exposes that same
//! verdict to [`is_complete`](crate::DecoderSession::is_complete), which would
//! otherwise call a stream ending mid-identifier complete on L1 lookahead alone.
//!
//! The identifier/string rules (N3, N1/N2, N6) narrow over **reachable byte
//! prefixes**, not whole classified lexemes: a token is kept while it can still
//! *extend some* legal name from the bytes emitted since the anchor (a
//! [`Trie`] walk). This is what makes the overlay sound under byte-level BPE,
//! where a schema identifier arrives in fragments (adversarial-review B1). The
//! type rules (T1, T2, T3) narrow by literal/operator/reducer class, which BPE
//! does not fragment.
//!
//! Only the shipped rules build a constraining mask (N3, N1/N2, N6, T1, T2,
//! T3). Every other position returns [`None`] — the mask passes through
//! unchanged.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::grammar::pda::{is_ident_start, is_ident_tail};
use crate::mask::BitMask;
use crate::schema::model::{Schema, TypeClass};
use crate::schema::scope::{
    EXTENT_INCOMPATIBLE_METHODS, L2Position, Lexeme, SOURCE_METHOD, STORE_METHODS, classify,
};
use crate::schema::trie::{NameClose, NameShape, Trie, Walk, walk};
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
    /// instead of re-scanning the whole vocabulary (§4.5).
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
    /// S1 source-method set — always the single name [`SOURCE_METHOD`].
    SourceMethod,
    /// N3c's store-method set — always [`STORE_METHODS`].
    StoreMethod,
    /// The source method's argument-position set — a whole-vocab constant
    /// (independent of schema, class, or emitted prefix), so one key suffices.
    SourceMethodArg,
    /// N3d's store-method argument-slot set — likewise a whole-vocab constant.
    StoreMethodArg,
    /// N3d's store-method separator set. Whether the call still owes another
    /// argument is the whole identity of the set (`,` while arguments remain,
    /// the call's own `)` once the arity is met), so it keys the memo exactly.
    StoreMethodArgSep(bool),
    /// N3e's class-extent continuation set, before vs. after the `-` that opens a
    /// step arrow — two whole-vocab constants.
    SourceExtent(bool),
    /// N3f's extent-method set at a given cursor in the deny trie. The trie is a
    /// whole-vocab constant ([`EXTENT_INCOMPATIBLE_METHODS`]), so the cursor node
    /// is the entire identity of the mask — exactly as a trie rule's per-node memo
    /// is keyed, but the trie lives in [`EXTENT_DENY`] rather than in a
    /// `RuleCache`, because this rule *clears* names instead of permitting them.
    ExtentMethod(u32),
    /// N3g's receiver-only argument-slot set — a whole-vocab constant, exactly
    /// like [`SourceMethodArg`](CacheKey::SourceMethodArg)'s.
    ReceiverOnlyArg,
    /// N4a's store-result continuation set, before vs. after the `-` that opens a
    /// step arrow — two whole-vocab constants, exactly like
    /// [`SourceExtent`](CacheKey::SourceExtent)'s.
    StoreResult(bool),
    /// N4c's string-literal operator set, before vs. after the `-` that opens a
    /// step arrow — two whole-vocab constants.
    StrOperator(bool),
    /// N4b's logical-operand set — a whole-vocab constant. Distinct from
    /// [`ReValue(Boolean)`](CacheKey::ReValue) even though the two share a fill:
    /// the T1 memo must stay reachable if `ReValue`'s Boolean arm is ever
    /// enabled, and one key per rule is what keeps the two independently
    /// invalidatable.
    LogicalOperand,
    /// N1/N2 member set of a class — one per class.
    Member(String),
    /// T1 operand class — the literal-class lever (cursor-independent).
    ReValue(TypeClass),
    /// T2 comparator class — the ordered-comparator lever (cursor-independent).
    Comparator(TypeClass),
    /// T6's non-scalar-operand set — a whole-vocab constant (it clears the
    /// ordered comparators outright, with no operand class to key on), so one
    /// key suffices.
    OrderedOperand,
    /// T3 reducer class — the aggregation-reducer lever (cursor-independent).
    Reducer(TypeClass),
    /// N6 column set at a given emitted-column count (monotonic within a stream,
    /// so the count pins the set exactly).
    Column(usize),
    /// N7's continuation set for an open bare value-position identifier — a
    /// whole-vocab constant (it narrows no name set, only what may follow one),
    /// so one key suffices.
    ValueIdent,
    /// N6 arm-R bare-ident column set at a given emitted-column count — the
    /// unquoted dual of [`Column`](CacheKey::Column) (a distinct trie kind, so it
    /// needs its own key).
    RelationColumn(usize),
    /// S2 refVar set at a given bound-variable count. The tracker's binder record
    /// is monotonic within a stream (see `ScopeTracker::bound_vars`), so — exactly
    /// as for [`Column`](CacheKey::Column) — the count pins the set.
    RefVar(usize),
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
    vars: &[String],
    vocab: &Vocab,
    eos_bit: u32,
) -> bool {
    if let Some(rule) = TrieRule::of(pos, schema, columns, vars) {
        return narrow_trie(dst, cache, &rule, prefix, vocab, eos_bit);
    }
    match pos {
        L2Position::ReValue(TypeClass::Boolean | TypeClass::Temporal) => {
            // Boolean/temporal operand narrowing is deferred (§6.6 T1 ships only
            // the string/numeric levers) — keep the L1 mask unchanged.
            false
        }
        L2Position::ReValue(tc) => {
            let masked_by = *tc;
            with_cache(dst, cache, CacheKey::ReValue(masked_by), |dst| {
                fill_operand(dst, vocab, eos_bit, masked_by);
            })
        }
        L2Position::Comparator(TypeClass::Numeric | TypeClass::Temporal) => {
            // An ordered comparator is legal on a numeric/temporal operand — no
            // constraint to apply.
            false
        }
        L2Position::Comparator(tc) => {
            let masked_by = *tc;
            with_cache(dst, cache, CacheKey::Comparator(masked_by), |dst| {
                fill_comparator(dst, vocab, eos_bit, masked_by);
            })
        }
        L2Position::OrderedOperand => with_cache(dst, cache, CacheKey::OrderedOperand, |dst| {
            fill_ordered_operand(dst, vocab, eos_bit);
        }),
        L2Position::Reducer(tc) => {
            let masked_by = *tc;
            with_cache(dst, cache, CacheKey::Reducer(masked_by), |dst| {
                fill_reducer(dst, vocab, eos_bit, masked_by);
            })
        }
        L2Position::SourceMethodArg => with_cache(dst, cache, CacheKey::SourceMethodArg, |dst| {
            fill_source_method_arg(dst, vocab, eos_bit);
        }),
        L2Position::StoreMethodArg => with_cache(dst, cache, CacheKey::StoreMethodArg, |dst| {
            fill_store_method_arg(dst, vocab, eos_bit);
        }),
        L2Position::StoreMethodArgSep { remaining } => {
            let remaining = *remaining;
            with_cache(dst, cache, CacheKey::StoreMethodArgSep(remaining), |dst| {
                fill_store_method_arg_sep(dst, vocab, eos_bit, remaining);
            })
        }
        L2Position::SourceExtent { after_dash } => {
            let after_dash = *after_dash;
            with_cache(dst, cache, CacheKey::SourceExtent(after_dash), |dst| {
                fill_source_extent(dst, vocab, eos_bit, after_dash);
            })
        }
        L2Position::ExtentMethod => {
            let Some(cursor) = extent_deny_cursor(prefix) else {
                return false;
            };
            with_cache(dst, cache, CacheKey::ExtentMethod(cursor), |dst| {
                fill_extent_method(dst, vocab, eos_bit, cursor);
            })
        }
        L2Position::ReceiverOnlyArg => with_cache(dst, cache, CacheKey::ReceiverOnlyArg, |dst| {
            fill_receiver_only_arg(dst, vocab, eos_bit);
        }),
        L2Position::StoreResult { after_dash } => {
            let after_dash = *after_dash;
            with_cache(dst, cache, CacheKey::StoreResult(after_dash), |dst| {
                fill_after_completed_term(
                    dst,
                    vocab,
                    eos_bit,
                    after_dash,
                    STORE_RESULT_DENIED_OPENERS,
                );
            })
        }
        L2Position::StrOperator { after_dash } => {
            let after_dash = *after_dash;
            with_cache(dst, cache, CacheKey::StrOperator(after_dash), |dst| {
                fill_after_completed_term(
                    dst,
                    vocab,
                    eos_bit,
                    after_dash,
                    STR_OPERATOR_DENIED_OPENERS,
                );
            })
        }
        L2Position::LogicalOperand => with_cache(dst, cache, CacheKey::LogicalOperand, |dst| {
            fill_operand(dst, vocab, eos_bit, TypeClass::Boolean);
        }),
        L2Position::ValueIdent => with_cache(dst, cache, CacheKey::ValueIdent, |dst| {
            fill_value_ident(dst, vocab);
        }),
        // Every trie position already returned above. They are spelled out here
        // rather than swept up by a `_` arm so this match stays **exhaustive**
        // over [`L2Position`]: a new variant then has to be classified here
        // instead of silently passing through, and — the reason this is not
        // merely style — deleting any arm of an exhaustive match does not
        // compile, which is what keeps the arms that guard a *no-op* narrow
        // (`Comparator(Numeric | Temporal)`, whose fall-through builds an
        // all-ones mask that intersects to nothing) from becoming
        // behaviourally-equivalent mutants no test can kill.
        L2Position::None
        | L2Position::SourceIdent
        | L2Position::SourceMethod
        | L2Position::StoreMethod
        | L2Position::Member(_)
        | L2Position::Column
        | L2Position::RelationColumn
        | L2Position::RefVar => false,
    }
}

/// Whether N7 actually constrains a value position that has emitted `prefix`:
/// a bare word is open, and it is not one of the [`VALUE_KEYWORDS`].
///
/// At the anchor there is no word yet, so every value shape — a literal, a
/// `$var`, a nested opener, a lambda — is still legal and the rule stays out of
/// the way. `ScopeTracker::position` applies this so
/// [`L2Position::ValueIdent`] is only ever *reported* where it narrows;
/// [`narrow_into`] and [`admits_eos`] then need no second copy of the test.
pub(crate) fn value_ident_constrains(prefix: &[u8]) -> bool {
    !prefix.is_empty() && !VALUE_KEYWORDS.iter().any(|kw| kw.as_bytes() == prefix)
}

/// Bare words that *are* complete values in their own right, so N7 leaves them
/// and whatever follows them alone. `docs/spec/grammar.md`'s `booleanLit` is the
/// whole set the emitted subset admits; the corpus exercises both (`… == true }`,
/// `pair(…, false)`).
const VALUE_KEYWORDS: &[&str] = &["true", "false"];

/// Refill `dst` with N7's continuation set for an open bare value-position
/// identifier: the token either continues the word itself, or is one of the
/// shapes that give a bare word a meaning — `.` (a nested pipeline source's own
/// dot, `…->filter(x|B.all()->isEmpty())`, or an enum-path value selection),
/// `:` (a `::` package separator, or a typed binder's annotation), `(` (a
/// function application), or `|` (the lambda arrow that makes the word a
/// binder).
///
/// Everything else is cleared, EOS included: whitespace, `,`, every closer, and
/// every operator would end the word as a standalone expression that resolves to
/// nothing. **Whitespace is deliberately in that set** even though `x |` is
/// legal Pure — admitting it would let a single space close the lexeme, drop
/// this rule out of scope, and hand the whole escape straight back (the same
/// reason [`NameClose::MustCall`] excludes it). Confirmed live on both counts:
/// `->col(between *'District_city')` and `->pair(code !='Name_T2')` are rejected
/// exactly as their space-free siblings are, and no in-scope gold query separates
/// a bare word from its continuation with whitespace.
///
/// `||` is excluded despite its leading `|`: it is boolean-or, never a lambda
/// arrow, and the vocabulary carries it as one token.
fn fill_value_ident(dst: &mut BitMask, vocab: &Vocab) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        if keeps_value_ident(vocab.bytes(id).unwrap_or(&[])) {
            dst.set(id);
        }
    }
}

/// Whether `bytes` may follow an open bare value-position identifier (N7).
fn keeps_value_ident(bytes: &[u8]) -> bool {
    match bytes.first() {
        Some(&byte) if is_ident_tail(byte) => true,
        Some(&(b'.' | b':' | b'(')) => true,
        Some(&b'|') => !bytes.starts_with(BOOLEAN_OR),
        _ => false,
    }
}

/// Boolean-or — the one `|`-led shape that is not a lambda arrow.
const BOOLEAN_OR: &[u8] = b"||";

/// Whether the L2 overlay permits the stream to **end** at this position — the
/// completion half of the same trie walk [`narrow_into`] masks with
/// (`docs/spec/schema.md` §6.5).
///
/// L1 acceptance is a pure lookahead fact ("does a value-boundary byte from
/// here reach a value-terminal state?"), and an identifier has no
/// self-terminating byte, so *any* partial name is trivially "completable"
/// under it: the moment the vocabulary holds a token that is a strict byte
/// prefix of a legal name (`"a"` beside `"all"`), a stream could stop
/// mid-identifier and call itself done — confirmed live, a walk ending in
/// `Class.a`, which the engine rejects. Reading the same [`NamePoint`] the mask
/// is built from is what makes
/// [`is_complete`](crate::DecoderSession::is_complete) agree with
/// [`allowed_mask`](crate::DecoderSession::allowed_mask)'s own EOS bit instead
/// of contradicting it.
///
/// Positions no trie rule governs are unconstrained (`true`): the type levers
/// (T1/T2/T3) and the source-method argument slot all sit inside an open
/// delimiter or right after an operator, where L1 does not accept anyway.
pub(crate) fn admits_eos(
    cache: &mut NarrowCache,
    schema: &Schema,
    pos: &L2Position,
    prefix: &[u8],
    columns: &[Vec<u8>],
    vars: &[String],
) -> bool {
    // N3f forbids ending on a *denied* whole name, and constrains nothing else —
    // the same verdict `fill_extent_method` writes into the EOS bit, read back
    // here so `is_complete` cannot disagree with the mask.
    if matches!(pos, L2Position::ExtentMethod) {
        return extent_deny_cursor(prefix).is_none_or(|cursor| !EXTENT_DENY.is_terminal(cursor));
    }
    let Some(rule) = TrieRule::of(pos, schema, columns, vars) else {
        // N7 is the one non-trie rule that forbids ending here: a bare word left
        // dangling in a value position is not an expression.
        return !matches!(pos, L2Position::ValueIdent);
    };
    let entry = rule.entry(cache);
    // A completed or diverged prefix hands the tail back to the byte-PDA — the
    // same no-constraint verdict `narrow_trie` returns for it.
    let Some(cursor) = cursor_of(entry, prefix) else {
        return true;
    };
    name_point(&entry.trie, cursor).admits_eos()
}

/// The legal-name set a trie rule narrows against, plus how that rule's memo is
/// keyed, which lexeme shape it governs, and what it admits once its name is
/// whole.
///
/// One description read by both [`narrow_into`]'s mask fill and [`admits_eos`]'s
/// completion check, so the two can never disagree about where a name legally
/// ends.
struct TrieRule<'a> {
    key: CacheKey,
    kind: TrieKind,
    names: Names<'a>,
}

/// Which rule's names a [`TrieRule`] builds from — the borrow the trie is built
/// out of on a cache miss, kept separate from the memo key so the (cheap) key
/// can be formed without touching the schema.
enum Names<'a> {
    /// N3: every source classpath, plus the block-statement `let` keyword.
    Source(&'a Schema),
    /// S1: the single source method [`SOURCE_METHOD`].
    SourceMethod,
    /// N3c: the store methods [`STORE_METHODS`].
    StoreMethod,
    /// N1/N2: one class's member names, bare and quoted. Pure admits either form
    /// after a navigation dot (`$x.name`, `$x.'Gross Credits'`), and both name the
    /// same member set — so both are candidates, and a quoted phantom is cleared
    /// exactly as a bare one is (live: `{|…::Countrylanguage.all().'Capital_T1'}`
    /// → "Can't find property 'Capital_T1' in class …::Countrylanguage").
    Member(&'a Schema, &'a str),
    /// N6: the emitted relation columns, quoted as the model writes them.
    Column(&'a [Vec<u8>]),
    /// N6 arm-R: the same columns, bare.
    RelationColumn(&'a [Vec<u8>]),
    /// S2: every variable name the stream has bound.
    RefVar(&'a [String]),
}

/// The byte a niladic method's mandatory call opens with. S1's `all` owes it:
/// confirmed live, a bare `Class.all` parses as a property read and fails to
/// compile, exactly as `Db->tableToTDS` without its `()` did.
const CALL_OPEN: u8 = b'(';

/// The byte a resolved **class** source path owes: its `.all()` navigation dot.
/// A class path is a `Class<T>[1]` metatype value, not the `T[*]` extent, so
/// every method arrowed straight off it is a type mismatch by construction —
/// live-attested (`…::Country->groupBy('CountryCode_T2_2')` →
/// "Can't find a match for function 'groupBy(Class<Country>[1],String[1])'").
const SOURCE_NAV_DOT: u8 = b'.';

/// The byte a resolved **store** path owes: the `-` of its `->` step. A store is
/// a `Database`, never a class extent, so it is arrowed into a store method and
/// never `.all()`-ed — live-attested (`|…::Db.all()` → "Can't find a match for
/// function 'getAll(Database[1])'"). The gold corpus agrees at both ends: across
/// its 5034 queries a class source path is followed by `.all` 501 times and by
/// nothing else, and a store path by `->tableReference` 8455 times and by
/// nothing else.
const SOURCE_STEP_ARROW: u8 = b'-';

impl<'a> TrieRule<'a> {
    /// The trie rule governing `pos`, or `None` where no trie rule applies.
    fn of(
        pos: &'a L2Position,
        schema: &'a Schema,
        columns: &'a [Vec<u8>],
        vars: &'a [String],
    ) -> Option<Self> {
        let (key, kind, names) = match pos {
            L2Position::SourceIdent => {
                (CacheKey::Source, TrieKind::ClassPath, Names::Source(schema))
            }
            L2Position::SourceMethod => (
                CacheKey::SourceMethod,
                TrieKind::IdentOrStr,
                Names::SourceMethod,
            ),
            L2Position::StoreMethod => (
                CacheKey::StoreMethod,
                TrieKind::IdentOrStr,
                Names::StoreMethod,
            ),
            L2Position::Member(class) => (
                CacheKey::Member(class.clone()),
                TrieKind::IdentOrStr,
                Names::Member(schema, class),
            ),
            L2Position::Column => (
                CacheKey::Column(columns.len()),
                TrieKind::Str,
                Names::Column(columns),
            ),
            L2Position::RelationColumn => (
                CacheKey::RelationColumn(columns.len()),
                TrieKind::Ident,
                Names::RelationColumn(columns),
            ),
            L2Position::RefVar => (
                CacheKey::RefVar(vars.len()),
                TrieKind::Ident,
                Names::RefVar(vars),
            ),
            // Exhaustive on purpose, like [`narrow_into`]'s own match: the
            // non-trie positions are listed rather than swept up by a `_`, so a
            // new [`L2Position`] must be classified here and no arm can be
            // deleted without a compile error.
            L2Position::None
            | L2Position::SourceMethodArg
            | L2Position::StoreMethodArg
            | L2Position::StoreMethodArgSep { .. }
            | L2Position::SourceExtent { .. }
            | L2Position::ExtentMethod
            | L2Position::ReceiverOnlyArg
            | L2Position::StoreResult { .. }
            | L2Position::StrOperator { .. }
            | L2Position::LogicalOperand
            | L2Position::ReValue(_)
            | L2Position::Comparator(_)
            | L2Position::OrderedOperand
            | L2Position::Reducer(_)
            | L2Position::ValueIdent => return None,
        };
        Some(Self { key, kind, names })
    }

    /// This rule's cache entry, building its trie on first use.
    fn entry<'c>(&self, cache: &'c mut NarrowCache) -> &'c mut RuleCache {
        cache
            .tries
            .entry(self.key.clone())
            .or_insert_with(|| RuleCache {
                trie: self.names.build(),
                kind: self.kind,
                masks: HashMap::new(),
            })
    }
}

impl Names<'_> {
    /// Build this rule's legal-name trie.
    fn build(&self) -> Trie {
        match self {
            Self::Source(schema) => Trie::from_closing_names(
                schema
                    .source_paths()
                    .map(|path| (path, source_close(schema, path)))
                    .chain(std::iter::once((LET_KEYWORD, NameClose::Free))),
            ),
            Self::SourceMethod => Trie::from_closing_names(std::iter::once((
                SOURCE_METHOD,
                NameClose::MustFollow(CALL_OPEN),
            ))),
            Self::StoreMethod => Trie::from_closing_names(
                STORE_METHODS
                    .iter()
                    .map(|(name, _)| (*name, NameClose::MustFollow(CALL_OPEN))),
            ),
            Self::Member(schema, class) => {
                let names = schema.member_names(class);
                let quoted: Vec<Vec<u8>> = names.iter().map(|n| quote(n.as_bytes())).collect();
                Trie::from_names(names.iter().map(|n| n.as_bytes().to_vec()).chain(quoted))
            }
            Self::Column(columns) => Trie::from_names(columns.iter().map(|c| quote(c))),
            Self::RelationColumn(columns) => Trie::from_names(columns.iter().cloned()),
            Self::RefVar(vars) => Trie::from_names(vars.iter().map(String::as_bytes)),
        }
    }
}

/// N3c: what a whole pipeline-source path owes as its continuation — the
/// `.all()` dot for a class, the `->` step for the store (`docs/spec/schema.md`
/// §6.5). The two are mutually exclusive and neither is optional, which is what
/// makes the position expressible as a mask at all: a class path denotes a
/// `Class<T>[1]` metatype, so arrowing a method straight off it mismatches every
/// signature, and a store path denotes a `Database`, which has no extent to
/// `.all()`.
///
/// [`Schema::has_class`](crate::schema::model::Schema::has_class) is the
/// classifier because it is the same one `source_paths` builds the set from:
/// every source path is a class or it is the store.
fn source_close(schema: &Schema, path: &str) -> NameClose {
    if schema.has_class(path) {
        NameClose::MustFollow(SOURCE_NAV_DOT)
    } else {
        NameClose::MustFollow(SOURCE_STEP_ARROW)
    }
}

/// Where a trie rule's cursor sits relative to a whole legal name — the single
/// fact the mask fill and the completion check both read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamePoint {
    /// No lexeme is open yet (the cursor is still the trie root).
    Anchor,
    /// A **strict** prefix of some legal name: no name ends here, so the lexeme
    /// must keep going. Every boundary token — and EOS — would close it on a
    /// name that does not exist.
    Partial,
    /// A whole legal name that stands on its own.
    Whole,
    /// A whole legal name owing a specific next byte
    /// ([`NameClose::MustFollow`]) — its mandatory call `(`, a class source's
    /// `.all()` dot, or a store source's `->` step.
    WholeMustFollow(u8),
}

impl NamePoint {
    /// Whether the stream may end here.
    const fn admits_eos(self) -> bool {
        matches!(self, Self::Anchor | Self::Whole)
    }
}

/// Classify `cursor` against the rule's names and the [`NameClose`] policy of
/// whichever name ends there.
fn name_point(trie: &Trie, cursor: u32) -> NamePoint {
    if cursor == trie.root() {
        return NamePoint::Anchor;
    }
    if !trie.is_terminal(cursor) {
        return NamePoint::Partial;
    }
    match trie.close_at(cursor) {
        NameClose::Free => NamePoint::Whole,
        NameClose::MustFollow(byte) => NamePoint::WholeMustFollow(byte),
    }
}

/// The trie cursor `prefix` walks to, or `None` when the prefix already
/// completed a legal name at a boundary byte (the byte-PDA takes the tail) or
/// diverged from every name — either way the rule stops constraining.
fn cursor_of(entry: &RuleCache, prefix: &[u8]) -> Option<u32> {
    if prefix.is_empty() {
        return Some(entry.trie.root());
    }
    match walk(&entry.trie, entry.trie.root(), prefix, entry.kind.shape()) {
        Walk::Stay(cursor) => Some(cursor),
        Walk::Complete { .. } | Walk::Diverge => None,
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
        // Exhaustive for the same reason [`narrow_into`] is: a `_` here would
        // let a deleted arm still compile, and an arm whose fall-through
        // happens to be a no-op then becomes a mutant no test can kill.
        L2Position::None
        | L2Position::SourceIdent
        | L2Position::SourceMethod
        | L2Position::StoreMethod
        | L2Position::SourceMethodArg
        | L2Position::StoreMethodArg
        | L2Position::StoreMethodArgSep { .. }
        | L2Position::SourceExtent { .. }
        | L2Position::ExtentMethod
        | L2Position::ReceiverOnlyArg
        | L2Position::StoreResult { .. }
        | L2Position::StrOperator { .. }
        | L2Position::LogicalOperand
        | L2Position::Column
        | L2Position::ReValue(_)
        | L2Position::Comparator(_)
        | L2Position::OrderedOperand
        | L2Position::Reducer(_)
        | L2Position::RefVar
        | L2Position::ValueIdent => false,
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
            Some(rest) => !matches!(walk(trie, root, rest, NameShape::Plain), Walk::Diverge),
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
///
/// The per-cursor memo stays exact under the [`NameClose`] policy because that
/// policy is fixed per [`CacheKey`] — the same rule always classifies the same
/// cursor node the same way.
fn narrow_trie(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    rule: &TrieRule,
    prefix: &[u8],
    vocab: &Vocab,
    eos_bit: u32,
) -> bool {
    let entry = rule.entry(cache);
    // The prefix already completed a legal name or diverged — the lexeme is done
    // (or was never legal); leave the L1 mask unchanged.
    let Some(cursor) = cursor_of(entry, prefix) else {
        return false;
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
/// on a miss run `fill` and memoize it. The cursor-independent rules reach here;
/// the trie rules memoize per cursor node in `narrow_trie`.
///
/// Returns `true` — [`narrow_into`]'s own "a constraint applied" verdict, which
/// every caller was otherwise restating one line later. Filling a rule's mask
/// *is* applying its constraint, so the two are never independently decidable,
/// and a hand-threaded copy per arm is only somewhere for them to disagree.
fn with_cache(
    dst: &mut BitMask,
    cache: &mut NarrowCache,
    key: CacheKey,
    fill: impl FnOnce(&mut BitMask),
) -> bool {
    if let Some(cached) = cache.operand.get(&key) {
        dst.copy_from(cached);
        return true;
    }
    fill(dst);
    cache.operand.insert(key, dst.clone());
    true
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
    /// A plain identifier (N1/N2, N6 arm-R, the refVar set): a candidate token
    /// starts with an identifier-tail byte.
    Ident,
    /// N3's `::`-joined source classpath. A candidate additionally includes a
    /// `:`-led token, because the byte-PDA keeps the classpath lexeme *open* across
    /// the separator (`InSourceIdent` → `SourceColon` → `SourceColon2`): the `::`
    /// is inside the name this rule governs, not a boundary handing off to an
    /// automaton that would re-vet the tail. Leaving it a non-candidate — and
    /// leaving [`walk`] to read the colon as a completed name's boundary — is what
    /// let a fabricated segment (`spider::w::Db` + `::desc`) past N3 entirely.
    ClassPath,
    /// A quoted string (N6): a candidate token opens a string (`'`) or continues
    /// one already in flight.
    Str,
    /// S1's source-method position: unlike every other rule here, which narrows
    /// one lexeme *shape* and lets every other shape pass through untouched
    /// (a quoted literal is legal wherever a property name is, so `Member`
    /// governs `Ident` only and leaves `Str` alone), a source classpath's dot
    /// only ever legally continues into the bare word `all` — a quoted literal
    /// here is never legal at all, so both shapes are candidates: whichever one
    /// it is, it must extend [`SOURCE_METHOD`](crate::schema::scope::SOURCE_METHOD)
    /// or it is cleared. A candidate token opens with an identifier-tail byte,
    /// opens a string (`'`), or continues one already in flight.
    IdentOrStr,
}

impl TrieKind {
    /// The lexeme shape the rule's names have — the boundary predicate [`walk`]
    /// applies. Only N3's source rule spells `::` inside its names.
    const fn shape(self) -> NameShape {
        match self {
            Self::ClassPath => NameShape::ClassPath,
            Self::Ident | Self::Str | Self::IdentOrStr => NameShape::Plain,
        }
    }
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

/// Whether `bytes` is one of the **ordered** comparators. [`classify`] folds
/// every comparator shape into one [`Lexeme::Cmp`], and ordered-vs-equality is
/// exactly what T2 and T6 both distinguish, so the set is read off the raw bytes
/// — stated once here rather than spelled out at each of its two use sites.
fn is_ordered_comparator(bytes: &[u8]) -> bool {
    matches!(bytes, b"<" | b">" | b"<=" | b">=")
}

/// Whether a comparator token is kept under a T2 constraint with LHS class `lhs`
/// (§6.6 T2). An ordered comparator is legal only for a numeric or temporal
/// operand; equality/inequality (`== !=`) and every non-comparator shape stay
/// kept.
fn keeps_comparator(bytes: &[u8], lhs: TypeClass) -> bool {
    if is_ordered_comparator(bytes) {
        matches!(lhs, TypeClass::Numeric | TypeClass::Temporal)
    } else {
        true
    }
}

/// Refill `dst` with T6's set for a non-scalar navExpr, plus EOS
/// ([`L2Position::OrderedOperand`]): every token but the ordered comparators,
/// which the engine declares no overload of for a collection or a class-typed
/// operand. `== !=` are deliberately kept — Pure's `equal` is `Any[*]`-generic,
/// and all three of the position's shapes compile with it live.
fn fill_ordered_operand(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        if !is_ordered_comparator(vocab.bytes(id).unwrap_or(&[])) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Refill `dst` with the T2 comparator set for LHS class `masked_by`, plus EOS.
fn fill_comparator(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, masked_by: TypeClass) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_comparator(bytes, masked_by) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Whether a reducer-method-name token is kept under a T3 constraint with
/// element class `tc` (§6.6 T3). `sum`/`average` are legal only on a numeric
/// element — corpus-confirmed exclusively numeric, and arithmetically
/// meaningless otherwise. `min`/`max`/`count` are left unconstrained: `count`
/// is legal on any collection by definition, and a gold `car_1` query uses
/// `->min()` on a `String[*]` element (lexicographic ordering), which
/// falsifies a numeric/temporal-only reading of the ordered reducers — with no
/// counter-evidence to mask any type instead, the safe default is to admit all
/// (§4, "do not invent constraints the corpus does not exercise").
fn keeps_reducer(bytes: &[u8], tc: TypeClass) -> bool {
    !matches!(bytes, b"sum" | b"average") || matches!(tc, TypeClass::Numeric)
}

/// Refill `dst` with the T3 reducer set for element class `masked_by`, plus EOS.
fn fill_reducer(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, masked_by: TypeClass) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_reducer(bytes, masked_by) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Refill `dst` with [`L2Position::SourceMethodArg`]'s set, plus EOS: unlike every
/// other rule here, which keeps a token unless it is a wrong-shaped *candidate*
/// the rule specifically governs, this rule's default is inverted — the value
/// position right after the source method's own `(` (or after a comma inside
/// it) has no legal *identifier/literal* argument at all, only its own closer,
/// intervening whitespace, or a milestoning date (`docs/spec/grammar.md`'s
/// `milestoneLit`/`dateLit`, both classified [`Lexeme::Date`] — bitemporal
/// milestoning legally passes zero, one, or two comma-separated date arguments
/// here, confirmed by the corpus's own `Firm.all(%latest, %latest)` fixture and
/// the modern-dialect seed corpus). This rule does not cap the count at two or
/// validate a milestone symbol beyond its lexical shape — like every other
/// `%`-literal position in this overlay, that residue is left to the compiler
/// oracle; it only ever masks the phantom-argument shapes the walker was
/// actually observed emitting (`Class.all('French')`, `Class.all(all)` — an
/// identifier or string literal, never legal here). Every OTHER legal-at-L1
/// value-start byte (an identifier, a string quote, a non-milestone digit, `$`,
/// `~`, a nested opener, `|`) resolves to a distinct, non-`Ws`/`Close`/`Date`
/// [`Lexeme`] under [`classify`], so a whole-token classification (mirroring
/// [`fill_operand`]'s style — no trie/prefix-walk is needed, since legality here
/// is a function of shape alone) keeps exactly the three shapes that are never a
/// phantom argument: `Ws`, `Close`, and `Date`.
fn fill_source_method_arg(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if matches!(classify(bytes), Lexeme::Ws | Lexeme::Close | Lexeme::Date) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// The byte a store-method argument owes: its string literal's opening quote.
/// Every store-method parameter is a `String[1]` (`STORE_METHODS`' corpus and
/// signature evidence), and a Pure string literal has exactly one opener.
const STR_OPEN: u8 = b'\'';

/// The byte that separates two store-method arguments.
const ARG_SEP: u8 = b',';

/// The byte that closes a store method's own call.
const CALL_CLOSE: u8 = b')';

/// Refill `dst` with [`L2Position::StoreMethodArg`]'s set, plus EOS: an opened
/// store-method argument slot owes a string literal, so only whitespace and a
/// string's own opening quote survive. The closer is *not* kept — that is the
/// arity half of N3d, and it is why `->tableReference()` (live:
/// "tableReference(Database[1])") cannot be walked at all.
///
/// Discriminated on the token's **first byte** rather than a whole-token
/// [`classify`], exactly as N7's [`keeps_value_ident`] is: under byte-level BPE
/// a legal continuation routinely arrives fused to its neighbours
/// (`,\n    'Faculty'` is one token over the gold vocabulary), and a whole-token
/// classification would mask it — over-masking a *gold* query, not a phantom.
fn fill_store_method_arg(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        if opens_with(vocab.bytes(id).unwrap_or(&[]), |byte| {
            byte.is_ascii_whitespace() || byte == STR_OPEN
        }) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// Refill `dst` with [`L2Position::StoreMethodArgSep`]'s set for a call that has
/// completed `complete` arguments, plus EOS: a `,` while arguments remain, the
/// call's own `)` once the arity is met, and whitespace either way. No operator
/// is ever legal between a store method's arguments, which is what closes the
/// `->tableReference('a'=='b')` residue the compiler — not the parser — rejects.
///
/// A `'` is kept too: this position is also read on an argument literal's
/// *pending* closing quote (see `ScopeTracker::store_method_arg_sep`), where a
/// second `'` doubles it and the same literal continues (`'O''Brien'`).
///
/// First-byte discrimination for the same BPE reason [`fill_store_method_arg`]
/// documents.
fn fill_store_method_arg_sep(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, remaining: bool) {
    let owed = if remaining { ARG_SEP } else { CALL_CLOSE };
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        if opens_with(vocab.bytes(id).unwrap_or(&[]), |byte| {
            byte.is_ascii_whitespace() || byte == owed || byte == STR_OPEN
        }) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// The byte a pipeline step opens with: the `-` of its `->`.
const STEP_DASH: u8 = b'-';

/// The byte a property navigation over an extent opens with.
const NAV_DOT: u8 = b'.';

/// The byte that completes a step arrow once its `-` has been emitted.
const STEP_GT: u8 = b'>';

/// The whole step connector, for the vocabularies that offer it as one token.
const STEP_ARROW: &[u8] = b"->";

/// Refill `dst` with [`L2Position::SourceExtent`]'s set, plus EOS: a closed
/// `Class.all()` is a `T[*]` extent, and the only things that follow one are a
/// pipeline step (`->`), a property navigation that maps over it (`.`), or the
/// end of the query. Every operator the vocabulary offers is a type mismatch
/// against a collection, which is what this clears.
///
/// The `-` of the step arrow is the one byte an *arithmetic* minus shares, so it
/// is admitted only as the arrow: either whole (`->`, however much rides behind
/// it), or as the bare `-` a vocabulary that splits the connector offers — and
/// then `after_dash` narrows the very next token to the `>` that completes it,
/// so `Class.all()-'HeadOfState'` (live: "Collection element must have a
/// multiplicity [1]") cannot be reassembled a byte at a time.
fn fill_source_extent(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, after_dash: bool) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_source_extent(bytes, after_dash) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// N3f's deny trie: the method names no class extent can present a receiver for
/// ([`EXTENT_INCOMPATIBLE_METHODS`]). Built once — the set is a compile-time
/// constant with no schema, column or vocabulary input, so it needs neither a
/// per-session cache entry nor a rebuild.
static EXTENT_DENY: LazyLock<Trie> =
    LazyLock::new(|| Trie::from_names(EXTENT_INCOMPATIBLE_METHODS.iter().copied()));

/// The [`EXTENT_DENY`] cursor `prefix` walks to, or `None` once the open method
/// name has left every denied name — the rule then constrains nothing.
///
/// The dual of [`cursor_of`]: a [`Walk::Diverge`] is the *good* case here (the
/// name being typed is not one this rule denies), and a [`Walk::Complete`] means
/// a denied name was already closed, which only happens if the closing token was
/// admitted — it never is, because [`fill_extent_method`] is what clears it.
fn extent_deny_cursor(prefix: &[u8]) -> Option<u32> {
    if prefix.is_empty() {
        return Some(EXTENT_DENY.root());
    }
    match walk(&EXTENT_DENY, EXTENT_DENY.root(), prefix, NameShape::Plain) {
        Walk::Stay(cursor) => Some(cursor),
        Walk::Complete { .. } | Walk::Diverge => None,
    }
}

/// Refill `dst` with [`L2Position::ExtentMethod`]'s set: every vocabulary token,
/// less the ones that would **close** the open method name on an entry of
/// [`EXTENT_INCOMPATIBLE_METHODS`].
///
/// Subtractive by construction, which is what keeps the rule sound in the
/// direction that matters. It names no legal set — there is none to name (see
/// [`L2Position::ExtentMethod`]) — so a builtin the engine does accept on an
/// extent is never touched, whether or not any corpus has heard of it.
///
/// The clear lands on the *closing* token rather than the name's first byte,
/// because under byte-level BPE a denied name is routinely a live prefix of a
/// legal one (`in` ⊂ `indexOf`, `pair` ⊂ `pairwise`): [`walk`] descends an edge
/// in preference to a terminal, so those keep walking and only a boundary byte —
/// the call's `(`, whitespace, an operator — completes the denied name and is
/// cleared. EOS is cleared at a terminal cursor for the same reason: a stream
/// that ends on `->sum` has closed the name just as surely.
fn fill_extent_method(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32, cursor: u32) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_extent_method(cursor, bytes) {
            dst.set(id);
        }
    }
    if !EXTENT_DENY.is_terminal(cursor) {
        dst.set(eos_bit);
    }
}

/// Whether `bytes` may continue an extent method name — see
/// [`fill_extent_method`].
fn keeps_extent_method(cursor: u32, bytes: &[u8]) -> bool {
    !matches!(
        walk(&EXTENT_DENY, cursor, bytes, NameShape::Plain),
        Walk::Complete { .. }
    )
}

/// Whether `bytes` may continue a class extent — see [`fill_source_extent`].
fn keeps_source_extent(bytes: &[u8], after_dash: bool) -> bool {
    if after_dash {
        return completes_step_arrow(bytes);
    }
    match bytes.first() {
        Some(&byte) if byte.is_ascii_whitespace() || byte == NAV_DOT => true,
        Some(&STEP_DASH) => opens_step_arrow(bytes),
        _ => false,
    }
}

/// Whether `bytes`' first byte satisfies `keep` — the shared first-byte test the
/// N3d/N3e fills discriminate on (an empty token keeps nothing).
fn opens_with(bytes: &[u8], keep: impl Fn(u8) -> bool) -> bool {
    bytes.first().copied().is_some_and(keep)
}

/// Refill `dst` with [`L2Position::ReceiverOnlyArg`]'s set, plus EOS: an arrow
/// call of a [`RECEIVER_ONLY_METHODS`] entry has already supplied the one
/// parameter its whole overload set declares, so its slot admits **only**
/// whitespace and the call's own closer.
///
/// The exact complement of N3d's arity half. There the opened slot owes an
/// argument and the closer is what is cleared (`->tableReference()` cannot be
/// walked); here the slot owes nothing and every *opener* is cleared instead, so
/// `->isEmpty('x')` (live: "isEmpty(Country[*],String[1])", against a candidate
/// set that is `isEmpty(Any[0..1])` and `isEmpty(Any[*])` and nothing else)
/// cannot be walked. Both are the same fact — an argument list is exactly as long
/// as the signature says — read from the two ends.
///
/// First-byte discriminated for the reason [`fill_store_method_arg`] gives: under
/// byte-level BPE a closer routinely arrives fused to what follows it.
fn fill_receiver_only_arg(dst: &mut BitMask, vocab: &Vocab, eos_bit: u32) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        if opens_with(vocab.bytes(id).unwrap_or(&[]), |byte| {
            byte.is_ascii_whitespace() || byte == CALL_CLOSE
        }) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// The operator bytes no `Table[1]` can open a term with — every ordered
/// comparison, arithmetic and logical operator the vocabulary offers, keyed on
/// the byte each begins with (§6.6 N4a).
///
/// A first-byte set rather than a token list, for the reason
/// [`fill_store_method_arg`] gives in the other direction: under byte-level BPE
/// an operator arrives fused to its right-hand operand (`&&'x'`, `>'Edispl_T2'`),
/// and a whole-token match would let every such fusion through. Each byte here
/// opens no *legal* continuation of a store result, so clearing the whole family
/// masks nothing real:
///
/// * `&` and `|` only ever begin `&&`/`||` — live `and(Table[1],String[1])`,
///   `or(Table[1],String[1])`;
/// * `<` and `>` begin the four ordered comparators — live
///   `greaterThan(Table[1],String[1])`, `lessThanEqual(Table[1],String[1])` (the
///   step arrow's own `>` arrives behind its `-`, at
///   [`StoreResult::after_dash`](L2Position::StoreResult));
/// * `+`, `*` and `/` begin the arithmetic operators — live `plus(Any[2])`,
///   `times(Any[2])`, `divide(Table[1],String[1])`.
///
/// `=` and `!` are deliberately absent: `==`/`!=` resolve through
/// `equal(Any[1],Any[1])` and compile live on a store result.
const STORE_RESULT_DENIED_OPENERS: &[u8] = b"&|<>+*/";

/// The operator bytes no **string literal** can be the left operand of — the
/// three arithmetic operators with no `String` overload (§6.6 N4c).
///
/// A first-byte set, for the reason [`STORE_RESULT_DENIED_OPENERS`] gives: under
/// byte-level BPE an operator arrives fused to its right-hand operand
/// (`*'Continent_t1'` is one token over the gold vocabulary). Neither byte opens
/// any legal continuation of a string literal, in any of the three corpora: after
/// a closing quote, `*` and `/` occur **zero** times across the 5034 gold
/// queries, the modern-dialect seeds and the engine-labelled differential set.
///
/// `+` is deliberately absent — `plus(String[*])` is concatenation and compiles
/// live — as are `<`/`>` (`greaterThan(String[1],String[1])` is a real overload)
/// and `&`/`|`, which follow a string literal all through the corpus while taking
/// the enclosing *comparison*, not the literal, as their operand.
/// The `-` split matters more at this rule than anywhere else in the overlay: a
/// string literal is arrowed 32309 times across the three corpora and is the left
/// operand of an arithmetic minus in none of them, so the byte must stream as the
/// arrow and die as the operator ([`keeps_after_completed_term`]).
const STR_OPERATOR_DENIED_OPENERS: &[u8] = b"*/";

/// Refill `dst` with the continuation set of a **completed term** whose type
/// forbids the operator family `denied`, plus EOS — N4a's and N4c's shared fill.
///
/// **Subtractive**, unlike N3e's [`fill_source_extent`]: both rules govern a term
/// that really does accept more than a class extent does — a bare
/// `|…::Db->tableReference('T','S')` compiles live and returns `Table`, and a
/// string literal takes `+`, the ordered comparators and a step arrow — so each
/// clears exactly its own attested family and leaves every closer, separator,
/// `.` navigation and equality comparison alone.
///
/// One fill for both, because the two differ in nothing but `denied`: the
/// per-rule facts live in [`STORE_RESULT_DENIED_OPENERS`] and
/// [`STR_OPERATOR_DENIED_OPENERS`], and the guard they share is stated once in
/// [`keeps_after_completed_term`] (constitution §4).
fn fill_after_completed_term(
    dst: &mut BitMask,
    vocab: &Vocab,
    eos_bit: u32,
    after_dash: bool,
    denied: &[u8],
) {
    dst.clear_all();
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        if keeps_after_completed_term(bytes, after_dash, denied) {
            dst.set(id);
        }
    }
    dst.set(eos_bit);
}

/// The shared keep-rule N4a and N4c discriminate on: after a completed term,
/// every token survives except one that opens with a byte in `denied`, with the
/// `-` of a step arrow admitted only as the arrow — whole (`->`, however much
/// rides behind it), or as the bare `-` a splitting vocabulary offers, which
/// `after_dash` then narrows to the `>` that completes it.
///
/// One derivation for both rules, so the reassembly guard the two share is
/// stated exactly once (constitution §4).
fn keeps_after_completed_term(bytes: &[u8], after_dash: bool, denied: &[u8]) -> bool {
    if after_dash {
        return completes_step_arrow(bytes);
    }
    match bytes.first() {
        Some(&STEP_DASH) => opens_step_arrow(bytes),
        Some(byte) => !denied.contains(byte),
        None => false,
    }
}

/// Whether a `-`-leading `bytes` is admissible **as a step arrow**: the whole
/// `->` (however much rides behind it), or the bare `-` a vocabulary that splits
/// the connector offers. The one place the arithmetic-minus carve-out is stated.
fn opens_step_arrow(bytes: &[u8]) -> bool {
    bytes.len() == 1 || bytes.starts_with(STEP_ARROW)
}

/// Whether `bytes` completes a step arrow whose `-` is already committed — the
/// `after_dash` half of the same carve-out.
fn completes_step_arrow(bytes: &[u8]) -> bool {
    bytes.first() == Some(&STEP_GT)
}

/// Refill `dst` from a trie walk: keep every vocab token that can still reach a
/// legal name from `cursor`, plus the reserved EOS bit — both subject to where
/// `cursor` sits relative to a whole name ([`NamePoint`]).
///
/// A *non-candidate* (a structural/whitespace token the rule does not govern) is
/// kept at the anchor and after a free-standing whole name, exactly as the
/// whole-lexeme narrower kept every non-`Ident`/`Str` lexeme. It is **cleared**
/// at a [`NamePoint::Partial`] cursor: the rule's own lexeme is open on a strict
/// prefix of some legal name, so a boundary token would end the lexeme on a name
/// that does not exist (confirmed live: a walk `l->pair(…)`, where `l` is a
/// strict prefix of the `let` keyword the source rule admits — "Can't find the
/// packageable element 'l'"). At a [`NamePoint::WholeMustFollow`] cursor it is
/// kept only when it opens with the byte that name owes. The reserved EOS bit
/// follows the same rule, which is what [`admits_eos`] reads back for
/// [`is_complete`](crate::DecoderSession::is_complete).
fn fill_trie(
    dst: &mut BitMask,
    vocab: &Vocab,
    eos_bit: u32,
    trie: &Trie,
    cursor: u32,
    kind: TrieKind,
) {
    dst.clear_all();
    let point = name_point(trie, cursor);
    let mid = point != NamePoint::Anchor;
    for id in 0..vocab.len() as u32 {
        let bytes = vocab.bytes(id).unwrap_or(&[]);
        let keep = if is_candidate(bytes, kind, mid) {
            keeps_candidate(trie, cursor, bytes, kind.shape())
        } else {
            match point {
                NamePoint::Partial => false,
                NamePoint::WholeMustFollow(owed) => bytes.first() == Some(&owed),
                NamePoint::Anchor | NamePoint::Whole => true,
            }
        };
        if keep {
            dst.set(id);
        }
    }
    if point.admits_eos() {
        dst.set(eos_bit);
    }
}

/// Whether a *candidate* token may be kept from `cursor`: it either stays on a
/// live prefix of some legal name, or completes one at a boundary that name's
/// own [`NameClose`] admits.
///
/// The second half is what makes the close policy hold under byte-level BPE. A
/// token can carry both a name's tail and the byte that ends it (`y->` closing
/// `…::Country`), so the cursor at the token's *start* is only a strict prefix
/// and never reaches [`NamePoint::WholeMustFollow`]. [`Walk::Complete`] reports
/// where the name ended and on which byte, so the policy is applied at the name
/// the token actually completed rather than being handed off to a byte-PDA that
/// re-vets the tail lexically but knows nothing of `.all()` or `->`.
fn keeps_candidate(trie: &Trie, cursor: u32, bytes: &[u8], shape: NameShape) -> bool {
    match walk(trie, cursor, bytes, shape) {
        Walk::Stay(_) => true,
        Walk::Complete { at, boundary } => trie.close_at(at).admits(boundary),
        Walk::Diverge => false,
    }
}

/// Whether `bytes` is a token the `kind` trie may clear: an identifier-tail start
/// for an `Ident` rule, or a string opener (or any byte once a string is in
/// flight) for a `Str` rule.
fn is_candidate(bytes: &[u8], kind: TrieKind, mid_lexeme: bool) -> bool {
    match bytes.first() {
        None => false,
        Some(&first) => match kind {
            TrieKind::Ident => is_ident_tail(first),
            TrieKind::ClassPath => is_ident_tail(first) || first == b':',
            TrieKind::Str => first == b'\'' || mid_lexeme,
            TrieKind::IdentOrStr => is_ident_tail(first) || first == b'\'' || mid_lexeme,
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

    /// Two classes on purpose, one a strict byte prefix of the other (`A` ⊂
    /// `Ab`): the real `Country` / `Countrylanguage` shape N3c's close policy
    /// must not break.
    const SAMPLE: &str = r#"{
      "db_id": "d", "db_path": "spider::d::Db",
      "classes": { "A": { "simple_name": "A",
        "properties": [{"name": "countryName", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
                       {"name": "country", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}] },
        "Ab": { "simple_name": "Ab",
        "properties": [{"name": "country", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}] } },
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
            &[],
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
    fn n3c_splits_the_continuation_a_whole_source_path_owes() {
        // A *whole* class path owes its `.all()` dot and nothing else; the store
        // path owes its `->` and nothing else. Both mask EOS, so neither can end
        // a query on its own.
        let v = vocab(&[b".", b"->", b"-", b" ", b"(", b"::", b"x"]);
        let eos = v.len() as u32;
        let (applied, class) = run_prefix(&L2Position::SourceIdent, &[], b"A", v.clone());
        assert!(applied);
        assert!(bit(&class, 0), "a class path keeps its navigation dot");
        for id in [1u32, 2, 3, 4, 5, 6, eos] {
            assert!(!bit(&class, id), "token {id} must be masked after a class");
        }
        let (_applied, store) = run_prefix(&L2Position::SourceIdent, &[], b"spider::d::Db", v);
        assert!(bit(&store, 1), "a store path keeps its `->` step");
        assert!(bit(&store, 2), "…including the `-` it may arrive as");
        for id in [0u32, 3, 4, 5, 6, eos] {
            assert!(
                !bit(&store, id),
                "token {id} must be masked after the store"
            );
        }
    }

    #[test]
    fn n3c_keeps_a_longer_source_path_that_a_shorter_one_prefixes() {
        // `A` is a whole class path *and* a strict prefix of `Ab`. The close
        // policy must not stop the trie walking on into the longer name — the
        // real `Country` / `Countrylanguage` trap.
        let v = vocab(&[b"b", b"->"]);
        let (_applied, mask) = run_prefix(&L2Position::SourceIdent, &[], b"A", v);
        assert!(bit(&mask, 0), "the longer sibling path stays reachable");
        assert!(
            !bit(&mask, 1),
            "the arrow is still masked at the shorter one"
        );
    }

    #[test]
    fn store_method_keeps_the_real_name_and_masks_every_other_arrowed_method() {
        let v = vocab(&[b"tableReference", b"table", b"tableToTDS", b"limit", b"'x'"]);
        let (applied, mask) = run(&L2Position::StoreMethod, &[], v);
        assert!(applied);
        assert!(bit(&mask, 0), "the real store method survives");
        assert!(bit(&mask, 1), "a leading prefix of it survives");
        assert!(!bit(&mask, 2), "a non-store method is masked");
        assert!(!bit(&mask, 3), "a generic collection builtin is masked");
        assert!(
            !bit(&mask, 4),
            "a quoted literal is masked, not passed through"
        );
    }

    #[test]
    fn source_method_keeps_all_and_a_leading_prefix_but_masks_a_quoted_literal() {
        // S1: the identifier right after a pipeline-source classpath's `.` must
        // be exactly `all`. Unlike every other trie rule here, a quoted literal
        // is never legal at this position either (`Class.'name'` is never valid
        // Pure, verified live), so `IdentOrStr` must mask it too — the bug this
        // regression guards: an earlier version used plain `Ident`, which left
        // every `Str`-shaped candidate (a quoted phantom property) completely
        // unnarrowed, since `fill_trie` keeps a non-candidate token unconditionally.
        let v = vocab(&[b"all", b"a", b"'name'", b"Xy"]);
        let (applied, mask) = run(&L2Position::SourceMethod, &[], v);
        assert!(applied);
        assert!(bit(&mask, 0), "the exact method name survives");
        assert!(
            bit(&mask, 1),
            "a leading prefix of the method name survives"
        );
        assert!(
            !bit(&mask, 2),
            "a quoted literal is masked, not passed through"
        );
        assert!(!bit(&mask, 3), "an unrelated identifier is masked");
    }

    #[test]
    fn source_method_arg_keeps_the_closer_and_milestone_dates_but_masks_a_phantom_argument() {
        // The position right after the source method's own `(` (or after a
        // comma inside it): a milestoning date argument legally appears here
        // (bitemporal `Firm.all(%latest, %latest)`, confirmed in the corpus'
        // `differential_l1.jsonl`), so `Lexeme::Date` must survive alongside the
        // matching closer and whitespace — but the exact phantom-argument shapes
        // the walker was observed emitting (`Class.all('French')`,
        // `Class.all(all)`) must still be masked.
        let v = vocab(&[
            b")",           // 0: the matching closer — kept
            b"  ",          // 1: inter-token whitespace — kept
            b"%latest",     // 2: a symbolic milestoning literal — kept
            b"%2018-01-01", // 3: a numeric date literal — kept
            b"all",         // 4: an identifier (a phantom argument) — masked
            b"'name'",      // 5: a string literal argument — masked
            b"5",           // 6: a numeric literal argument — masked
            b"$",           // 7: a refVar argument — masked
            b"(",           // 8: a nested call argument — masked
            b",", // 9: not legal at L1 here either, but never a candidate structure — masked
        ]);
        let (applied, mask) = run(&L2Position::SourceMethodArg, &[], v);
        assert!(applied);
        assert!(bit(&mask, 0), "the matching closer survives");
        assert!(bit(&mask, 1), "inter-token whitespace survives");
        assert!(bit(&mask, 2), "a symbolic milestoning literal survives");
        assert!(bit(&mask, 3), "a numeric date literal survives");
        assert!(!bit(&mask, 4), "an identifier argument is masked");
        assert!(!bit(&mask, 5), "a string literal argument is masked");
        assert!(!bit(&mask, 6), "a numeric literal argument is masked");
        assert!(!bit(&mask, 7), "a refVar argument is masked");
        assert!(!bit(&mask, 8), "a nested call argument is masked");
        assert!(!bit(&mask, 9), "a comma is masked");
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

    /// T6 clears exactly the four ordered comparators and nothing else — the
    /// equality pair and every collapse continuation the spec names stay
    /// admissible, which is what keeps the gold anchors
    /// (`$c.fk1DefaultCountrylanguage->filter(…)->isEmpty()`) replayable.
    #[test]
    fn ordered_operand_masks_only_the_ordered_comparators() {
        let masked: &[&[u8]] = &[b"<", b">", b"<=", b">="];
        let kept: &[&[u8]] = &[b"==", b"!=", b"->", b".", b")", b"isEmpty"];
        let tokens: Vec<&[u8]> = masked.iter().chain(kept).copied().collect();
        let (applied, mask) = run(&L2Position::OrderedOperand, &[], vocab(&tokens));
        assert!(applied);
        for (id, token) in masked.iter().enumerate() {
            assert!(
                !bit(&mask, id as u32),
                "the ordered comparator {:?} has no overload for a non-scalar operand",
                String::from_utf8_lossy(token)
            );
        }
        for (offset, token) in kept.iter().enumerate() {
            assert!(
                bit(&mask, (masked.len() + offset) as u32),
                "{:?} stays admissible after a non-scalar navExpr",
                String::from_utf8_lossy(token)
            );
        }
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
            &[],
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
            &[],
            grammar.vocab(),
            grammar.eos_bit(),
        );
        assert!(
            bit(&second, 1) && !bit(&second, 0),
            "after clear the second stream keeps 'ghost' and masks 'cnt' (a no-op clear would return the stale 'cnt' mask)"
        );
    }
}
