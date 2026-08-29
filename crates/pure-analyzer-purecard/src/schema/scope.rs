//! The L2 scope-tracking state machine (`docs/spec/schema.md` §6.4).
//!
//! L1's byte-PDA surfaces only the *lexical* anchor (`AfterDot`, `ExpectSource`,
//! a comparison operator state); it cannot know which class a `$var` is bound to,
//! the class a navigation has reached, or whether the pipeline has become a
//! relation — the context-sensitive facts §6.1 forbids a PDA from carrying. The
//! [`ScopeTracker`] threads that small typed scope through the parse **in
//! lockstep** with the byte-PDA (advanced token-by-token from
//! [`DecoderSession`](crate::DecoderSession)), and at each identifier/operand
//! position yields an [`L2Position`] the narrower keys on.
//!
//! It never widens: an unresolved or unknown scope yields
//! [`L2Position::None`] (pass-through), so a position the tracker cannot type is
//! left exactly as L1 allowed it.

use std::collections::{HashMap, HashSet};

use crate::grammar::pda::{Frame, LexKind, Pda, State, Step, is_ident_start, is_ident_tail, step};
use crate::schema::model::{PrimName, Resolved, Schema, TypeClass};
use crate::schema::narrow::value_ident_constrains;

/// The first byte of a two-byte operator the PDA has **already consumed** when a
/// token opens at `anchor` — the byte a [`classify`] of that token's own bytes
/// alone cannot see.
///
/// [`flush_gap`](ScopeTracker::flush_gap) munches a two-byte operator whole
/// *within* one token, but a vocabulary that offers `-` and `>` as separate
/// tokens splits it across two, and then no token's bytes are `->` at all: the
/// step arrow is read as a dash and a comparison, and every rule keyed on
/// [`on_arrow`](ScopeTracker::on_arrow) — N3c's store-method set, T3's reducer
/// arming — is silently bypassed. Live-attested: `|…::Db->min('default'…)` was
/// walked past N3c precisely this way (`-`, `>`, `min` as three tokens), and the
/// engine rejected it with "Can't find a match for function
/// 'min(Database[1],…)'".
///
/// These are the PDA's own "first half consumed" states, minus the one whose
/// reconstruction no consumer could observe: [`State::SawAmp`] is left out
/// because `&&` and a lone `&` both [`classify`] as [`Lexeme::Other`], so an arm
/// for it would be a no-op no test could pin (and an unkillable mutant — the
/// hazard issue #55 Phase 2 recorded). Every state listed changes the verdict:
/// `->` is an [`Arrow`](Lexeme::Arrow) where a lone `>` is a comparison, `==`
/// `!=` `>=` `<=` are [`Cmp`](Lexeme::Cmp) where a lone `=` is
/// [`Other`](Lexeme::Other), and `||` is `Other` where a lone `|` is the lambda
/// binder [`Pipe`](Lexeme::Pipe).
const fn pending_operator_byte(anchor: State) -> Option<u8> {
    match anchor {
        State::SawDash | State::SourceDash => Some(b'-'),
        State::SawEq => Some(b'='),
        State::SawBang => Some(b'!'),
        State::SawGt => Some(b'>'),
        State::SawLt => Some(b'<'),
        State::SawPipe => Some(b'|'),
        _ => None,
    }
}

/// [`classify`] a token read from the PDA state it opened at, so a two-byte
/// operator split across two tokens is classified as the operator it is rather
/// than as its second byte alone (see [`pending_operator_byte`]).
///
/// Only a *single-byte* token that genuinely completes one of the operators
/// ([`is_two_byte_op`]) is treated this way: anything longer opened its own
/// lexeme, and a byte that does not complete the operator (the whitespace of
/// `weight < 3500`, the digit of `-5`) is still its own token.
fn classify_at(bytes: &[u8], anchor: State) -> Lexeme {
    match (pending_operator_byte(anchor), bytes) {
        (Some(first), [second]) if is_two_byte_op(first, *second) => classify(&[first, *second]),
        _ => classify(bytes),
    }
}

/// Whether `a`, `b` begin one of the two-byte operators the grammar recognises
/// (`-> == != <= >= && ||`). A structural-gap walk munches these whole so an
/// operator never fragments into mis-classified single bytes (`>` alone reads as
/// a comparison).
const fn is_two_byte_op(a: u8, b: u8) -> bool {
    matches!(
        (a, b),
        (b'-', b'>')
            | (b'=', b'=')
            | (b'!', b'=')
            | (b'<', b'=')
            | (b'>', b'=')
            | (b'&', b'&')
            | (b'|', b'|')
    )
}

/// `bytes` as the operator it forms at `anchor`: a two-byte operator split across
/// two tokens is rejoined with the byte the anchor is holding, exactly as
/// [`classify_at`] rejoins it. Every other token is returned unchanged.
///
/// Split out rather than folded into [`classify_at`] because N4b needs the
/// *bytes* (`&&` and `||` both classify as [`Lexeme::Other`], which cannot tell
/// them apart), while `classify_at` needs the [`Lexeme`].
fn operator_bytes(bytes: &[u8], anchor: State) -> Vec<u8> {
    match (pending_operator_byte(anchor), bytes) {
        (Some(first), [second]) if is_two_byte_op(first, *second) => vec![first, *second],
        _ => bytes.to_vec(),
    }
}

/// A lexical token, classified from its raw bytes — the granularity the tracker
/// and narrower reason over. Whole identifiers/classpaths, string/number/date
/// literals, and the operators that drive scope transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lexeme {
    /// Inter-token whitespace (skipped by the scope machine).
    Ws,
    /// An identifier or `::`-joined classpath; carries its text.
    Ident(String),
    /// A single-quoted string literal; carries its unescaped content as raw bytes
    /// (byte-exact, so the N6 column key never desyncs from the trie through a
    /// lossy UTF-8 round-trip).
    Str(Vec<u8>),
    /// A numeric literal.
    Number,
    /// A `%`-prefixed date/time literal.
    Date,
    /// The `->` step connector.
    Arrow,
    /// A `.` navigation dot.
    Dot,
    /// A `$` refVar sigil.
    Dollar,
    /// A lone `|` (lambda binder pipe, or the query opener at `Start`).
    Pipe,
    /// A comparison operator (`== != < > <= >=`); carries its operand type-class
    /// eligibility (all are comparisons; ordered-vs-equality is a deferred T2).
    Cmp,
    /// An opening delimiter `(` `[` `{`.
    Open,
    /// A closing delimiter `)` `]` `}`.
    Close,
    /// An argument/list separator `,`.
    Comma,
    /// Any other byte(s) not load-bearing for L2 (`; : ! + - * / && || =`).
    Other,
}

/// Classify a token's raw bytes into a [`Lexeme`].
pub(crate) fn classify(bytes: &[u8]) -> Lexeme {
    if bytes.is_empty() || bytes.iter().all(u8::is_ascii_whitespace) {
        return Lexeme::Ws;
    }
    match bytes {
        b"->" => return Lexeme::Arrow,
        b"==" | b"!=" | b"<=" | b">=" | b"<" | b">" => return Lexeme::Cmp,
        b"." => return Lexeme::Dot,
        b"$" => return Lexeme::Dollar,
        b"|" => return Lexeme::Pipe,
        b"," => return Lexeme::Comma,
        b"(" | b"[" | b"{" => return Lexeme::Open,
        b")" | b"]" | b"}" => return Lexeme::Close,
        _ => {}
    }
    let first = bytes[0];
    if first == b'\'' {
        return Lexeme::Str(unquote(bytes));
    }
    if first == b'%' {
        return Lexeme::Date;
    }
    if first.is_ascii_digit() || (first == b'-' && bytes.get(1).is_some_and(u8::is_ascii_digit)) {
        return Lexeme::Number;
    }
    if is_ident_start(first) {
        // An identifier or `::`-joined classpath.
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Lexeme::Ident(text.to_owned());
        }
    }
    Lexeme::Other
}

/// Strip the surrounding single quotes and undouble `''` from a string literal's
/// raw bytes, yielding its logical content (§5.5 quote doubling).
fn unquote(bytes: &[u8]) -> Vec<u8> {
    let inner = bytes
        .strip_prefix(b"'")
        .and_then(|rest| rest.strip_suffix(b"'"))
        .unwrap_or(bytes);
    // Undouble `''` -> `'` on the raw bytes — byte-exact, no UTF-8 round-trip that
    // a `�` could corrupt. Consuming the slice head each step (dropping the paired
    // second quote on a match) keeps the walk advancing without an index cursor to
    // mutate into a non-terminating loop.
    let mut out = Vec::with_capacity(inner.len());
    let mut rest = inner;
    while let Some((&b, tail)) = rest.split_first() {
        out.push(b);
        rest = if b == b'\'' && tail.first() == Some(&b'\'') {
            &tail[1..]
        } else {
            tail
        };
    }
    out
}

/// The schema-consistency constraint that applies at the current position — the
/// key the narrower ([`narrow_into`](crate::schema::narrow::narrow_into)) builds a legal
/// set from. [`None`](L2Position::None) means "no L2 constraint here" (the L1
/// mask passes through unchanged).
///
/// `#[doc(hidden)] pub`, re-exported as `crate::schema::L2Position`:
/// test-support surface (issue #59's per-named-rule coverage bullet), not
/// part of the crate's documented public contract (excluded from the
/// `cargo public-api` snapshot) — a plain `pub(crate)` cannot cross the crate
/// boundary integration tests under `tests/` compile behind (mirrors
/// `grammar::pda::ALL_STATES`'s own promotion for the same reason;
/// re-exported individually rather than promoting the whole `scope` module,
/// so no other already-public item in this file gains a second,
/// newly-public path). Read via
/// [`DecoderSession::active_l2_position`](crate::DecoderSession::active_l2_position).
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L2Position {
    /// N3: the pipeline source classpath must be a real class (or the store).
    SourceIdent,
    /// S1: the identifier right after a pipeline-source classpath's `.` must be
    /// exactly [`SOURCE_METHOD`] (`all`) — `ClassScope` is only ever entered via
    /// `ClassPath.all()` (`docs/spec/schema.md` §6.4's S1/S3 progression), so
    /// nothing else is a legal continuation of *this* dot. Kept distinct from
    /// [`Member`](L2Position::Member): this dot's `dot_base` is deliberately
    /// `None` (no class is being navigated *from* yet), so folding it into that
    /// rule would also have to narrow every other class-typed `.` with no base
    /// — wrongly reaching `EnumPath.IDENT` value positions (`SortDirection.ASC`)
    /// that share the same `AfterDot` state but anchor at a value position, not
    /// a source position (see [`on_dot`](ScopeTracker::on_dot)'s doc comment).
    SourceMethod,
    /// N3's grammar production for the pipeline source is the literal
    /// `classpath ".all()"` (`docs/spec/schema.md` §7) — the call ordinarily
    /// takes no argument, with one carved-out exception: bitemporal milestoning
    /// legally passes zero, one, or two comma-separated milestone/date literals
    /// (`Firm.all(%latest, %latest)`, corpus `differential_l1.jsonl`;
    /// `docs/spec/grammar.md`'s `milestoneLit`/`dateLit`). [`SourceMethod`](L2Position::SourceMethod)
    /// narrows the identifier to exactly `all`; this narrows the position right
    /// after the call's own opening `(` (or a following comma) to admit only
    /// whitespace, a milestoning date, or — at the first slot — the matching
    /// closer, so the call cannot smuggle a phantom identifier/string/number
    /// argument in behind it (confirmed live: `Class.all('French')` /
    /// `Class.all(all)` both fail to compile). It does not cap the argument
    /// count at two or validate a milestone symbol beyond its lexical shape —
    /// that residue, like every other `%`-literal position in this overlay, is
    /// left to the compiler oracle. Armed only for [`SOURCE_METHOD`]'s own
    /// call — other niladic builtins (`->toOne()`, aggregation reducers) may
    /// have the same latent gap but are not narrowed here; out of scope for
    /// this rule.
    SourceMethodArg,
    /// N3c (store arm): the identifier right after a pipeline-source **store**
    /// path's own `->` must name a real store method. A store path denotes a
    /// `meta::relational::metamodel::Database`, not a class extent, so the only
    /// thing it legally produces is a table reference — every other method the
    /// vocabulary offers either has no `Database` overload at all (live:
    /// `|…::Db->tableToTDS()` → "Can't find a match for function
    /// 'tableToTDS(Database[1])'") or matches a generic collection builtin and
    /// returns the store right back (`|…::Db->limit(3)` → `Database`), which is
    /// never a query. The legal set is read off the corpus, not invented: across
    /// the 5034 gold queries a store path is followed by `->tableReference`
    /// 8455 times and by nothing else.
    StoreMethod,
    /// N3d: a value slot inside a store method's own call ([`STORE_METHODS`]) —
    /// right after its `(`, or right after one of its commas. Every store-method
    /// parameter is a string literal, per the engine's own signature quoted back
    /// in its rejection (`tableReference(Database[1],String[1],String[1]):Table[1]`),
    /// so nothing else may open a slot here. The matching closer is masked too,
    /// which is the arity half of the rule: an opened slot owes its argument, so
    /// `->tableReference()` and a one-argument `->tableReference('T')` cannot
    /// complete. Corpus-attested, not invented — all 8455 store-method calls
    /// across the 5034 gold queries pass exactly two single-quoted strings.
    StoreMethodArg,
    /// N3d's separator half: the position right after a *completed* store-method
    /// argument, carrying how many arguments the call has completed so far. A `,`
    /// is legal only while arguments remain, the call's own `)` only once the
    /// arity is met, and an operator never is — the walker's residue here was
    /// `->tableReference('a'=='b')` and `->tableReference('a'*'b')`, which the
    /// engine reports as a *signature* mismatch
    /// (`tableReference(Database[1],Boolean[1])`) rather than a parse error, so
    /// no L1 tightening can reach it.
    StoreMethodArgSep {
        /// Whether the call still owes at least one more argument. The tracker
        /// derives it from the arity [`STORE_METHODS`] declares for the open
        /// call and the commas it has emitted, so the arity stays a single fact
        /// stated once beside the method's own name.
        remaining: bool,
    },
    /// N3e: the position right after the source method's own call closed —
    /// `Class.all()`, the class **extent**. An extent is a `T[*]` collection, so
    /// every binary operator the vocabulary offers mismatches it by construction
    /// (live: `{|…::ModelList.all() && 'Year_T1'}` → "Can't find a match for
    /// function 'and(ModelList[*],String[1])'"; `{|…::CarMakers.all()*'Accelerate_T2'}`
    /// → "Collection element must have a multiplicity [1]"). What an extent *does*
    /// take is a pipeline step, a property navigation that maps over it, or
    /// nothing at all — and that is exactly what the corpus shows: across the 5034
    /// gold queries a closed `.all()` is followed by `->` 438 times, by a `.`
    /// property 37 times, and by end-of-query 25 times, and by nothing else. The
    /// same three, and only those three, in the modern-dialect seeds and the
    /// engine-labelled differential corpus.
    ///
    /// The direct successor of [`StoreMethod`](L2Position::StoreMethod)'s own
    /// rule: N3c stops a method being arrowed off the bare `Class<T>[1]` metatype,
    /// this stops one being *operated on* once `.all()` has produced the extent.
    SourceExtent {
        /// Whether the `-` that opens the step arrow has already been emitted, so
        /// only the `>` that completes it may follow. Without this second half a
        /// vocabulary offering `-` and `>` separately would reassemble an
        /// arithmetic minus one byte at a time (live:
        /// `{|…::Countrylanguage.all() -'HeadOfState_T1_3'}` → "Collection element
        /// must have a multiplicity [1]").
        after_dash: bool,
    },
    /// N3g: the value slot inside a **receiver-only** builtin's arrow call
    /// ([`RECEIVER_ONLY_METHODS`]) — right after its `(`, or right after one of
    /// its commas. The arity half of the treatment
    /// [`StoreMethodArg`](L2Position::StoreMethodArg) gives the store method,
    /// applied to the generic collection builtins whose *every* engine overload
    /// takes the receiver and nothing else: an arrow call has already supplied
    /// that one parameter, so the slot it opens can only be closed, never filled.
    ///
    /// Stated of the **arrow** form alone, because the plain-function form is the
    /// same overload read the other way round — there the receiver *is* the
    /// argument, and `|count(…::Country.all())` and `|isEmpty($x.name)` both
    /// compile live. Both directions are frozen.
    ReceiverOnlyArg,
    /// N4a: the position right after a **store method's** own call closed —
    /// `Db->tableReference('T','S')`, a `Table[1]`. The store-arm dual of
    /// [`SourceExtent`](L2Position::SourceExtent), and the same argument one type
    /// over: a `Table` is neither a `Boolean`, a `Number`, a `String` nor a
    /// `Date`, so every ordered, arithmetic and logical operator the vocabulary
    /// offers mismatches its overload set by construction (live:
    /// `and(Table[1],String[1])`, `or(Table[1],String[1])`,
    /// `greaterThan(Table[1],String[1])`, `lessThanEqual(Table[1],String[1])`,
    /// `divide(Table[1],String[1])`, `plus(Any[2])`, `minus(Any[2])`,
    /// `times(Any[2])`).
    ///
    /// Deliberately **subtractive**, unlike `SourceExtent`'s permit set: a bare
    /// `|…::Db->tableReference('T','S')` compiles live and returns `Table`, and
    /// `=='x'` / `!='x'` compile through `equal(Any[1],Any[1])`, so this rule
    /// clears the attested operators and leaves every closer, separator, `.`
    /// navigation and equality comparison alone.
    StoreResult {
        /// Whether the `-` that opens the step arrow has already been emitted, so
        /// only the `>` that completes it may follow — the same reassembly guard
        /// [`SourceExtent`](L2Position::SourceExtent) needs, for the same reason:
        /// `->tableToTDS()` is the store result's one real step, and `-` is the
        /// byte an arithmetic minus shares with it.
        after_dash: bool,
    },
    /// N4c: the position right after a completed **string literal** — the
    /// operator half of [`LogicalOperand`](L2Position::LogicalOperand)'s
    /// argument, read from the left instead of the right. `minus`, `times` and
    /// `divide` have no `String` overload at all (live: `minus(String[2])`,
    /// `times(String[2])`, `divide(String[1],String[1])`, and `minus(Any[2])` /
    /// `times(Any[2])` once the operands' classes differ), so `-`, `*` and `/`
    /// can never take a string literal as their left operand.
    ///
    /// Narrow by design, and the narrowness is the rule. `+` stays: `plus(String[*])`
    /// is string concatenation and compiles live. The *ordered* comparators stay:
    /// `greaterThan(String[1],String[1])` is a real overload. `&&` and `||` stay,
    /// and must — a comparison binds tighter than a conjunction, so the `&&` in
    /// the corpus's canonical `filter(x|$x.a == 'p' && $x.b == 'q')` follows a
    /// string literal while taking the whole *comparison* as its operand. Only
    /// the three operators whose left operand really is the literal are cleared.
    StrOperator {
        /// Whether the `-` that opens a step arrow has already been emitted.
        /// A string literal is arrowed all through the corpus — all 32309
        /// post-literal `-` bytes across the three corpora open a `->`, none an
        /// arithmetic minus, and `|'a'->toUpper()` compiles live — so the `-` is
        /// admitted only as the arrow, exactly as
        /// [`SourceExtent`](L2Position::SourceExtent) admits it.
        after_dash: bool,
    },
    /// N4b: the operand slot a **logical** operator (`&&`, `||`) opens. `and`/`or`
    /// have Boolean-only overloads (`and(Boolean[1],Boolean[1])`,
    /// `and(Boolean[*])`), so a string, numeric or date literal in that slot can
    /// never match — live, on both sides of the operator: `'a'&&true`,
    /// `true&&'a'`, `true&&1`, `1||true` and `%2020-01-01&&true` all fail, while
    /// `true&&true` and `('a'=='b')&&(1<2)` compile.
    ///
    /// A position of its own rather than a reuse of
    /// [`ReValue(Boolean)`](L2Position::ReValue), which stays deferred: T1 governs
    /// a *comparison*'s operand, where `equal(Any[1],Any[1])` keeps a
    /// type-mismatched literal legal beside a Boolean navExpr. The narrowing is
    /// nonetheless the same predicate, applied at a position where it does hold.
    LogicalOperand,
    /// N3f: the method-name identifier right after a class extent's own `->` —
    /// the coarse **receiver-category** position. N3e admits the step arrow off a
    /// closed `Class.all()`; this decides what that arrow may open.
    ///
    /// The mirror image of [`StoreMethod`](L2Position::StoreMethod), and
    /// deliberately the *opposite* shape. A store path denotes one type with one
    /// method, so its rule is a permit set. A class extent is a `T[*]` collection,
    /// and the generic collection builtins that legally operate on one are open-
    /// ended — `at`, `drop`, `slice`, `init`, `tail`, `first`, `last`, `reverse`,
    /// `removeDuplicates` and `fold` all compile here and appear in no corpus, so
    /// §4 forbids the allow-list that would mask them. What *is* closed is the
    /// complement this rule states: the names whose entire overload set demands a
    /// relation/store or primitive-scalar receiver, which a class extent can never
    /// present ([`EXTENT_INCOMPATIBLE_METHODS`]).
    ///
    /// A denied name is cleared at the token that **closes its lexeme**, not at
    /// its first byte — `pair` must stay walkable as the live prefix of a longer
    /// legal name, exactly as N3c's close policy keeps `Country` walkable inside
    /// `Countrylanguage`.
    ExtentMethod,
    /// N1/N2: the identifier after `.` must be a member of `class`.
    Member(String),
    /// T1: the comparison operand's literal type must match `class`.
    ReValue(TypeClass),
    /// T2: an ordered comparator (`< > <= >=`) is legal only when the completed
    /// navExpr just left of it is numeric or temporal.
    Comparator(TypeClass),
    /// T3: `sum`/`average` are legal only when the reduce lambda's declared
    /// element type is numeric; `min`/`max`/`count` are unconstrained (see
    /// `narrow::keeps_reducer`'s doc comment for the corpus evidence).
    Reducer(TypeClass),
    /// N6: a relation-column string reference must name an emitted column.
    Column,
    /// N6 (arm-R): a *bare-ident* column access `$row.<Col>` on a relation row
    /// must name an emitted column — the unquoted dual of [`Column`](L2Position::Column).
    RelationColumn,
    /// S2: the identifier after a `$` sigil must name a variable something in the
    /// stream has actually bound. Pure resolves `$x` against the lambda/`let`
    /// bindings in scope, so an unbound name is not a typing error but a missing
    /// graph element ("Can't find variable class for variable 'code'"). Keyed on
    /// the *count* of bound names rather than the names themselves: the tracker's
    /// binder record is deliberately monotonic (see
    /// [`bound_vars`](ScopeTracker::bound_vars)), so within one stream the count
    /// pins the set exactly — the same argument that makes
    /// [`Column`](L2Position::Column)'s count key exact.
    RefVar,
    /// N7: a bare identifier in a **value** position, with its own lexeme still
    /// open, may only continue into one of the shapes that give a bare word
    /// meaning in Pure — a lambda binder (`x|…`, `x: T[1]|…`), a package/class
    /// path (`meta::…::JoinType`), a member/enum-value selection (`.`), or a
    /// function application (`(`). Anything else ends the word as a standalone
    /// expression, and a standalone bare word resolves to nothing ("Can't find
    /// the packageable element 'pair'"). Unlike the trie rules this narrows no
    /// name **set** — novel binder names must stay admissible — only what may
    /// follow one, so it is keyed on nothing but the vocabulary.
    ValueIdent,
    /// No L2 constraint here — pass the L1 mask through unchanged.
    None,
}

/// The method that opens a pipeline from a class extent (`Class.all()`). A call
/// to it below the top level is a *nested* pipeline whose arm/relation state must
/// not inherit or leak the enclosing pipeline's. `pub(crate)` so `narrow.rs` can
/// build [`L2Position::SourceMethod`]'s single-name trie from the same constant.
pub(crate) const SOURCE_METHOD: &str = "all";

/// The store methods a pipeline-source store path may be arrowed into
/// ([`L2Position::StoreMethod`]), each with the number of string arguments its
/// call takes ([`L2Position::StoreMethodArg`]). `pub(crate)` so `narrow.rs`
/// builds the rule's trie from the same list.
///
/// The name set and the arity are read off the same corpus evidence: across the
/// 5034 gold queries a store path is followed by `->tableReference` 8455 times
/// and by nothing else, and every one of those 8455 calls passes exactly two
/// single-quoted strings. The engine agrees, naming the signature in its own
/// rejection: `tableReference(Database[1],String[1],String[1]):Table[1]` (the
/// receiver is the first parameter).
///
/// The arity travels *with* the name rather than as a lone constant so a second
/// store method cannot be added without stating its own — the arity is a fact
/// about one method's signature, never about the set.
pub(crate) const STORE_METHODS: &[(&str, usize)] = &[("tableReference", 2)];

/// N3f's deny set: the vocabulary method names whose **every** overload demands a
/// receiver category a class extent can never be ([`L2Position::ExtentMethod`]).
///
/// Read off the engine's own function registry, never invented. Asked for a name
/// it cannot match, the compiler prints the whole candidate set it *could* have
/// matched; for each name below not one candidate's receiver parameter admits the
/// `T[*]` a `Class.all()` produces, so no argument list can rescue the call. Two
/// receiver categories account for all of them:
///
/// * **relation / store** receivers — `agg(String[1]|FunctionDefinition<…>[1],…)`,
///   `join(Relation<T>[1]|TabularDataSet[1],…)`,
///   `renameColumns(TabularDataSet[1],…)`, `restrict(TabularDataSet[1],String[*])`,
///   `tableReference(Database[1],String[1],String[1])`, `tableToTDS(Table[1])`;
/// * **primitive scalar** receivers — `average(Float|Integer|Number[*])`,
///   `between(StrictDate|DateTime|Number|String[0..1],…)`,
///   `endsWith(String[…],String[1])`, `in(Any[1]|Any[0..1],Any[*])`,
///   `pair(U[1],V[1])`, `parseFloat(String[1])`,
///   `startsWith(String[…],String[1])`, `substring(String[1],…)`,
///   `sum(Float|Integer|Number[*])`, `toLower(String[1])`, `toString(Any[1])`,
///   `year(Date[1]|Date[0..1])`.
///
/// A class extent is neither, which is the whole rule — and why this is a
/// *deny* set rather than the permit set N3c gets for the store arm. There is no
/// closed permit set to have: `at`, `drop`, `slice`, `add`, `init`, `tail`,
/// `first`, `last`, `removeDuplicates`, `reverse` and `fold` all compile on a
/// class extent while appearing nowhere in the corpus, so an allow-list built
/// from corpus names would mask eleven legal collection builtins. Every entry
/// here was sent through the running engine on this branch first, at zero, one
/// and two literal arguments and with a lambda, before it was written down.
///
/// `pub(crate)` so `narrow.rs` builds the rule's trie from the same list.
pub(crate) const EXTENT_INCOMPATIBLE_METHODS: &[&str] = &[
    "agg",
    "average",
    "between",
    "endsWith",
    "in",
    "join",
    "pair",
    "parseFloat",
    "renameColumns",
    "restrict",
    "startsWith",
    "substring",
    "sum",
    "tableReference",
    "tableToTDS",
    "toLower",
    "toString",
    "year",
];

/// N3g's set: the builtins whose **entire** engine overload set is
/// receiver-only, so an arrow call of one takes no further argument
/// ([`L2Position::ReceiverOnlyArg`]).
///
/// Read off the engine's own registry, exactly as N3f's deny set is. Asked for
/// one of these names with an argument, the compiler prints back the complete
/// candidate list it *could* have matched, and every candidate has arity one —
/// the receiver:
///
/// * `count(Any[*]):Integer[1]`;
/// * `isEmpty(Any[0..1]):Boolean[1]`, `isEmpty(Any[*]):Boolean[1]`;
/// * `isNotEmpty(Any[0..1]):Boolean[1]`, `isNotEmpty(Any[*]):Boolean[1]`;
/// * `size(Relation<T>[1]):Integer[1]`, `size(Any[*]):Integer[1]`;
/// * `toOne(T[*]):T[1]`.
///
/// The receiver parameter is `Any`/`T`, so the arity is a fact about the name
/// alone and not about what it is arrowed off — live-confirmed on a class
/// extent, a `TableTDS`, a primitive collection and a `filter` result alike. The
/// corpus agrees and adds nothing: across the 5034 gold queries these names are
/// called 3048 times and **never** with an argument.
///
/// Names the engine would not adjudicate are left out rather than guessed:
/// `->distinct('x')` answers `RuntimeException: Not possible!` with no candidate
/// list, so `distinct` is not here (§4 — invent no constraint the oracle does
/// not state), and `sort` is excluded outright, taking a comparator argument in
/// all 1048 of its corpus calls.
///
/// `pub(crate)` so `narrow.rs` and the tracker share one list.
pub(crate) const RECEIVER_ONLY_METHODS: &[&str] =
    &["count", "isEmpty", "isNotEmpty", "size", "toOne"];

/// The lone `-` a vocabulary that splits the step connector offers as its own
/// token — the one token N3e's extent arming survives (see
/// [`L2Position::SourceExtent`]).
const STEP_DASH_TOKEN: &[u8] = b"-";

/// The logical operators whose operand slot N4b narrows
/// ([`L2Position::LogicalOperand`]). `classify` folds both into
/// [`Lexeme::Other`] — they carry no other L2 meaning — so the arming reads the
/// raw token bytes.
const LOGICAL_OPERATORS: &[&[u8]] = &[b"&&", b"||"];

/// `name`'s declared argument count if it is a [`STORE_METHODS`] entry.
fn store_method_arity(name: &str) -> Option<usize> {
    STORE_METHODS
        .iter()
        .find(|(method, _)| *method == name)
        .map(|(_, arity)| *arity)
}

/// Which kind of pipeline source a schema-resolved source path is — the fact
/// N3c's two arms split on. A class path denotes a `Class<T>[1]` metatype and
/// owes its `.all()`; a store path denotes a `Database` and owes its `->`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// A schema class path (`Schema::has_class`).
    Class,
    /// The schema's store/`Db` path.
    Store,
}

/// The least delimiter depth at which an `all()` call is *nested*: the top-level
/// `|Class.all()` sits at depth 1, so a call at depth 2 or deeper is inside a
/// lambda body and heads a nested pipeline.
const NESTED_SOURCE_MIN_DEPTH: u32 = 2;

/// Names the L1 methods that establish a named relation scope (§6.4.5/6.4.6):
/// after one of these calls closes, subsequent column references are narrowed
/// (N6). Their own argument lambdas still run over the pre-relation scope, so a
/// reference *inside* them is not narrowed.
const ESTABLISHING_METHODS: &[&str] = &["project", "groupBy", "olapGroupBy"];

/// Names the L1 positions whose string argument is a relation-column *reference*
/// (§6.5 N6): the TDS getters and the sort/column selectors.
const REF_METHODS: &[&str] = &[
    "getInteger",
    "getFloat",
    "getString",
    "getBoolean",
    "sort",
    "asc",
    "desc",
    "restrict",
];

/// An identifier / string lexeme being accumulated across BPE sub-tokens (§6.4).
///
/// Byte-level BPE fragments a schema identifier (`countryName` → `country` +
/// `Name`); the tracker buffers the fragments and dispatches the scope transition
/// (resolve / bind / emit) only once the *whole* lexeme completes, so
/// [`resolve_member`](ScopeTracker::resolve_member) sees the whole name. The
/// buffered bytes also serve as the trie-walk prefix the narrower reads (B1), so
/// the constraint persists across the sub-tokens rather than firing only at the
/// leading one.
#[derive(Debug, Clone)]
struct Pending {
    /// The lexeme class being accumulated.
    kind: LexKind,
    /// The bytes emitted since the anchor (the trie-walk prefix, and the whole
    /// lexeme once it closes).
    buf: Vec<u8>,
    /// The PDA state where the lexeme opened — the "pre-state" the buffered token
    /// is dispatched under, so its scope transition matches the whole-token path.
    anchor: State,
    /// The L2 rule constraining this lexeme (or [`None`](L2Position::None) for an
    /// unnarrowed lexeme such as a keyword or a plain operand), read by
    /// [`position`](ScopeTracker::position) while the lexeme is in flight.
    pos: L2Position,
}

/// A lambda binder's shadowed prior binding, saved at its `|` and restored when
/// the enclosing delimiter closes (§6.4 lexical scoping). `prev_class` is `None`
/// when the name had no class binding to restore.
#[derive(Debug, Clone)]
struct BinderSave {
    depth: u32,
    name: String,
    prev_class: Option<Option<String>>,
    prev_relation_row: bool,
    prev_reducer_type: Option<TypeClass>,
}

/// The enclosing pipeline's state, snapshotted at a lambda body's binder pipe and
/// restored when the body's delimiter closes (§6.4 lexical scoping). A lambda body
/// is the one lexical region where a *nested pipeline* — headed by `Class.all()`
/// or by a navigation (`$x.rel->groupBy(~[…])`) — can appear; restoring on close
/// stops that subquery's class and arm/relation state from leaking out and
/// re-classifying an outer binder or navigation.
#[derive(Debug, Clone)]
struct ScopeSave {
    depth: u32,
    prev_cur_class: Option<String>,
    prev_rel_explicit: bool,
    prev_saw_tilde_bracket: bool,
}

/// The §6.4 scope machine, advanced in lockstep with the byte-PDA.
///
/// It holds the pipeline element class, the lambda variable bindings, the
/// in-flight navigation cursor, and the relation-scope / column-reference
/// bookkeeping N6 keys on. Every field defaults to "unknown", and every
/// transition that cannot be typed leaves the scope unknown — so
/// [`position`](ScopeTracker::position) degrades to [`L2Position::None`] rather
/// than risk masking a real token.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScopeTracker {
    /// The identifier / string lexeme accumulating across sub-tokens, if any.
    pending: Option<Pending>,
    /// The current pipeline element class (the most recent `Class.all()` source).
    cur_class: Option<String>,
    /// Lambda variable → the class it is bound to (`None` = unknown, e.g. a TDS
    /// row binder), for N1 rooted at `$var`.
    var_class: HashMap<String, Option<String>>,
    /// A `$` was just seen; the next identifier is its refVar name.
    pending_refvar: Option<String>,
    /// A `.` was just seen; the class we are navigating *from* (N1/N2 base), or
    /// `None` when the dot is not a member navigation (`.all()`, `.getX`).
    dot_base: Option<String>,
    /// The class a navigation chain has reached so far (feeds N2).
    nav_cursor: Option<String>,
    /// The type-class of the most recently completed primitive navExpr — read by
    /// the *next* comparison operator to arm T1 (`cmp_pending`), and by the
    /// `AfterValue`/`AfterName` anchor right in front of it to arm T2
    /// (`Comparator`).
    last_resolved: Option<TypeClass>,
    /// The class the most recently completed navExpr resolved to (a to-many/class
    /// nav receiver), used to bind a following method lambda's variable.
    last_nav_class: Option<String>,
    /// T1 is armed: the next operand position expects a literal of this class.
    cmp_pending: Option<TypeClass>,
    /// Lambda variable → its declared primitive element type (`y: Integer[*]|…`),
    /// for T3's aggregation-reducer legality check. Distinct from [`var_class`](Self::var_class),
    /// which only ever holds a *schema class* binding.
    var_reducer_type: HashMap<String, TypeClass>,
    /// A binder name paired with its colon-typed primitive annotation
    /// (`("y", Numeric)` for `y: Integer[*]|…`), captured together at the type
    /// identifier itself (see the field's use site) and awaiting that binder's
    /// own `|` to bind the pair into [`var_reducer_type`](Self::var_reducer_type).
    pending_binder_element: Option<(String, TypeClass)>,
    /// T3 is armed: a `->` was just seen right after a bare refVar bound to a
    /// primitive element type, so the next identifier is a reducer-method name
    /// legal only for that type.
    awaiting_reducer: Option<TypeClass>,
    /// The first identifier of the current lambda argument (its binder name).
    lambda_first_ident: Option<String>,
    /// Receiver class captured at a `->`, awaiting the method's `(` to become the
    /// enclosing paren's lambda-binding class.
    pending_arrow_receiver: Option<Option<String>>,
    /// Per-open-paren lambda-binding receiver class.
    paren_receiver: Vec<Option<String>>,
    /// Paren depths at which an establishing op is still open.
    est_stack: Vec<u32>,
    /// Paren depths at which a column-reference method is still open.
    ref_stack: Vec<u32>,
    /// The current delimiter nesting depth.
    depth: u32,
    /// The most recent identifier — the candidate method name before a `(`.
    last_ident: Option<String>,
    /// Have we passed a *closed* establishing op (a named relation exists)?
    rel_explicit: bool,
    /// Has any `~[…]` opened? Latches the pipeline as arm-R (the Relation/Function
    /// API); a pure arm-A/TDS pipeline never opens one. Gates arm-R relation-row
    /// column narrowing so a TDS `$r.getString(…)` getter is never mistaken for a
    /// column and masked.
    saw_tilde_bracket: bool,
    /// Per-open-delimiter flag: was this delimiter a `~[…]` column set? Pushed and
    /// popped in lockstep with [`paren_receiver`](Self::paren_receiver), so an
    /// identifier at the `ExpectValue` key position of the innermost open delimiter
    /// is a column name exactly when the top flag is set.
    tilde_open: Vec<bool>,
    /// Lambda variables bound to an arm-R **relation row** (a post-establishing-op
    /// row over the emitted-column universe), so `$row.<Col>` is a bare-ident column
    /// access (N6), not a class member navigation.
    relation_row_vars: HashSet<String>,
    /// The binding a lambda binder shadowed, restored when its enclosing delimiter
    /// closes — so a lambda scope cannot leak its binder into an outer scope that
    /// reuses the name (`filter(x|…innerRelation with x…) … $x.member`). Without it a
    /// re-used name keeps the inner scope's class/relation-row classification and
    /// masks a valid outer navigation.
    binder_saves: Vec<BinderSave>,
    /// The enclosing pipeline's class + arm/relation state at each open lambda body,
    /// saved at the binder pipe and restored when the body's delimiter closes — so a
    /// nested pipeline inside the body (a `Class.all()` *or* a navigation-headed
    /// `$x.rel->groupBy(~[…])` subquery) cannot leak its source class, `rel_explicit`,
    /// or `saw_tilde_bracket` out to re-classify an outer binder or navigation.
    scope_saves: Vec<ScopeSave>,
    /// A `.` was just seen over a relation-row binder; the following identifier is a
    /// bare-ident column reference ([`RelationColumn`](L2Position::RelationColumn)).
    dot_is_column: bool,
    /// Which kind of source path the identifier just dispatched from a
    /// source-triggering anchor (`ExpectSource`/`BlockStmt`/`BlockStmtClose`)
    /// was, when it was one at all — read and cleared by whichever of `.`
    /// ([`on_dot`](ScopeTracker::on_dot)) or `->`
    /// ([`on_arrow`](ScopeTracker::on_arrow)) follows it, the only two
    /// continuations a source path has. Always [`None`] for `let` (also legal at
    /// that anchor, per `schema.source_paths().chain(once(LET_KEYWORD))` in
    /// `narrow.rs`), since `let` is not itself a source path.
    source_path_seen: Option<SourceKind>,
    /// A `.` was just seen right after a source classpath (`source_path_seen`
    /// was set); the following identifier must be [`SOURCE_METHOD`] (S1). Read
    /// once by [`opening_position`](ScopeTracker::opening_position) when that
    /// identifier's lexeme opens, then cleared the same way `dot_base` is
    /// consumed by [`resolve_member`](ScopeTracker::resolve_member).
    awaiting_source_method: bool,
    /// A `->` was just seen right after a **store** source path; the following
    /// identifier must name a store method ([`L2Position::StoreMethod`]).
    /// Consumed one token later, exactly like
    /// [`awaiting_reducer`](Self::awaiting_reducer): the arrow sets it and any
    /// following non-whitespace token clears it.
    awaiting_store_method: bool,
    /// The call just opened via `on_open` was a [`STORE_METHODS`] entry's own,
    /// carrying that method's declared argument count. Every value slot inside
    /// it is a [`L2Position::StoreMethodArg`] and every completed argument is
    /// followed by a [`L2Position::StoreMethodArgSep`]. Cleared at the matching
    /// `on_close` for the same reason
    /// [`in_source_method_args`](Self::in_source_method_args) is.
    store_call_arity: Option<usize>,
    /// How many commas the open store-method call has emitted, so the separator
    /// position knows how many arguments are already complete (a `,` only ever
    /// follows a completed argument, so `complete = commas + 1`).
    store_call_commas: usize,
    /// The call just opened via `on_open` was [`SOURCE_METHOD`]'s own — the
    /// value position immediately following it (and after each following
    /// comma) admits only whitespace, a milestoning date, or the call's own
    /// closer, never a phantom identifier/string/number argument
    /// ([`L2Position::SourceMethodArg`]). Cleared unconditionally at the matching
    /// `on_close`, not merely consumed on first read like
    /// [`awaiting_source_method`](Self::awaiting_source_method): the value
    /// position it targets can be re-queried across whitespace tokens before
    /// the closer commits, but once the call's own delimiter closes the flag
    /// must not survive to wrongly mask an unrelated value position reached
    /// without an intervening `on_open` (a comparison operand directly
    /// following the call's close, e.g. a hypothetical `A.all() == 5`).
    in_source_method_args: bool,
    /// The source method's own call ([`SOURCE_METHOD`]) has just closed, so the
    /// next token sits on the class extent ([`L2Position::SourceExtent`]).
    /// Consumed one token later, exactly like
    /// [`awaiting_store_method`](Self::awaiting_store_method): the close sets it
    /// and any following non-whitespace token clears it.
    awaiting_extent_step: bool,
    /// N3f: the `->` just seen was the class extent's own step arrow, so the
    /// following identifier is an extent method name ([`L2Position::ExtentMethod`]).
    /// Consumed one token later, exactly like
    /// [`awaiting_store_method`](Self::awaiting_store_method): the arrow sets it
    /// and any following non-whitespace token clears it.
    awaiting_extent_method: bool,
    /// N4a: a [`STORE_METHODS`] call has just closed, so the next token sits on
    /// its `Table[1]` result ([`L2Position::StoreResult`]). Set unconditionally
    /// at every [`on_close`](ScopeTracker::on_close) from whether *that* close
    /// was the store call's own, which is what keeps an enclosing `)` from
    /// carrying the arming outward — the same discipline
    /// [`awaiting_extent_step`](Self::awaiting_extent_step) uses, and cleared on
    /// the same one-token schedule (a `-` excepted, since a split step arrow
    /// still owes its `>`).
    awaiting_store_result: bool,
    /// N3g: the call just opened via [`on_open`](ScopeTracker::on_open) was a
    /// [`RECEIVER_ONLY_METHODS`] entry's **arrow** call, so every value slot
    /// inside it is a [`L2Position::ReceiverOnlyArg`]. Cleared at the matching
    /// `on_close` for the same reason
    /// [`store_call_arity`](Self::store_call_arity) is.
    receiver_only_call: bool,
    /// Whether the identifier [`last_ident`](Self::last_ident) holds was reached
    /// straight off a `->`. N3g's arity claim is about the arrow form alone (the
    /// plain-function form spends the same single parameter on its argument), so
    /// the call shape has to travel with the name to `on_open`.
    last_ident_after_arrow: bool,
    /// N4b: the token just seen was a [`LOGICAL_OPERATORS`] entry, so the operand
    /// it opens is a [`L2Position::LogicalOperand`]. Lives exactly one
    /// non-whitespace token, like [`cmp_pending`](Self::cmp_pending).
    logical_pending: bool,
    /// N4c: a **string literal** has just completed, so the next token sits on it
    /// as a left operand ([`L2Position::StrOperator`]). Cleared on the same
    /// one-token schedule as [`awaiting_store_result`](Self::awaiting_store_result),
    /// and for the same reason spares the bare `-` of a split step arrow.
    awaiting_str_operator: bool,
    /// Every column name emitted so far — quoted string literals (arm-A N6,
    /// `~'Gross Credits'`) and arm-R `~`-introduced names (`~Col`, `~[Week, …]`
    /// keys). A superset stored as raw (unquoted) bytes, so a real reference to a
    /// previously-emitted name is never masked; the quoted `Column` narrower keys on
    /// `quote(c)` and the bare `RelationColumn` narrower on `c` itself.
    emitted_strings: Vec<Vec<u8>>,
    /// Every name the stream has bound as a variable — S2's legal `$var` set.
    ///
    /// **Monotonic and deliberately a superset**, exactly like
    /// [`emitted_strings`](Self::emitted_strings). Two reasons, both load-bearing:
    ///
    /// * *Soundness.* The scoped maps ([`var_class`](Self::var_class),
    ///   [`relation_row_vars`](Self::relation_row_vars),
    ///   [`var_reducer_type`](Self::var_reducer_type)) are restored on scope exit
    ///   and do not model every binder form the grammar admits — a join lambda's
    ///   second typed binder (`{row1: …[1], row2: …[1]|…}`) reaches no `on_pipe`
    ///   binding at all. Narrowing `$var` against them would mask real gold
    ///   queries. Recording a name wherever the tracker sees a *binder candidate*
    ///   and never retracting it keeps the set a superset of what is truly in
    ///   scope, so S2 only ever masks a name **nothing anywhere bound** — which is
    ///   the whole failure class it targets.
    /// * *Cache exactness.* A set that only grows is pinned by its length within a
    ///   stream, so [`L2Position::RefVar`] needs no name fingerprint in its cache
    ///   key. A shrinking set would alias distinct scopes under one count.
    ///
    /// Sacrificing precision here (a name bound in a sibling scope stays
    /// admissible) is the same trade `emitted_strings` documents: over-recording
    /// only lets more through, never masks.
    bound_vars: Vec<String>,
}

impl ScopeTracker {
    /// A fresh tracker at the start of a stream.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Advance the scope machine by one committed token, given its raw `bytes`, the
    /// pre-fold PDA configuration `pre` (state **and** stack), and the `schema`.
    ///
    /// Called from [`accept_token`](crate::DecoderSession::accept_token) as the
    /// token commits, so scope moves in lockstep with the automaton. A byte-level
    /// BPE token may straddle several lexeme boundaries (`'MaxRevenue')`, `.count`,
    /// `('`): the walk re-drives [`step`] read-only over the token, splitting it at
    /// each interior lexeme boundary and driving every constituent lexeme through
    /// the same per-lexeme logic a lexeme-granular stream uses (constitution §4,
    /// DRY). A run still open at the token's end (an identifier/string arriving in
    /// fragments) is buffered into [`Pending`] and resolved when a later token
    /// closes it (§6.4, B1); a run that closes inside the token is dispatched at
    /// once, so a buried `.`/`(` fires `on_dot`/`on_open` (H2) and a merged closing
    /// quote records the true column bytes (H1). The seed stack lets an interior
    /// closer (`)`) route through the matching frame rather than dying.
    pub(crate) fn observe(&mut self, bytes: &[u8], pre: &Pda, schema: &Schema) {
        let mut state = pre.state();
        let mut stack: Vec<Frame> = pre.stack().to_vec();
        // The first segment continues a lexeme buffered before this token only when
        // the pre-state sits inside that pending lexeme's own class.
        let mut continuing = self
            .pending
            .as_ref()
            .is_some_and(|p| state.lexeme_kind() == Some(p.kind));
        // The pre-state at the current segment's first byte — the anchor its scope
        // transition dispatches under (a continuation inherits the buffered anchor).
        let mut seg_anchor = if continuing {
            self.pending.as_ref().map_or(state, |p| p.anchor)
        } else {
            state
        };
        let mut seg_start = 0usize;
        // A pending that this token does not continue would be an unclosed lexeme L1
        // never admits; flush it defensively so no buffer leaks across tokens.
        if !continuing && let Some(done) = self.pending.take() {
            self.dispatch_token(&done.buf, done.anchor, schema);
        }

        for i in 0..bytes.len() {
            let before = state;
            let prev_kind = before.lexeme_kind();
            let top = stack.last().copied();
            state = match step(before, top, bytes[i]) {
                Step::Next(s) => s,
                Step::Push(frame, s) => {
                    stack.push(frame);
                    s
                }
                Step::Pop(s) => {
                    stack.pop();
                    s
                }
                // The token was pre-validated by L1's fold, so no byte dies here.
                Step::Dead => return,
            };
            let cur_kind = state.lexeme_kind();

            match prev_kind {
                // A lexeme closed via delegation at byte `i`: that byte is the
                // boundary that ended it (not part of it). Dispatch the lexeme
                // (prepending any cross-token buffer), then reopen a segment at `i`.
                Some(k) if cur_kind != Some(k) => {
                    self.emit_lexeme(&bytes[seg_start..i], seg_anchor, continuing, schema);
                    continuing = false;
                    seg_start = i;
                    seg_anchor = before;
                }
                // Still inside the same lexeme — keep accumulating.
                Some(_) => {}
                // In a structural gap; when a lexeme opens at byte `i`, flush the
                // gap that preceded it and start the lexeme segment here.
                None => {
                    if cur_kind.is_some() {
                        self.flush_gap(&bytes[seg_start..i], seg_anchor, schema);
                        seg_start = i;
                        seg_anchor = before;
                        continuing = false;
                    }
                }
            }
        }

        // The trailing segment: an open lexeme is buffered (resolved when a later
        // token closes it); a structural gap is dispatched whole.
        match state.lexeme_kind() {
            Some(kind) => self.buffer_trailing(kind, &bytes[seg_start..], seg_anchor, continuing),
            None => self.flush_gap(&bytes[seg_start..], seg_anchor, schema),
        }
    }

    /// Dispatch a closed lexeme through the per-token scope logic, prepending the
    /// cross-token [`Pending`] buffer when this run continues one. A `Str` lexeme
    /// arrives with both quotes, so [`classify`]/[`unquote`] records its byte-exact
    /// content into `emitted_strings` (H1).
    fn emit_lexeme(&mut self, seg: &[u8], anchor: State, continuing: bool, schema: &Schema) {
        if continuing && let Some(done) = self.pending.take() {
            let mut full = done.buf;
            full.extend_from_slice(seg);
            self.dispatch_token(&full, done.anchor, schema);
            return;
        }
        self.dispatch_token(seg, anchor, schema);
    }

    /// Split a structural gap (operators, punctuation, and — on the block-query
    /// `let` path — a bare keyword identifier) into its constituent tokens and
    /// dispatch each. Maximal munch mirrors [`classify`]'s granularity so a
    /// multi-byte operator (`->`, `==`) stays one token rather than fragmenting
    /// into mis-classified single bytes. Every gap token shares the gap's anchor
    /// pre-state; only `|` (`on_pipe`) and the `let`-path ident read it, and both
    /// classify identically from that anchor.
    fn flush_gap(&mut self, gap: &[u8], anchor: State, schema: &Schema) {
        let mut rest = gap;
        while let Some((&b, tail)) = rest.split_first() {
            if b.is_ascii_whitespace() {
                rest = tail;
            } else if let [x, y, ..] = rest
                && is_two_byte_op(*x, *y)
            {
                self.dispatch_token(&rest[..2], anchor, schema);
                rest = &rest[2..];
            } else if is_ident_start(b) {
                // `b` is an ident-start (⊆ ident-tail), so a correct scan takes at
                // least it; `.max(1)` makes that a hard floor, so the cursor advances
                // every iteration even if `is_ident_tail` misbehaves — the loop can
                // never spin on a zero-width token.
                let n = rest
                    .iter()
                    .take_while(|&&c| is_ident_tail(c))
                    .count()
                    .max(1);
                self.dispatch_token(&rest[..n], anchor, schema);
                rest = &rest[n..];
            } else {
                self.dispatch_token(&rest[..1], anchor, schema);
                rest = tail;
            }
        }
    }

    /// Buffer a lexeme still open at the token's end into [`Pending`], resolved and
    /// narrowed once a later token closes it (§6.4, B1). A continuation extends
    /// the existing buffer; a fresh run opens a new one, stamping the rule its
    /// anchor establishes (T1's `ReValue` lever, S1's `SourceMethodArg` and N3d's
    /// `StoreMethodArg` are
    /// both whole-token/first-byte shape tests with no prefix/trie walk, so their
    /// continuation sub-tokens pass through untouched once the anchor token's own
    /// shape has already been narrowed — e.g. a milestoning literal fragmented by
    /// BPE, `%late` + `st`, must not have its second fragment masked for not
    /// itself starting with `%`; likewise N3d's string argument, `'defa` + `ult',`,
    /// whose closing fragment carries the `,` that ends it).
    fn buffer_trailing(&mut self, kind: LexKind, seg: &[u8], anchor: State, continuing: bool) {
        if continuing {
            if let Some(pending) = self.pending.as_mut() {
                pending.buf.extend_from_slice(seg);
            }
            return;
        }
        let pos = match (kind, self.opening_position(anchor)) {
            // N7 outranks T1 for a bare word: `keeps_operand` narrows a
            // *literal*'s shape, which this lexeme can no longer become, while
            // what may follow a bare word still binds — live-rejected,
            // `->filter('SUM(SurfaceArea)'<agg/'…')` ("Can't find the
            // packageable element 'agg'").
            (LexKind::Ident, L2Position::ReValue(_)) => L2Position::ValueIdent,
            (
                _,
                L2Position::ReValue(_) | L2Position::SourceMethodArg | L2Position::StoreMethodArg,
            ) => L2Position::None,
            (_, narrowed) => narrowed,
        };
        self.pending = Some(Pending {
            kind,
            buf: seg.to_vec(),
            anchor,
            pos,
        });
    }

    /// Apply one whole lexeme's scope transition, given its raw `bytes` and the
    /// PDA `pre_state` it opened at. This is the per-token logic a lexeme-granular
    /// stream drives directly; the BPE path routes buffered lexemes through it too
    /// (constitution §4, DRY), so a fragmented and a whole identifier drive scope
    /// identically.
    fn dispatch_token(&mut self, bytes: &[u8], pre_state: State, schema: &Schema) {
        let lex = classify_at(bytes, pre_state);
        if lex == Lexeme::Ws {
            return;
        }
        // The operand of an armed comparison consumes the T1 arming (position()
        // has already been read for this token before it was accepted).
        let was_cmp = matches!(lex, Lexeme::Cmp);
        // T3's reducer-name arming lives exactly one token past the arrow that
        // sets it (see `on_arrow`); every other token clears it.
        let was_arrow = matches!(lex, Lexeme::Arrow);
        // N3e's arming is set *by* this token when it is the source method's own
        // closer, so the closer itself must not immediately consume it — nor may
        // the lone `-` of a step arrow the vocabulary splits.
        let was_close = matches!(lex, Lexeme::Close);
        let was_step_dash = bytes == STEP_DASH_TOKEN;
        // N4b's arming is set by this token when it is a logical operator, so —
        // exactly like the comparison arming above — the operator itself must not
        // consume it.
        //
        // Read through the same reassembly `classify_at` uses, not off `bytes`:
        // a vocabulary that splits `||` offers its second `|` as a token of its
        // own, and matching the raw bytes would miss the operator entirely and
        // leave N4b unarmed — the very split-token case N4a and N4c go to
        // explicit trouble over.
        let was_logical = LOGICAL_OPERATORS.contains(&operator_bytes(bytes, pre_state).as_slice());
        let mut resolved_now: Option<TypeClass> = None;

        match &lex {
            Lexeme::Ident(text) => self.on_ident(text, pre_state, schema, &mut resolved_now),
            Lexeme::Dot => self.on_dot(),
            Lexeme::Arrow => self.on_arrow(),
            Lexeme::Cmp => {
                if let Some(tc) = self.last_resolved {
                    self.cmp_pending = Some(tc);
                }
            }
            Lexeme::Pipe => self.on_pipe(pre_state),
            Lexeme::Open => self.on_open(pre_state),
            Lexeme::Close => self.on_close(),
            Lexeme::Comma => {
                self.lambda_first_ident = None;
                self.last_ident = None;
                self.pending_binder_element = None;
                if self.store_call_arity.is_some() {
                    self.store_call_commas += 1;
                }
            }
            Lexeme::Str(content) => {
                self.emitted_strings.push(content.clone());
                self.last_ident = None;
                self.awaiting_str_operator = true;
            }
            // A `$` sigil, number, date, or other structural byte is not an
            // identifier, so it clears the pending method name. A `$` needs no
            // further work: the refVar name that follows overwrites `pending_refvar`
            // unconditionally, and a fresh navigation reads the bound var (via
            // `on_dot`'s precedence) rather than any stale `nav_cursor`.
            Lexeme::Dollar | Lexeme::Number | Lexeme::Date | Lexeme::Other => {
                self.last_ident = None;
                self.logical_pending |= was_logical;
            }
            Lexeme::Ws => {}
        }

        // T1 arming lives exactly one non-whitespace token: it is set by a
        // primitive navExpr and read by the immediately following comparison.
        self.last_resolved = resolved_now;
        // The comparison operand (any non-cmp token after an armed comparison)
        // clears the arming.
        if !was_cmp {
            self.cmp_pending = None;
        }
        // The reducer-name, store-method and extent-method identifiers (any
        // non-arrow token after an armed arrow) consume their arrow's arming.
        if !was_arrow {
            self.awaiting_reducer = None;
            self.awaiting_store_method = false;
            self.awaiting_extent_method = false;
        }
        // N4b's operand (any non-operator token after an armed logical operator)
        // clears the arming, exactly as a comparison operand clears T1's.
        if !was_logical {
            self.logical_pending = false;
        }
        // N3e's and N4a's armings likewise live exactly one non-whitespace token
        // past the close that set them (a `Lexeme::Ws` returns before reaching
        // here, so `Class.all() ->step()` keeps the arming across its whitespace)
        // — plus the bare `-` of a split step arrow, whose `>` both armings still
        // have to require.
        if !was_close && !was_step_dash {
            self.awaiting_extent_step = false;
            self.awaiting_store_result = false;
        }
        // N4c's arming is set *by* the string literal, so — like the two above —
        // the literal itself must not consume it, nor may a split arrow's `-`.
        if !matches!(lex, Lexeme::Str(_)) && !was_step_dash {
            self.awaiting_str_operator = false;
        }
    }

    fn on_ident(
        &mut self,
        text: &str,
        pre_state: State,
        schema: &Schema,
        resolved_now: &mut Option<TypeClass>,
    ) {
        // A fully-qualified class path only appears as a pipeline source; binding
        // the pipeline element class here also handles nested subquery sources.
        if schema.has_class(text) {
            self.cur_class = Some(text.to_owned());
        }
        // This identifier closed at a source-triggering anchor and is itself a
        // real source path (never true for the `let` keyword, also legal there
        // but not a source path) — S1: the very next `.` (`on_dot`) must narrow
        // its identifier to exactly `SOURCE_METHOD`, not any class member.
        self.source_path_seen = (matches!(
            pre_state,
            State::ExpectSource | State::BlockStmt | State::BlockStmtClose
        ) && schema.source_paths().any(|path| path == text))
        .then(|| {
            if schema.has_class(text) {
                SourceKind::Class
            } else {
                SourceKind::Store
            }
        });
        match pre_state {
            // A refVar use (`$x`): never a lambda binder, never a member position.
            State::AfterDollar => {
                self.pending_refvar = Some(text.to_owned());
            }
            State::AfterDot => {
                self.resolve_member(text, schema, resolved_now);
            }
            // An identifier at a fresh value position is a lambda binder candidate
            // (`filter(x|…)`, `row: …|…`), recorded so the next binder pipe can
            // bind it. Property/method/refVar/source identifiers arrive in other
            // states and are never binders. A value position holds at most one such
            // identifier before its binder pipe (a body ident sits behind a `.`,
            // `->`, or `$`), so recording it unconditionally is exact — no
            // first-vs-last ambiguity to guard against.
            State::ExpectValue | State::ExpectValueReq => {
                self.lambda_first_ident = Some(text.to_owned());
                self.bind_var(text);
            }
            // Two binders the grammar fixes by *position*, with no pipe to confirm
            // them, so neither reaches `on_pipe`'s `lambda_first_ident` path:
            // `let <name> = …` (which additionally outlives its own statement —
            // every later statement in the block may reference it, as
            // `->in($topStates)` does), and a join brace lambda's leading typed
            // binder (`{row1: …[1], row2: …[1]|…}`, whose *second* binder is caught
            // by the `ExpectValueReq` arm after the comma).
            State::ExpectBinder | State::ExpectBraceBinder => {
                self.bind_var(text);
            }
            // The arm-R map lambda binds its variable *after* a colon
            // (`~[Col: x|…]`, `~'Name': x|…`), which the byte-PDA parks in an
            // `InIdent` reached from `AfterColon`/`AfterColonWs`, not a value state.
            // Recording it rebinds the binder at the following `|` — without this a
            // re-used name keeps whatever class an earlier `filter(x|…)` lambda bound
            // it to, and N1 unsoundly masks a projected column. A *class-named*
            // identifier here is instead the type of a typed binder
            // (`row: Person[1]|…`), whose true binder precedes the colon; leaving the
            // pre-colon candidate in place keeps that narrowing intact. Likewise a
            // *primitive*-named identifier (`y: Integer[*]|…`, T3's aggregation-reduce
            // binder) is a type annotation, not the binder. Its multiplicity brackets
            // (`[*]`) still lie ahead, and `on_open` unconditionally clears
            // `lambda_first_ident` on every opening delimiter (a defensive reset for a
            // *fresh* argument list, which a multiplicity annotation is not) — so the
            // binder name is read and stashed together with the `TypeClass` *now*,
            // rather than trusted to survive in `lambda_first_ident` until `on_pipe`.
            State::AfterColon | State::AfterColonWs => {
                if let Some(prim) = PrimName::from_ident(text) {
                    if let Some(binder) = &self.lambda_first_ident {
                        self.pending_binder_element = Some((binder.clone(), prim.type_class()));
                    }
                } else if !schema.has_class(text) {
                    self.lambda_first_ident = Some(text.to_owned());
                    self.bind_var(text);
                }
            }
            _ => {}
        }
        // Record an arm-R column *name* into the emitted-column universe: a bare
        // `~Col` (anchored at `SawTilde`), or a `~[…]` key — an identifier at a value
        // position directly inside the innermost tilde bracket (`~[Week, Segment]`,
        // and the name before the `:` in `~[Week: …]`). The first key opens at
        // `ExpectValue`, a key after a comma at `ExpectValueReq`; both count. Body
        // identifiers sit behind a `$`/`.`/`|`, never at a bracket-level value anchor,
        // so they are not collected. Over-recording only lets more through, never
        // masks, so the set stays a superset of the columns live on any relation row.
        if pre_state == State::SawTilde || self.in_tilde_key(pre_state) {
            self.emitted_strings.push(text.as_bytes().to_vec());
        }
        self.last_ident = Some(text.to_owned());
        self.last_ident_after_arrow = pre_state == State::AfterArrow;
    }

    fn resolve_member(
        &mut self,
        ident: &str,
        schema: &Schema,
        resolved_now: &mut Option<TypeClass>,
    ) {
        if std::mem::take(&mut self.awaiting_source_method) {
            // The identifier closing this dot was narrowed to `SOURCE_METHOD`
            // (S1); `all` is not a class member, so there is nothing to resolve.
            return;
        }
        if self.dot_is_column {
            // A bare-ident column access (`$row.Col`) terminates navigation: a
            // column is a value, not a class, so no member resolves and a following
            // `.` degrades to pass-through.
            self.dot_is_column = false;
            self.nav_cursor = None;
            self.last_nav_class = None;
            return;
        }
        let Some(base) = self.dot_base.take() else {
            // A dot that is not a member navigation (`.all()`, `.getX`, `$r.` over
            // an unknown binder): no resolution, no cursor change.
            return;
        };
        match schema.resolve(&base, ident) {
            Some(Resolved::Class { path, .. }) => {
                self.nav_cursor = Some(path.clone());
                self.last_nav_class = Some(path);
            }
            Some(Resolved::Primitive { prim, .. }) => {
                *resolved_now = Some(prim.type_class());
                self.nav_cursor = None;
                self.last_nav_class = None;
            }
            Some(Resolved::Enum { .. }) | None => {
                self.nav_cursor = None;
                self.last_nav_class = None;
            }
        }
    }

    /// A dot right after a just-dispatched source classpath (`source_path_seen`)
    /// arms [`awaiting_source_method`](Self::awaiting_source_method) so the
    /// following identifier is narrowed to [`SOURCE_METHOD`] (S1) rather than
    /// falling through the ordinary `dot_base`/member logic below, which would
    /// otherwise stay `None` here regardless (a fresh source classpath sets
    /// neither `pending_refvar` nor `nav_cursor`) and leave this dot fully
    /// unnarrowed — the gap `L2Position::SourceMethod` exists to close.
    fn on_dot(&mut self) {
        self.dot_is_column = false;
        self.awaiting_source_method = self.source_path_seen.take().is_some();
        if let Some(var) = self.pending_refvar.take() {
            if self.relation_row_vars.contains(&var) {
                // `$row.` over an arm-R relation-row binder: the next identifier is a
                // bare-ident column reference, narrowed against the emitted-column
                // universe rather than a class's members.
                self.dot_is_column = true;
                self.dot_base = None;
            } else {
                self.dot_base = self.var_class.get(&var).cloned().flatten();
            }
        } else if let Some(cursor) = &self.nav_cursor {
            self.dot_base = Some(cursor.clone());
        } else if self.awaiting_extent_step {
            // A `.` straight off `Class.all()` navigates the extent's own class —
            // the one nav position with no `$var` and no prior cursor to read it
            // from, and so the one N1 used to leave wholly unnarrowed (live:
            // `{|…::Country.all().sort …}` → "Can't find property 'sort' in class
            // …::Country").
            self.dot_base = self.cur_class.clone();
        } else {
            self.dot_base = None;
        }
    }

    fn on_arrow(&mut self) {
        // N3c (store arm): an arrow straight off a store source path opens the
        // one position where a store method name is legal.
        self.awaiting_store_method = self.source_path_seen.take() == Some(SourceKind::Store);
        // N3f: the mirror arm. N3e's arming is still live at this point —
        // `dispatch_token` clears it only after this call returns — so an arrow
        // that N3e itself admitted off a closed `Class.all()` is exactly the one
        // that opens the extent's method-name position.
        self.awaiting_extent_method = self.awaiting_extent_step;
        // T3: a bare `$y->` (no intervening dot — `pending_refvar` still names the
        // just-dispatched refVar) where `y` is bound to a primitive element type
        // arms the reducer-name position (`opening_position` reads this one token
        // later, at the identifier that follows).
        if let Some(var) = &self.pending_refvar
            && let Some(tc) = self.var_reducer_type.get(var)
        {
            self.awaiting_reducer = Some(*tc);
        }
        // The arrow ends the current navExpr; capture the receiver for a possible
        // following method lambda, then reset the nav cursor.
        let receiver = self
            .last_nav_class
            .take()
            .or_else(|| self.cur_class.clone());
        self.pending_arrow_receiver = Some(receiver);
        self.pending_refvar = None;
        self.nav_cursor = None;
        self.last_ident = None;
    }

    fn on_pipe(&mut self, pre_state: State) {
        // The query-opening `|` at Start is not a binder.
        if matches!(pre_state, State::Start | State::ExpectSource) {
            return;
        }
        // Snapshot the enclosing pipeline's class and arm/relation state as the lambda
        // body begins — before a nested pipeline in the body (`Class.all()` or a
        // navigation-headed subquery) can change them — so the body's delimiter close
        // restores them and the nested pipeline's state cannot leak out.
        self.scope_saves.push(ScopeSave {
            depth: self.depth,
            prev_cur_class: self.cur_class.clone(),
            prev_rel_explicit: self.rel_explicit,
            prev_saw_tilde_bracket: self.saw_tilde_bracket,
        });
        if let Some(name) = self.lambda_first_ident.take()
            && !name.is_empty()
        {
            // Save the binding this lambda shadows, so the enclosing delimiter's close
            // restores it and the binder cannot leak into an outer scope reusing the
            // name.
            self.binder_saves.push(BinderSave {
                depth: self.depth,
                name: name.clone(),
                prev_class: self.var_class.get(&name).cloned(),
                prev_relation_row: self.relation_row_vars.contains(&name),
                prev_reducer_type: self.var_reducer_type.get(&name).copied(),
            });
            let receiver = self.paren_receiver.last().cloned().flatten();
            if receiver.is_none() && self.rel_explicit && self.saw_tilde_bracket {
                // An arm-R relation-row binder (a map lambda over a closed
                // establishing op's tilde relation): `$name.<Col>` is a bare-ident
                // column access. Track it apart from the class map so on_dot narrows
                // to the column universe instead of degrading to pass-through.
                self.var_class.remove(&name);
                self.relation_row_vars.insert(name.clone());
            } else {
                self.relation_row_vars.remove(&name);
                self.var_class.insert(name.clone(), receiver);
            }
            match self.pending_binder_element.take() {
                Some((_, tc)) => {
                    self.var_reducer_type.insert(name, tc);
                }
                None => {
                    self.var_reducer_type.remove(&name);
                }
            }
        } else if let Some((name, tc)) = self.pending_binder_element.take() {
            // T3: a primitive-typed binder's own multiplicity brackets
            // (`Integer[*]` in `y: Integer[*]|…`) reach `on_open`, which
            // unconditionally clears `lambda_first_ident` on every opening
            // delimiter (a defensive reset for a *fresh* argument list, which a
            // multiplicity annotation is not) — so by the time this pipe is
            // reached, `lambda_first_ident` is already gone. The (name,
            // `TypeClass`) pair `on_ident` stashed at the type identifier itself
            // survives that clear; bind it here instead.
            self.binder_saves.push(BinderSave {
                depth: self.depth,
                name: name.clone(),
                prev_class: None,
                prev_relation_row: false,
                prev_reducer_type: self.var_reducer_type.get(&name).copied(),
            });
            self.var_reducer_type.insert(name, tc);
        }
    }

    fn on_open(&mut self, pre_state: State) {
        let method = self.last_ident.take();
        self.depth += 1;
        // N3's grammar production constrains `all()`'s argument slot regardless
        // of nesting depth — unlike the nested-pipeline reset below, this is not
        // depth-gated.
        self.in_source_method_args = method.as_deref() == Some(SOURCE_METHOD);
        // N3d: a store method's own call owes exactly its declared string
        // arguments, so arm the argument/separator positions for its whole extent.
        self.store_call_arity = method.as_deref().and_then(store_method_arity);
        self.store_call_commas = 0;
        // N3g: an arrow call of a receiver-only builtin has already spent the one
        // parameter its whole overload set declares, so the slot this `(` opens
        // owes nothing and admits only its closer.
        self.receiver_only_call = self.last_ident_after_arrow
            && method
                .as_deref()
                .is_some_and(|name| RECEIVER_ONLY_METHODS.contains(&name));
        // A `~[` opens a relation column set: latch the pipeline as arm-R, so an
        // `ExpectValue` key identifier inside it is a column name and a following
        // relation-row binder narrows column access. The flag is pushed for *every*
        // open (in lockstep with `paren_receiver`) so nesting pops it cleanly.
        let is_tilde_bracket = pre_state == State::SawTilde;
        self.saw_tilde_bracket |= is_tilde_bracket;
        self.tilde_open.push(is_tilde_bracket);
        if let Some(name) = &method {
            if ESTABLISHING_METHODS.contains(&name.as_str()) {
                self.est_stack.push(self.depth);
            }
            if REF_METHODS.contains(&name.as_str()) {
                self.ref_stack.push(self.depth);
            }
            // A nested `all()` heads a fresh class-extent pipeline inside a lambda
            // body: reset the arm state so the subquery's own establishing ops
            // classify its binders against a clean baseline. The enclosing body's
            // `ScopeSave` (taken at its binder pipe) restores the outer state on close,
            // so this reset cannot leak back out.
            if name.as_str() == SOURCE_METHOD && self.depth >= NESTED_SOURCE_MIN_DEPTH {
                self.rel_explicit = false;
                self.saw_tilde_bracket = false;
            }
        }
        let receiver = self
            .pending_arrow_receiver
            .take()
            .unwrap_or_else(|| self.cur_class.clone());
        self.paren_receiver.push(receiver);
        self.lambda_first_ident = None;
    }

    fn on_close(&mut self) {
        // Consume the source-method-args flag at its own call's close — it must
        // never survive to wrongly mask a later value position reached without
        // an intervening `on_open` (see `in_source_method_args`'s doc comment).
        // N3e reads the same fact one token further on: the call that just closed
        // was the source method's, so what follows sits on the class extent.
        self.awaiting_extent_step = self.in_source_method_args;
        self.in_source_method_args = false;
        // N4a reads the store call's close the way N3e reads the source method's:
        // the call that just closed was the store method's, so what follows sits
        // on its `Table[1]` result. Assigned rather than or-ed, so an enclosing
        // `)` — which never has an arity — clears it instead of carrying it out.
        self.awaiting_store_result = self.store_call_arity.is_some();
        self.store_call_arity = None;
        self.receiver_only_call = false;
        // Restore every binder introduced at the closing delimiter's depth to what it
        // shadowed, so a lambda's binder never outlives its scope (§6.4). Deeper
        // scopes have already restored and popped, so the depth-matching saves are
        // contiguous at the top of the stack.
        while self
            .binder_saves
            .last()
            .is_some_and(|save| save.depth == self.depth)
        {
            let Some(save) = self.binder_saves.pop() else {
                break;
            };
            match save.prev_class {
                Some(class) => {
                    self.var_class.insert(save.name.clone(), class);
                }
                None => {
                    self.var_class.remove(&save.name);
                }
            }
            if save.prev_relation_row {
                self.relation_row_vars.insert(save.name.clone());
            } else {
                self.relation_row_vars.remove(&save.name);
            }
            match save.prev_reducer_type {
                Some(tc) => {
                    self.var_reducer_type.insert(save.name, tc);
                }
                None => {
                    self.var_reducer_type.remove(&save.name);
                }
            }
        }
        // Restore the enclosing pipeline's class + arm/relation state for lambda
        // bodies closing here, so a nested subquery (an `all()` or navigation-headed
        // pipeline in the body) cannot leak its class or arm-R state past its scope.
        // This runs *before* the establishing-op block below, which then re-clears
        // `cur_class` to `None` when this delimiter is a `project`/`groupBy`
        // (relation → TDS row).
        while self
            .scope_saves
            .last()
            .is_some_and(|save| save.depth == self.depth)
        {
            let Some(save) = self.scope_saves.pop() else {
                break;
            };
            self.cur_class = save.prev_cur_class;
            self.rel_explicit = save.prev_rel_explicit;
            self.saw_tilde_bracket = save.prev_saw_tilde_bracket;
        }
        self.tilde_open.pop();
        if self.ref_stack.last() == Some(&self.depth) {
            self.ref_stack.pop();
        }
        if self.est_stack.last() == Some(&self.depth) {
            self.est_stack.pop();
            // A named relation now exists downstream (§6.4.5/6.4.6): the pipeline
            // element is a TDS row, not a class instance, so a following lambda
            // binder must NOT bind to the (pre-group) source class. Clearing
            // `cur_class` makes such binders unknown → N1 pass-through, never a
            // mask of a TDS-row getter.
            self.rel_explicit = true;
            self.cur_class = None;
        }
        self.paren_receiver.pop();
        self.depth = self.depth.saturating_sub(1);
        self.pending_arrow_receiver = None;
    }

    /// Whether `state` is a value anchor sitting **directly inside** an arm-R
    /// tilde bracket (`~[Week, Segment]`, and the name before the `:` in
    /// `~[Week: …]`) — the position an arm-R column *key* opens at. The first key
    /// opens at `ExpectValue`, a key after a comma at `ExpectValueReq`; both
    /// count. Body identifiers sit behind a `$`/`.`/`|`, never at a bracket-level
    /// value anchor, so they are not keys.
    fn in_tilde_key(&self, state: State) -> bool {
        matches!(state, State::ExpectValue | State::ExpectValueReq)
            && self.tilde_open.last() == Some(&true)
    }

    /// Whether we are inside a column-reference method's arguments *and* a named
    /// relation exists and we are not inside an establishing op's own arguments —
    /// the exact condition for an N6 [`Column`](L2Position::Column) narrowing.
    fn in_column_arg(&self) -> bool {
        !self.ref_stack.is_empty() && self.rel_explicit && self.est_stack.is_empty()
    }

    /// N3d's separator position, when `state` is one of the two the open
    /// store-method call decides: right after a completed argument
    /// ([`State::AfterValue`]/[`State::AfterName`]), or on an argument literal's
    /// pending closing quote
    /// ([`State::InStrLit`] with a quote awaiting its disambiguating byte).
    ///
    /// One derivation for both, so the arity fact — how many arguments
    /// [`STORE_METHODS`] declares for the open call, against how many commas it
    /// has emitted — is stated exactly once.
    fn store_method_arg_sep(&self, state: State) -> Option<L2Position> {
        let decided = matches!(
            state,
            State::AfterValue | State::AfterName | State::InStrLit { escaped: true }
        );
        let arity = self.store_call_arity?;
        // A `,` only ever follows a completed argument, so the call has completed
        // one more argument than it has emitted commas.
        decided.then(|| L2Position::StoreMethodArgSep {
            remaining: self.store_call_commas + 1 < arity,
        })
    }

    /// The L2 constraint at the current PDA `state`.
    ///
    /// At an **anchor** state (an inter-lexeme position) the rule is read from the
    /// automaton state and the typed scope. At an **in-lexeme** state (mid
    /// identifier/string, where a BPE sub-token lands) it is the rule the open
    /// accumulation carries — so the trie narrows the continuation sub-tokens, not
    /// only the leading one (B1). An in-lexeme state with no open accumulation, or
    /// an accumulation the anchor did not narrow, is [`None`](L2Position::None).
    pub(crate) fn position(&self, state: State) -> L2Position {
        // N3d, the one in-lexeme state that is nonetheless a decided position: a
        // store-method argument literal sitting on a *pending* closing quote is
        // already a complete argument. The next byte either doubles the quote
        // (`'O''Brien'`, which the separator set admits for exactly this reason)
        // or is the `,`/`)` the call owes — never an operator. Without this the
        // arity half of the rule would be unreachable, because the token that
        // closes the literal is read at this state, not at `AfterValue`.
        if let Some(sep) = self.store_method_arg_sep(state) {
            return sep;
        }
        let pos = if state.lexeme_kind().is_some() {
            match &self.pending {
                Some(pending) => pending.pos.clone(),
                None => L2Position::None,
            }
        } else {
            self.opening_position(state)
        };
        // N7 has no anchor and no hold on a keyword literal, so report it only
        // where it actually narrows — a coverage consumer then sees a rule
        // firing rather than a position merely existing, and the narrower needs
        // no second copy of the test.
        let pos = if matches!(pos, L2Position::ValueIdent) && !self.value_ident_narrows() {
            L2Position::None
        } else {
            pos
        };
        // N4c, at the same in-lexeme state N3d needs above and for the same
        // reason: a string literal sitting on its *pending* closing quote is
        // already a complete operand, and the token that decides what follows it
        // is read here rather than at `AfterValue` — the literal itself is only
        // dispatched once a later token closes it, so the arming
        // (`awaiting_str_operator`) is still one token away.
        //
        // Applied only where no other rule governs the literal, which is what
        // keeps it out of N6's and T1's way: a quoted column name and a typed
        // comparison operand carry their own stamped position through this state,
        // and this rule never displaces one — N7's own stamp is the exception,
        // since it does not narrow a *string* lexeme at all and has already been
        // folded to `None` above. Nothing is lost by the deferral: once
        // whitespace or any other token closes the literal, the arming takes over
        // at `AfterValue`.
        if matches!(pos, L2Position::None) && Self::on_pending_str_quote(state) {
            return L2Position::StrOperator { after_dash: false };
        }
        pos
    }

    /// Whether `state` is a completed string literal awaiting the byte that
    /// disambiguates its closing quote ([`State::InStrLit`] with `escaped`) — the
    /// point at which N4c's operand is complete but its lexeme is not yet
    /// dispatched.
    fn on_pending_str_quote(state: State) -> bool {
        matches!(state, State::InStrLit { escaped: true })
    }

    /// Whether N7 narrows at the current point: a bare **identifier** lexeme is
    /// open and it is not one of the keyword literals.
    ///
    /// The lexeme-kind test is load-bearing. A string, number, or date literal
    /// opens at the very same value anchor and so carries the very same stamped
    /// position, but it is not a bare word: under a lexeme-granular vocabulary it
    /// arrives as one atomic token and the distinction never shows, while
    /// byte-level BPE fragments it and the rule would mask its own continuation
    /// bytes (`'default'` → `'def` + `ault'`).
    fn value_ident_narrows(&self) -> bool {
        matches!(&self.pending, Some(pending) if pending.kind == LexKind::Ident)
            && value_ident_constrains(self.narrow_prefix())
    }

    /// The L2 rule at the anchor `state` where a lexeme opens — read from the
    /// automaton state and the typed scope. Shared by [`position`] (for anchor
    /// states) and by `observe` (to stamp an opening accumulation's rule).
    fn opening_position(&self, state: State) -> L2Position {
        match state {
            State::ExpectSource | State::BlockStmt | State::BlockStmtClose => {
                L2Position::SourceIdent
            }
            // S2: a `$` sigil's identifier names a variable, and the only names in
            // the graph are the ones this stream bound.
            State::AfterDollar => L2Position::RefVar,
            State::AfterDot => {
                if self.awaiting_source_method {
                    L2Position::SourceMethod
                } else if self.dot_is_column {
                    L2Position::RelationColumn
                } else {
                    match &self.dot_base {
                        Some(base) => L2Position::Member(base.clone()),
                        None => L2Position::None,
                    }
                }
            }
            State::ExpectValue | State::ExpectValueReq => self.value_opening_position(state),
            // A bare word also opens a value straight after a lambda arrow (the
            // body) or after an operator that has its own intermediate state
            // because it may still grow into a longer one (`<` `>` `-` `|`) —
            // the same N7 position, reached without ever passing through
            // `ExpectValue`. Live-attested on both routes:
            // `->col(between|true!='Brazil')` and
            // `->filter('SUM(SurfaceArea)'<agg/'…')`.
            // N3e's second half outranks N7 here: the `-` it is holding open is a
            // step arrow under construction, not a word in operand position.
            State::SawDash if self.awaiting_extent_step => {
                L2Position::SourceExtent { after_dash: true }
            }
            // N4a's and N4c's second halves, for the same reason and by the same
            // mechanism.
            State::SawDash if self.awaiting_store_result => {
                L2Position::StoreResult { after_dash: true }
            }
            State::SawDash if self.awaiting_str_operator => {
                L2Position::StrOperator { after_dash: true }
            }
            State::SawPipe | State::SawLt | State::SawGt | State::SawDash => L2Position::ValueIdent,
            // T2's comparator lever reads a *completed* term, whichever of the
            // two terminal hubs it landed in — a navExpr ends on its property
            // name, so `$x.population > 5` reaches the comparator through
            // `AfterName`, not `AfterValue`.
            State::AfterValue if self.awaiting_extent_step => {
                L2Position::SourceExtent { after_dash: false }
            }
            // N4a. Mutually exclusive with N3e above — one arming is set by the
            // source method's close, the other by the store method's — and it
            // outranks T2's comparator lever, which reads a *primitive* navExpr
            // and so is never armed on a `Table[1]` anyway.
            State::AfterValue | State::AfterName if self.awaiting_store_result => {
                L2Position::StoreResult { after_dash: false }
            }
            // N4c. Mutually exclusive with N4a above: a store call's `)` clears
            // the string arming its last argument set, so only one can hold.
            State::AfterValue | State::AfterName if self.awaiting_str_operator => {
                L2Position::StrOperator { after_dash: false }
            }
            State::AfterValue | State::AfterName => match self.last_resolved {
                Some(tc) => L2Position::Comparator(tc),
                None => L2Position::None,
            },
            // A method-name identifier right after `->` opens here (distinct from
            // `ExpectValue`/`ExpectValueReq`, which a value/lambda term opens at).
            State::AfterArrow if self.awaiting_store_method => L2Position::StoreMethod,
            // N3f. Mutually exclusive with the store arm above: a stream reaches
            // one only off a store path and the other only off a class extent.
            State::AfterArrow if self.awaiting_extent_method => L2Position::ExtentMethod,
            State::AfterArrow => match self.awaiting_reducer {
                Some(tc) => L2Position::Reducer(tc),
                None => L2Position::None,
            },
            _ => L2Position::None,
        }
    }

    /// The L2 rule at a **value** anchor (`ExpectValue`/`ExpectValueReq`) — the one
    /// arm of [`opening_position`](ScopeTracker::opening_position) that is itself a
    /// cascade rather than a state test, since every rule that governs an argument
    /// or operand slot competes for the same two states.
    ///
    /// Split out so the two stay separately readable: the caller is a table of
    /// automaton states, this is a precedence order over the open call and
    /// comparison context.
    fn value_opening_position(&self, state: State) -> L2Position {
        if self.in_source_method_args {
            L2Position::SourceMethodArg
        } else if self.store_call_arity.is_some() {
            L2Position::StoreMethodArg
        } else if self.receiver_only_call {
            L2Position::ReceiverOnlyArg
        } else if self.logical_pending {
            L2Position::LogicalOperand
        } else if let Some(tc) = self.cmp_pending {
            L2Position::ReValue(tc)
        } else if self.in_column_arg() {
            L2Position::Column
        } else if self.in_tilde_key(state) {
            // An arm-R `~[Col, …]` key is a bare word that *is* a complete value,
            // so N7's "a dangling word resolves to nothing" premise does not hold
            // for it. None of the 8 fixture corpora use arm-R at all
            // (`schema_walk_rule_coverage.rs`'s `EXPECTED_UNFIRED`), so there is
            // no evidence here for what may follow one — and §4's rule is to
            // invent no constraint the corpus does not exercise.
            L2Position::None
        } else {
            L2Position::ValueIdent
        }
    }

    /// The **post-dot** L2 target a fused nav-dot token (`.<member>` / `.<col>` in
    /// one BPE token) should be narrowed against here, or [`None`] where a following
    /// `.` would open no class-member / relation-column navigation.
    ///
    /// The ordinary [`position`](ScopeTracker::position) is read at the anchor
    /// *before* the dot, so it cannot narrow an identifier that rides in behind a
    /// fused `.`; the session applies this as a second, subtractive narrow
    /// ([`narrow_fused_into`](crate::schema::narrow::narrow_fused_into)). This mirrors
    /// [`on_dot`](ScopeTracker::on_dot) read-only, resolving the identifier a dot
    /// would close from the still-open pending buffer (mid-BPE) or a just-dispatched
    /// refvar. Where a `.` is not in fact L1-legal the affected mask bits are already
    /// clear, so an over-permissive target is a no-op, never unsound. Returns only
    /// [`Member`](L2Position::Member) or [`RelationColumn`](L2Position::RelationColumn).
    pub(crate) fn fused_nav_position(&self, schema: &Schema) -> Option<L2Position> {
        // (a) An identifier still open at the token boundary — a `.` closes it, then
        //     navigates from what it resolves to. Only `$var` (a refvar, anchored at
        //     `AfterDollar`) and a member step (anchored at `AfterDot`) navigate; a
        //     source classpath, method name, or open string does not.
        if let Some(pending) = &self.pending {
            if pending.kind != LexKind::Ident {
                return None;
            }
            let ident = std::str::from_utf8(&pending.buf).ok()?;
            return match pending.anchor {
                State::AfterDollar => self.nav_from_var(ident),
                State::AfterDot => self.nav_from_member(ident, schema),
                _ => None,
            };
        }
        // (b) No open identifier: a refvar already dispatched and awaiting its dot
        //     (`$c .foo`), or a completed navigation resting at a class.
        if let Some(var) = &self.pending_refvar {
            return self.nav_from_var(var);
        }
        self.nav_cursor.clone().map(L2Position::Member)
    }

    /// The fused-nav target of a `.` closing the refvar `var` — the emitted-column
    /// universe for an arm-R relation row, else the class `var` is bound to (nav
    /// terminates, so [`None`], when `var` is unbound, e.g. a TDS row binder).
    fn nav_from_var(&self, var: &str) -> Option<L2Position> {
        if self.relation_row_vars.contains(var) {
            return Some(L2Position::RelationColumn);
        }
        self.var_class
            .get(var)
            .cloned()
            .flatten()
            .map(L2Position::Member)
    }

    /// The fused-nav target of a `.` closing the member step `member` — the class it
    /// resolves to from the current navigation base, or [`None`] when it resolves to
    /// a primitive/enum (nav terminates) or has no base to resolve against.
    fn nav_from_member(&self, member: &str, schema: &Schema) -> Option<L2Position> {
        let base = self.dot_base.as_ref()?;
        match schema.resolve(base, member) {
            Some(Resolved::Class { path, .. }) => Some(L2Position::Member(path)),
            _ => None,
        }
    }

    /// The identifier/string bytes emitted since the current lexeme's anchor — the
    /// trie-walk prefix the narrower reads. Empty at an anchor (no open
    /// accumulation) so the walk starts at the trie root.
    pub(crate) fn narrow_prefix(&self) -> &[u8] {
        match &self.pending {
            Some(pending) => &pending.buf,
            None => &[],
        }
    }

    /// The N6 legal column set: every string literal emitted so far.
    pub(crate) fn emitted_columns(&self) -> &[Vec<u8>] {
        &self.emitted_strings
    }

    /// The S2 legal `$var` set: every name the stream has bound (see
    /// [`bound_vars`](Self::bound_vars)).
    pub(crate) fn bound_variables(&self) -> &[String] {
        &self.bound_vars
    }

    /// Record `name` as a bound variable. Deduplicated so the length stays a
    /// faithful identity for [`L2Position::RefVar`]'s cache key — a re-recorded
    /// name must not churn the memo for a set that did not change.
    fn bind_var(&mut self, name: &str) {
        if !self.bound_vars.iter().any(|bound| bound == name) {
            self.bound_vars.push(name.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        L2Position, Lexeme, SOURCE_METHOD, ScopeTracker, classify, classify_at, is_two_byte_op,
    };
    use crate::grammar::pda::{Pda, State};
    use crate::schema::model::{Schema, TypeClass};

    /// A two-byte operator split across two tokens is classified as the operator
    /// it is, not as its second byte alone — the fix for the leak that let a
    /// store path be arrowed into an arbitrary method past N3c (a vocabulary
    /// offering `-` and `>` separately meant no token's bytes were ever `->`, so
    /// `on_arrow` never fired). Every listed anchor changes the verdict; the
    /// contrast rows below pin that it changes nothing else.
    #[test]
    fn a_two_byte_operator_split_across_tokens_classifies_as_the_whole_operator() {
        for (anchor, second, want) in [
            (State::SawDash, b'>', Lexeme::Arrow),
            (State::SourceDash, b'>', Lexeme::Arrow),
            (State::SawEq, b'=', Lexeme::Cmp),
            (State::SawBang, b'=', Lexeme::Cmp),
            (State::SawGt, b'=', Lexeme::Cmp),
            (State::SawLt, b'=', Lexeme::Cmp),
            // `||` is boolean-or, deliberately *not* the lambda binder `Pipe` a
            // lone `|` classifies as.
            (State::SawPipe, b'|', Lexeme::Other),
        ] {
            assert_eq!(
                classify_at(&[second], anchor),
                want,
                "{:?} + {:?} completes a two-byte operator",
                anchor.name(),
                char::from(second)
            );
        }
        // The same second bytes read at an anchor holding no operator half
        // classify on their own bytes.
        assert_eq!(classify_at(b">", State::AfterValue), Lexeme::Cmp);
        assert_eq!(classify_at(b"|", State::AfterValue), Lexeme::Pipe);
        assert_eq!(classify_at(b"=", State::AfterValue), Lexeme::Other);
        // A byte that does not complete the operator is still its own token —
        // the whitespace of `weight < 3500` must stay `Ws` (a scope no-op), not
        // become a state-clearing `Other`.
        assert_eq!(classify_at(b" ", State::SawLt), Lexeme::Ws);
        assert_eq!(classify_at(b"5", State::SawDash), Lexeme::Number);
        // Only a single-byte token can be an operator's second half.
        assert_eq!(classify_at(b">take", State::SawDash), Lexeme::Other);
    }

    const SAMPLE: &str = r#"{
      "db_id": "d", "db_path": "spider::d::Db",
      "classes": {
        "A": { "simple_name": "A", "properties": [
          {"name": "n", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}},
          {"name": "s", "type": {"kind": "primitive", "name": "String"}, "mult": {"lower": 0, "upper": 1}}
        ] },
        "B": { "simple_name": "B", "properties": [
          {"name": "m", "type": {"kind": "primitive", "name": "Integer"}, "mult": {"lower": 1, "upper": 1}}
        ] } },
      "associations": [], "enums": {}
    }"#;

    fn schema() -> Schema {
        Schema::from_json(SAMPLE).expect("parses")
    }

    /// Drive `tokens` through a fresh PDA + tracker exactly as the session does
    /// (pre-state captured before folding), returning both so a test can read the
    /// position at the live automaton state.
    fn run(tokens: &[&[u8]]) -> (ScopeTracker, Pda) {
        let schema = schema();
        let mut pda = Pda::new();
        let mut tracker = ScopeTracker::new();
        for token in tokens {
            let pre = pda.clone();
            for &byte in *token {
                pda.advance(byte)
                    .expect("test tokens are valid emitted Pure");
            }
            tracker.observe(token, &pre, &schema);
        }
        (tracker, pda)
    }

    #[test]
    fn classify_distinguishes_structural_lexemes() {
        assert_eq!(classify(b""), Lexeme::Ws);
        assert_eq!(classify(b"  \n"), Lexeme::Ws);
        assert_eq!(classify(b"->"), Lexeme::Arrow);
        assert_eq!(classify(b"=="), Lexeme::Cmp);
        assert_eq!(classify(b">"), Lexeme::Cmp);
        assert_eq!(classify(b"."), Lexeme::Dot);
        assert_eq!(classify(b"$"), Lexeme::Dollar);
        assert_eq!(classify(b"|"), Lexeme::Pipe);
        assert_eq!(classify(b","), Lexeme::Comma);
        assert_eq!(classify(b"("), Lexeme::Open);
        assert_eq!(classify(b"]"), Lexeme::Close);
    }

    #[test]
    fn classify_distinguishes_value_lexemes() {
        assert_eq!(classify(b"42"), Lexeme::Number);
        assert_eq!(classify(b"-7"), Lexeme::Number);
        assert_eq!(classify(b"%2018-01-01"), Lexeme::Date);
        assert_eq!(
            classify(b"spider::d::A"),
            Lexeme::Ident("spider::d::A".to_owned())
        );
        assert_eq!(classify(b"+"), Lexeme::Other);
        assert_eq!(classify(b"-"), Lexeme::Other);
    }

    #[test]
    fn two_byte_op_matches_the_operators_and_nothing_else() {
        // The seven two-byte operators a structural gap munches whole.
        for op in [b"->", b"==", b"!=", b"<=", b">=", b"&&", b"||"] {
            assert!(
                is_two_byte_op(op[0], op[1]),
                "{op:?} is a two-byte operator"
            );
        }
        // Adjacent non-operator pairs must NOT munch as one token — otherwise a
        // gap like `.(` or `))` fragments wrongly and its bytes mis-classify.
        for pair in [
            b"><", b">>", b"<<", b"--", b"=>", b"=<", b").", b".(", b"))", b"|&", b"&|",
        ] {
            assert!(
                !is_two_byte_op(pair[0], pair[1]),
                "{pair:?} is not a two-byte operator"
            );
        }
    }

    #[test]
    fn classify_unquotes_and_undoubles_a_string_literal() {
        assert_eq!(classify(b"'ab'"), Lexeme::Str(b"ab".to_vec()));
        // A doubled quote collapses to one (§5.5).
        assert_eq!(classify(b"'a''b'"), Lexeme::Str(b"a'b".to_vec()));
    }

    #[test]
    fn source_position_is_reported_before_any_token() {
        let tracker = ScopeTracker::new();
        assert_eq!(
            tracker.position(State::ExpectSource),
            L2Position::SourceIdent
        );
        assert_eq!(tracker.position(State::BlockStmt), L2Position::SourceIdent);
    }

    #[test]
    fn a_bound_var_dot_yields_a_member_position_on_its_class() {
        // `|A.all()->filter(x|$x.` — x is bound to A, so the dot is N1 on A.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned())
        );
    }

    #[test]
    fn fused_nav_position_targets_the_bound_var_class_before_the_dot() {
        // `|A.all()->filter(x|$x` — the dot is NOT yet consumed (a fused `.member`
        // token would carry it). The fused pass must still see the coming nav as N1
        // on A, resolving the still-open refvar `x` from the pending buffer.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
        ];
        let (tracker, _pda) = run(tokens);
        assert_eq!(
            tracker.fused_nav_position(&schema()),
            Some(L2Position::Member("A".to_owned()))
        );
    }

    #[test]
    fn fused_nav_position_survives_a_dispatched_refvar_awaiting_its_dot() {
        // `$x ` — the refvar closed on whitespace (so it is dispatched, not pending),
        // yet a following `.member` still navigates A.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b" ",
        ];
        let (tracker, _pda) = run(tokens);
        assert_eq!(
            tracker.fused_nav_position(&schema()),
            Some(L2Position::Member("A".to_owned()))
        );
    }

    #[test]
    fn fused_nav_position_is_none_at_the_source_classpath() {
        // `|A` — the source classpath's coming dot opens `.all()`, not a member nav,
        // so a fused `.all` must never be narrowed against A's members.
        let (tracker, _pda) = run(&[b"|", b"A"]);
        assert_eq!(tracker.fused_nav_position(&schema()), None);
    }

    #[test]
    fn an_all_dot_is_a_source_method_position_not_a_member_navigation() {
        // The `.` of `A.all()` navigates from no bound var — no Member narrowing —
        // but it *is* narrowed: S1 requires exactly `all` here (A source dot and a
        // value dot share `AfterDot`, so this is `SourceMethod`, not `None`).
        let (tracker, pda) = run(&[b"|", b"A", b"."]);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(tracker.position(pda.state()), L2Position::SourceMethod);
    }

    #[test]
    fn a_quoted_member_after_a_source_dot_is_a_source_method_position() {
        // `|A.'name'` — a quoted member off a source dot. The dot shares the unified
        // `AfterDot` state (no bound var, so no Member narrowing), and the quoted
        // member streams cleanly to an accepting state — still `SourceMethod` (S1: a
        // quoted string is never `all`, so the narrower correctly rejects it here).
        // This guards the original revert this rule is layered onto: the source dot
        // must not be a separate, identifier-only PDA *state* that would reject a
        // quoted continuation outright — `SourceMethod` narrows the shared
        // `AfterDot` state's *position*, it does not change the automaton.
        let (tracker, pda) = run(&[b"|", b"A", b".", b"'name'"]);
        assert_eq!(pda.state(), State::InStrLit { escaped: true });
        assert_eq!(tracker.position(pda.state()), L2Position::SourceMethod);
    }

    #[test]
    fn a_nested_source_dot_is_deliberately_not_yet_a_source_method_position() {
        // `|A.all()->filter(x|B.all()->isEmpty())` — a nested subquery source. Its
        // classpath `B` dispatches from `ExpectValue` (a fresh lambda body's
        // opening value position — the same anchor a lambda binder candidate or
        // an `EnumPath.IDENT` value literal like `SortDirection.ASC` uses), not
        // `ExpectSource`/`BlockStmt`/`BlockStmtClose`. `on_open`'s existing
        // `SOURCE_METHOD && depth >= NESTED_SOURCE_MIN_DEPTH` check only
        // recognizes a nested source *retroactively*, once `.all(` is actually
        // seen — there is no positional signal available *before* that to
        // distinguish "this value-position classpath is about to become a
        // nested source" from a genuine value-position classpath (the exact
        // `SortDirection.ASC` ambiguity `SourceMethod`'s doc comment describes).
        // Extending S1 to nested sources needs that extra signal (e.g. "is this
        // the pipe's first identifier") built and proven not to also catch
        // `EnumPath.IDENT` — deliberately not attempted here; this test pins
        // today's actual (unchanged, pre-existing) behavior so a future change
        // is a visible, intentional decision, not a silent regression either way.
        let (tracker, pda) = run(&[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"B", b".",
        ]);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn a_classpath_at_a_value_position_does_not_arm_source_method_narrowing() {
        // A real class name dispatched from a *value* position (not a pipeline
        // source anchor) must never arm S1's `SOURCE_METHOD` narrowing on its
        // following dot — the regression guard the `EnumPath.IDENT` value-literal
        // shape (`SortDirection.ASC`, `docs/spec/schema.md` §5.7/N4) needs: it
        // shares the same `AfterDot` state as a source dot, but its classpath
        // anchors at a value position, not `ExpectSource`/`BlockStmt`/
        // `BlockStmtClose`. Drives `dispatch_token` directly (a private method,
        // reachable from this in-file test module) rather than a hand-built PDA
        // byte sequence, since the fact under test — which `pre_state` a
        // dispatch closed at — does not depend on how that state was reached.
        let mut tracker = ScopeTracker::new();
        tracker.dispatch_token(b"A", State::ExpectValue, &schema());
        assert!(
            tracker.source_path_seen.is_none(),
            "a value-position classpath armed source-path narrowing"
        );
    }

    #[test]
    fn the_source_methods_own_open_paren_is_a_source_method_arg_position() {
        // `|A.all(` — the position right after the call's own opening `(` must
        // be narrowed (N3's grammar constrains `all()`'s argument to at most a
        // milestoning date), not fall through to `L2Position::None` the way an
        // ordinary call's first argument slot does.
        let (tracker, pda) = run(&[b"|", b"A", b".", b"all", b"("]);
        assert_eq!(pda.state(), State::ExpectValue);
        assert_eq!(tracker.position(pda.state()), L2Position::SourceMethodArg);
    }

    #[test]
    fn a_milestoning_date_argument_is_admitted_and_the_call_still_closes() {
        // `|A.all(%latest)` — bitemporal milestoning's single-argument form
        // (corpus `differential_l1.jsonl`'s `Firm.all(%latest)`) must reach a
        // clean `AfterValue` close, proving `SourceMethodArg` does not regress
        // the pre-existing milestoning pass-through
        // (`a_milestoning_literal_is_an_l2_pass_through_operand`) into a mask.
        let (_tracker, pda) = run(&[b"|", b"A", b".", b"all", b"(", b"%latest", b")"]);
        assert_eq!(pda.state(), State::AfterValue);
    }

    #[test]
    fn an_ordinary_calls_open_paren_is_not_a_source_method_arg_position() {
        // `|A.all()->filter(` — `filter` is not `SOURCE_METHOD`, so its own
        // argument slot must stay unconstrained (a lambda binder candidate),
        // proving `in_source_method_args` is armed by the method name, not by every
        // call's opening paren.
        let (tracker, pda) = run(&[b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"("]);
        assert_eq!(pda.state(), State::ExpectValue);
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn source_method_arg_does_not_leak_past_the_calls_own_close() {
        // Drives `on_open`/`on_close` directly (private methods, reachable from
        // this in-file test module) to prove `in_source_method_args` cannot survive
        // its own call's close and wrongly mask an unrelated value position
        // reached without an intervening `on_open` — e.g. a comparison operand
        // directly following the call (a hypothetical `A.all() == 5`). Without
        // the unconditional clear in `on_close`, this would wrongly report
        // `SourceMethodArg` instead of the armed `ReValue` comparison.
        let mut tracker = ScopeTracker::new();
        tracker.last_ident = Some(SOURCE_METHOD.to_owned());
        tracker.on_open(State::AfterDot);
        assert_eq!(
            tracker.opening_position(State::ExpectValue),
            L2Position::SourceMethodArg,
            "sanity: opening the source method's call arms SourceMethodArg"
        );
        tracker.on_close();
        tracker.cmp_pending = Some(TypeClass::Numeric);
        assert_eq!(
            tracker.opening_position(State::ExpectValue),
            L2Position::ReValue(TypeClass::Numeric),
            "a stale SourceMethodArg flag must not mask a real comparison after the call's own close"
        );
    }

    #[test]
    fn the_let_keyword_at_a_source_anchor_does_not_arm_source_method_narrowing() {
        // The mirror image of the value-position guard above: `let` closes at a
        // genuine source-triggering anchor (`ExpectSource`/`BlockStmt`/
        // `BlockStmtClose`) — N3's own `source_paths().chain(once(LET_KEYWORD))`
        // admits it there — but it is not itself a source *path*
        // (`schema.source_paths()` never yields it), so `source_path_seen` must
        // stay empty. Without this check, `on_ident` would arm S1 after `let`,
        // wrongly forcing the block-statement's `$name = …` binder syntax through
        // a trie that only ever admits `all`.
        let mut tracker = ScopeTracker::new();
        tracker.dispatch_token(b"let", State::BlockStmt, &schema());
        assert!(
            tracker.source_path_seen.is_none(),
            "the `let` keyword armed source-path narrowing"
        );
    }

    #[test]
    fn a_shadowed_binder_is_restored_when_the_inner_scope_closes() {
        // Soundness (unit-level dual of the arm-R integration test in
        // `tests/l2_precision.rs`): a nested `B` subquery reuses the outer filter's
        // binder name `x` and rebinds it to B. When that inner lambda closes, `x` must
        // be restored to the outer `A` binding, so the outer `$x.n` (valid on A) is
        // not masked against B. Without binder-scope restoration, `$x.n` was masked.
        // `|A.all()->filter(x|B.all()->map(x|$x.m)->isEmpty() && $x.`
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"B", b".",
            b"all", b"(", b")", b"->", b"map", b"(", b"x", b"|", b"$", b"x", b".", b"m", b")",
            b"->", b"isEmpty", b"(", b")", b"&&", b"$", b"x", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned())
        );
    }

    #[test]
    fn a_nested_pipeline_source_class_does_not_leak_to_an_outer_navigation() {
        // Soundness: an outer `A` pipeline whose filter predicate is a *nested* `B`
        // subquery. The nested `B.all()` sets `cur_class` to B; scoping must restore
        // the outer `A` when the filter closes, so the outer `map(z|$z.` navigation is
        // narrowed against A (which has member `n`), not the leaked B. Without the
        // per-scope `cur_class` restore, `$z.n` was masked (B has no `n`).
        // `|A.all()->filter(x|B.all()->isEmpty())->map(z|$z.`
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"B", b".",
            b"all", b"(", b")", b"->", b"isEmpty", b"(", b")", b")", b"->", b"map", b"(", b"z",
            b"|", b"$", b"z", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned())
        );
    }

    #[test]
    fn a_primitive_navexpr_then_comparison_arms_t1_with_its_type_class() {
        // `$x.n ==` — n is Integer, so the operand position is ReValue(Numeric).
        let numeric: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"n", b"==",
        ];
        let (tracker, pda) = run(numeric);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::ReValue(TypeClass::Numeric)
        );
        // `$x.s ==` — s is String, so the operand is ReValue(Str).
        let string: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"s", b"==",
        ];
        let (tracker, pda) = run(string);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::ReValue(TypeClass::Str)
        );
    }

    #[test]
    fn a_milestoning_literal_is_an_l2_pass_through_operand() {
        // `%latest` is a `Lexeme::Date`, so it carries no L2 narrowing: neither a
        // bare source/argument position nor an *armed* comparison operand masks it
        // (the tracker maps a `ReValue` opening for a whole-token literal to
        // pass-through). `A.all(%latest)`-style milestoning therefore never risks
        // masking the model's emitted milestone symbol.
        let bare: &[&[u8]] = &[b"|", b"A", b".", b"all", b"(", b"%latest"];
        let (tracker, pda) = run(bare);
        assert_eq!(pda.state(), State::InMilestoneLit, "mid milestone literal");
        assert_eq!(tracker.position(pda.state()), L2Position::None);

        // Even after `$x.n ==` arms T1 (n is Integer), the `%latest` operand is
        // pass-through, not a masked `ReValue` position.
        let armed: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"n", b"==", b"%latest",
        ];
        let (tracker, pda) = run(armed);
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn an_arm_r_tilde_column_is_an_l2_pass_through() {
        // A `~`-column reference (`~A`, `~[Col: …]`) opens at the `SawTilde` anchor,
        // whose L2 rule is `None`: the synthetic relation-column name is never a
        // schema Member/Source/Column position, so arm-R never risks masking it.
        // A bare column ref inside `sort([ascending(~A)])`:
        let sort_ref: &[&[u8]] = &[
            b"|",
            b"A",
            b".",
            b"all",
            b"(",
            b")",
            b"->",
            b"sort",
            b"(",
            b"[",
            b"ascending",
            b"(",
            b"~",
            b"A",
        ];
        let (tracker, pda) = run(sort_ref);
        assert_eq!(pda.state(), State::InIdent, "mid `~A` column name");
        assert_eq!(tracker.position(pda.state()), L2Position::None);

        // The `~[` column-set opener and its `Col` name in `project(~[Col: …])`:
        let project_set: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"project", b"(", b"~", b"[", b"Col",
        ];
        let (tracker, pda) = run(project_set);
        assert_eq!(
            pda.state(),
            State::InIdent,
            "mid `Col` relation column name"
        );
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn a_merged_closing_quote_records_the_true_column_bytes() {
        // H1: a string literal fused with its trailing `)` into one token
        // (`'ab')`) must still record the byte-exact content `ab` in the emitted
        // set — not the garbage `'ab')` the whole-token `unquote` produced. The
        // buried `)` must also fire `on_close` (the filter paren balances).
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"s", b"==", b"'ab')",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(
            tracker.emitted_columns(),
            [b"ab".to_vec()],
            "the merged closing quote records `ab`, byte-exact"
        );
        // The `)` buried in the token closed the filter paren: back at top level.
        assert_eq!(pda.state(), State::AfterValue);
        assert!(pda.stack_top().is_none(), "the filter paren is closed");
    }

    #[test]
    fn a_doubled_quote_in_a_merged_close_undoubles_byte_exact() {
        // `'a''b')` — a doubled `''` inside the literal collapses to one `'`, and
        // the trailing `)` is not part of the recorded content.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"s", b"==", b"'a''b')",
        ];
        let (tracker, _pda) = run(tokens);
        assert_eq!(tracker.emitted_columns(), [b"a'b".to_vec()]);
    }

    #[test]
    fn a_buried_navigation_dot_still_fires_member_narrowing() {
        // H2: a `.` fused to the leading identifier byte (`.n`) must still fire
        // `on_dot`, arming the member position on the bound var's class — else the
        // buried dot would silently disable N1 (pass-through) rather than narrow.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".n",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::InIdent, "landed mid-identifier `n`");
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned()),
            "the buried dot armed N1 on A for the buffered member"
        );
    }

    #[test]
    fn a_multi_byte_operator_swallowed_in_a_gap_is_not_split() {
        // A structural gap fusing a value's tail into `->` (`n->`, then a step) must
        // munch `->` whole (an Arrow), not a stray `>` that would read as a
        // comparison and mis-arm T1. Feeding `n->` then a fresh nav must resolve the
        // navExpr, not leave a dangling comparison arming.
        let numeric: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"n", b"==", b"5",
        ];
        let (tracker, pda) = run(numeric);
        // After the operand `5`, T1 arming is spent; the operand position is clear.
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn a_comparison_without_a_resolved_navexpr_does_not_arm_t1() {
        // `take(1 ==` never resolved a primitive navExpr, so no T1 arming — the
        // operand position stays unconstrained (pass-through).
        let (tracker, pda) = run(&[b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"("]);
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn unquote_undoubling_pins_its_scan_indices() {
        // These two literals pin the undouble loop's index math, which `'ab'` /
        // `'a''b'` leave unconstrained. `''''` is a doubled quote alone: only a
        // scan that skips exactly the *doubled* pair (not any quote, not the wrong
        // count) collapses it to one. `''x'` carries a lone quote followed by more
        // content: the "look at the *next* byte" check must not read the current one.
        assert_eq!(classify(b"''''"), Lexeme::Str(b"'".to_vec()));
        assert_eq!(classify(b"''x'"), Lexeme::Str(b"'x".to_vec()));
    }

    #[test]
    fn an_arm_r_map_lambda_binder_narrows_columns_after_a_preceding_filter() {
        // The arm-R aggregation map lambda binds its variable *after* a colon
        // (`~'s': x|…`), which the byte-PDA parks in an `InIdent` reached from
        // `AfterColon`/`AfterColonWs`. That binder is re-captured (gap report G-L2, so
        // a re-used name cannot keep the class a preceding `filter(x|…)` gave it) and,
        // because the pipeline is now an arm-R relation, bound as a *relation row*: its
        // `$x.C` is a bare-ident column access narrowed against the emitted-column
        // universe (which contains the projected column `C`), not a class member.
        //
        // `|A.all()->filter(x|$x.n>=0)->project(~[C: x|$x.n])->groupBy(~[C], ~'s': x|$x.`
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"x", b"|", b"$", b"x",
            b".", b"n", b">=", b"0", b")", b"->", b"project", b"(", b"~", b"[", b"C", b":", b"x",
            b"|", b"$", b"x", b".", b"n", b"]", b")", b"->", b"groupBy", b"(", b"~", b"[", b"C",
            b"]", b",", b"~", b"'s'", b":", b"x", b"|", b"$", b"x", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot, "at the `$x.` column position");
        assert_eq!(tracker.position(pda.state()), L2Position::RelationColumn);
        // `C` is in the emitted-column universe, so the narrower admits it — the real
        // projected column is never masked.
        assert!(tracker.emitted_columns().contains(&b"C".to_vec()));
    }

    #[test]
    fn an_arm_a_tds_row_is_not_narrowed_as_an_arm_r_column() {
        // An arm-A `project([…], […])` opens no `~[`, so the pipeline never latches
        // arm-R: a following TDS-row binder stays off the relation-column path and
        // `$r.getString(…)` is pass-through (`None`), never masked as a phantom column.
        // `|A.all()->project([x|$x.n], ['Name'])->filter(r|$r.`
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"project", b"(", b"[", b"x", b"|", b"$",
            b"x", b".", b"n", b"]", b",", b"[", b"'Name'", b"]", b")", b"->", b"filter", b"(",
            b"r", b"|", b"$", b"r", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(tracker.position(pda.state()), L2Position::None);
    }

    #[test]
    fn the_emitted_column_universe_collects_arm_r_names() {
        // The column universe is a superset: it records `~[…]` keys (project and
        // groupBy), bare `~col` refs, and quoted agg names — so a later `$row.Col`
        // narrowing finds every emitted column. The sort uses a *distinct* `~Bare`
        // so the bare-`~` (`SawTilde`) recording path is verified independently of
        // the `~[…]` key `C`.
        // `|A.all()->project(~[C: x|$x.n])->groupBy(~[C], ~'Agg': x|$x.C : y|$y->sum())->sort([ascending(~Bare)])`
        let tokens: &[&[u8]] = &[
            b"|",
            b"A",
            b".",
            b"all",
            b"(",
            b")",
            b"->",
            b"project",
            b"(",
            b"~",
            b"[",
            b"C",
            b":",
            b"x",
            b"|",
            b"$",
            b"x",
            b".",
            b"n",
            b"]",
            b")",
            b"->",
            b"groupBy",
            b"(",
            b"~",
            b"[",
            b"C",
            b"]",
            b",",
            b"~",
            b"'Agg'",
            b":",
            b"x",
            b"|",
            b"$",
            b"x",
            b".",
            b"C",
            b":",
            b"y",
            b"|",
            b"$",
            b"y",
            b"->",
            b"sum",
            b"(",
            b")",
            b")",
            b"->",
            b"sort",
            b"(",
            b"[",
            b"ascending",
            b"(",
            b"~",
            b"Bare",
            b")",
            b"]",
            b")",
        ];
        let (tracker, _pda) = run(tokens);
        let cols = tracker.emitted_columns();
        assert!(
            cols.contains(&b"C".to_vec()),
            "project/groupBy key `C` recorded"
        );
        assert!(
            cols.contains(&b"Agg".to_vec()),
            "quoted agg name `Agg` recorded"
        );
        assert!(
            cols.contains(&b"Bare".to_vec()),
            "bare `~Bare` sort reference recorded"
        );
    }

    #[test]
    fn a_typed_value_binder_keeps_its_pre_colon_binding() {
        // The dual concern of the arm-R capture: a *typed* value-position binder
        // (`filter(row: A|$row.…)`) names its binder *before* the colon and its class
        // *after* it. The post-colon `A` is a schema class, so it must not be
        // mistaken for the binder — `row` stays bound to `A` and N1 still narrows
        // `$row.n`. Without the `has_class` guard the type name would overwrite the
        // binder and this position would degrade to a pass-through.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"filter", b"(", b"row", b":", b"A", b"|",
            b"$", b"row", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned())
        );
    }

    #[test]
    fn an_arm_r_project_map_lambda_binder_binds_to_the_source_class() {
        // The dual of the soundness test: inside `project(~[C: x|$x.` the binder `x`
        // *is* a row of the source relation, so it must bind to the source class `A`
        // (N1 narrows `$x.n` against A) — the rebinding fix must not degrade this
        // still-typed position to pass-through.
        let tokens: &[&[u8]] = &[
            b"|", b"A", b".", b"all", b"(", b")", b"->", b"project", b"(", b"~", b"[", b"C", b":",
            b"x", b"|", b"$", b"x", b".",
        ];
        let (tracker, pda) = run(tokens);
        assert_eq!(pda.state(), State::AfterDot);
        assert_eq!(
            tracker.position(pda.state()),
            L2Position::Member("A".to_owned())
        );
    }

    #[test]
    fn an_identifier_split_across_tokens_continues_one_buffer() {
        // `filter` arrives as two tokens `fil` + `ter`. The second token's pre-state
        // is still mid-identifier, so it must *continue* the first token's pending
        // buffer, not flush `fil` and start `ter` afresh. Only a correct cross-token
        // continuation leaves the whole `filter` as the open narrowing prefix.
        let (tracker, _) = run(&[b"|", b"A", b".", b"all", b"(", b")", b"->", b"fil", b"ter"]);
        assert_eq!(tracker.narrow_prefix(), b"filter");
    }
}
