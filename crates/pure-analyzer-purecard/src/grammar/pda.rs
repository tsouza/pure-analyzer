//! The byte-level pushdown automaton for the emitted-Pure grammar (§5).
//!
//! This is the live automaton: an explicit, hand-written state machine, not a
//! compiled EBNF table (see `docs/spec/grammar.md`). [`step`] is a
//! **pure** transition function — `(state, stack_top, byte) -> `[`Step`] — with no
//! I/O, allocation, or hidden state; [`Pda`] is the thin mutable driver that
//! applies each [`Step`] to a state field and a [`Frame`] stack.
//!
//! ## Shape of the recognizer
//!
//! The grammar is a *pipeline of terms*: `source ( "->" step )*`, where a term is
//! an identifier / classpath, a literal, a `$`-var navigation, a lambda, a list,
//! or a parenthesised sub-expression. Rather than one state per named production,
//! the automaton lexes byte-by-byte around the value hubs — [`State::ExpectValue`]
//! / [`State::ExpectValueReq`] (at the start of a term) and [`State::AfterValue`]
//! (having just completed one) — and defers all delimiter nesting to the [`Frame`]
//! stack. The machine is a *deliberate, residual* over-approximation of §5: it
//! still admits strings the compiler rejects — arithmetic/`if` type coherence,
//! projected-column vs name-count equality, typed-binder multiplicity — but those
//! are exactly, and only, the escapes §5.6 enumerates. §5.6 does **not** sanction
//! dropping the `source` production, the `->` connector between steps, keyword
//! terminals, operator arity, or literal well-formedness, and the machine does not
//! drop them: a query must open with `|`/`{|` on an *identifier source*; a
//! completed term is followed by `->`/`.`/`::`/`(`/an operator/a closer (never a
//! bare abutting identifier outside a `let` binder); every binary operator demands
//! an operand; numeric and date literals are well-formed; brackets balance against
//! the matching opener; strings close on an un-doubled quote; and `$`/`.`/`->`/`:`
//! each demand the token that may follow. Because neither soundness (100% by
//! construction over all-gold), coverage, nor mutation can *observe*
//! over-acceptance, this precision is pinned externally: by the negative reject
//! corpus (`tests/precision_reject.rs`) and the seeded completeness walker
//! (§8.2/G3/T8). This comment must not be read to excuse a widening beyond §5.6.
//!
//! Multi-byte operators (`->`, `::`, `==`, `&&`, `||` vs. the lambda `|`, …) are
//! recognised by a "saw first byte" state that consults the *next* byte and, when
//! the second byte does not complete the operator, **delegates** it back into the
//! hub state it belongs to by re-invoking [`step`]. That delegation is what keeps
//! the machine a true byte-at-a-time recogniser without any look-ahead buffer.

use crate::grammar::DeadState;

/// A recognizer state: a position in the byte-level parse of an emitted-Pure
/// query.
///
/// The two hubs are [`ExpectValue`](State::ExpectValue) (the machine is about to
/// read a fresh term) and [`AfterValue`](State::AfterValue) (it has just finished
/// one and expects an operator, separator, or closer). Every other variant is a
/// transient lexical position: inside an identifier, number, string, or date
/// literal, or one byte into a multi-byte operator.
/// `#[non_exhaustive]`: the variant set is the decoder's internal state machine,
/// exposed only so a caller can *observe* the automaton configuration (via
/// [`Pda::state`]) — it is not a stability contract. Variants are added as the
/// grammar grows (e.g. `SawTilde`); a downstream match on
/// `State` must carry a `_` arm rather than break on each addition. In-crate
/// exhaustive matches (`step`, `name`, `index`) are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum State {
    /// Before the first byte: only `|` (a simple query) or `{` (a block query)
    /// may open the stream.
    Start,
    /// Right after a top-level `|` or a block body's `{|`: the pipeline *source*
    /// begins here, and a source is always an identifier classpath (`X.all()` /
    /// `db::Db->…`). A literal, `$`-var, bracket, or operator in source position is
    /// a dead state — a query is never a bare value like `|42` or `|( )`.
    ExpectSource,
    /// Right after a `{` opened a *block query* at the stream start: only `|` (with
    /// optional leading whitespace) may follow, so a block query is always `{|…}`.
    AfterBraceOpen,
    /// At the start of a block-query statement — right after `{|` or a `;`. Only a
    /// `let` binding, a pipeline source (a classpath ident or a `$`-var), or (after
    /// a `;`) the trailing `}` may follow; a bare literal statement is a dead state.
    /// The two variants differ only in whether `}` is legal here.
    BlockStmt,
    /// Like [`BlockStmt`](State::BlockStmt) but a trailing `}` may close the block
    /// — the position right after a `;`, so `;}` completes a block query.
    BlockStmtClose,
    /// Inside a pipeline *source* classpath identifier segment. Unlike the generic
    /// [`InIdent`](State::InIdent), a source is not a completed value: only a `.`
    /// (routing into [`AfterDot`](State::AfterDot) — `.all()`, a property/getter, or
    /// a quoted member `X.'name'`), `->` (arm-A `tableReference`), or a `::` classpath
    /// separator may follow — never whitespace-to-accepting or a closer, so a bare
    /// `|X ` dies.
    InSourceIdent,
    /// Just consumed the first `:` of a source-classpath `::` separator; a second
    /// `:` must follow immediately (`db::Db`), so a lone `:` or interior whitespace
    /// in source position is a dead state.
    SourceColon,
    /// Just consumed the second `:` of a source-classpath `::`; a classpath
    /// identifier must follow, keeping the source in its own state across the `::`.
    SourceColon2,
    /// Just consumed a `-` in source position; only `>` (completing `->`) may
    /// follow — a source is never the left operand of arithmetic minus.
    SourceDash,
    /// Consumed `l` at a block-statement start: a candidate `let` keyword. Falls
    /// back to a source identifier for any classpath that merely begins with `l`.
    LetL,
    /// Consumed `le`: still a candidate `let`.
    LetLe,
    /// Consumed `let`: the `let` keyword only if whitespace follows; otherwise the
    /// bytes were the prefix of a longer source identifier (`letters`, `let.foo`).
    LetLet,
    /// After the `let` keyword and its whitespace: the binder name identifier must
    /// follow (`let m = …`).
    ExpectBinder,
    /// Inside a `let` binder name identifier.
    InBinder,
    /// After a completed binder name: whitespace then the single `=` that opens the
    /// binding's right-hand-side pipeline (`let m = …`). A second name is dead.
    AfterBinder,
    /// Right after a `[` that holds a `*` multiplicity token: only the closing `]`
    /// may follow, so `[*]` is the only shape `*` reaches (never `take(*)`).
    InMultiplicity,
    /// Right after the `{` of a `join` brace lambda: a typed binder identifier must
    /// follow (`{r1: …[1], … | body}`), so a literal body like `{1}` is dead.
    ExpectBraceBinder,
    /// After a single `:` *and* the whitespace that followed it — a typed-binder
    /// colon with trailing space (`row: Type`). A `::` must be contiguous, so a
    /// second `:` here (`meta: :pure`) is a dead state; only an identifier may follow.
    AfterColonWs,
    /// At the start of a term where the term is *optional*: entered right after a
    /// `(` or `[` (or a block-body `;`), so a matching closer may legally follow —
    /// the empty argument list `all()`, the empty list `[]`, the empty key
    /// `groupBy([]…)`, or the trailing `;}` of a block query.
    ExpectValue,
    /// At the start of a term where the term is *required*: entered after a binary
    /// operator, a `,`, a lambda/`||` pipe, or a unary `!`/`-`. Identical to
    /// [`ExpectValue`](State::ExpectValue) except a closer is a dead state, so an
    /// operator may not dangle against a `)`/`]`/`}` (`take(1 +)`, `$x.a && )`).
    ExpectValueReq,
    /// Having just completed a term; an operator, separator, call, or closer may
    /// follow.
    AfterValue,
    /// Inside an identifier or classpath segment (`[A-Za-z_][A-Za-z0-9_]*`).
    InIdent,
    /// Just consumed the `-` sign of a numeric literal in value position; a digit
    /// must follow (a digit for `-5`, or a `.` for the leading-dot float `-.5`), so
    /// a bare `-` or `--5` is a dead state.
    SawNumSign,
    /// Inside the integer part of a number literal.
    InNumberInt,
    /// Just consumed the `.` of a number literal (or a leading `.`); at least one
    /// fractional digit must follow, so a trailing `1.` or a bare `.` is a dead state.
    NeedFracDigit,
    /// Inside the fractional part of a number literal, after the `.`. An `e`/`E`
    /// here opens a scientific-notation exponent.
    InNumberFrac,
    /// Just consumed the `e`/`E` of an exponent; an optional sign then a digit
    /// follow (`1.5e3`, `1.5e-3`).
    SawExp,
    /// An exponent sign (`+`/`-`) was seen; a digit is required.
    NeedExpDigit,
    /// Inside the digits of a scientific-notation exponent.
    InExp,
    /// Inside a single-quoted string literal.
    ///
    /// `escaped` is `true` when the previous byte was a `'` whose role — closing
    /// quote or the first half of a doubled `''` — is decided by the current
    /// byte (§5.5 quote doubling).
    InStrLit {
        /// Whether a pending `'` is awaiting its disambiguating byte.
        escaped: bool,
    },
    /// Just consumed `%`; at least one date character must follow, so a bare `%`
    /// (`take(%)`) is a dead state.
    SawPercent,
    /// Inside a `%`-prefixed date/time literal (`%2018-03-17T07:13:53`).
    InDateLit,
    /// Inside a `%`-prefixed *symbolic* milestoning literal (`%latest`,
    /// `%latestdate`): a `%` sigil followed by lowercase ASCII letters. Distinct
    /// from [`InDateLit`](State::InDateLit) so a milestone symbol and a numeric
    /// date literal never share a byte class; value-terminal like `InDateLit`.
    InMilestoneLit,
    /// Just consumed `$`; a `refVar` identifier must follow.
    AfterDollar,
    /// Just consumed `.`; a property / getter / `all` identifier, or a quoted
    /// member/column name (`$x.'Gross Credits'`, `X.'name'`), must follow — in both
    /// value-navigation and pipeline-source position (the Legend grammar admits the
    /// same set — ws / identifier / quoted string — after either dot).
    AfterDot,
    /// Just consumed `->`; a step / method / reducer identifier must follow.
    AfterArrow,
    /// Just consumed a single `:` (a typed-binder colon `row: …[1]`) or the first
    /// `:` of a `::` classpath separator; a classpath identifier or a second `:`
    /// must follow.
    AfterColon,
    /// Just consumed the second `:` of a `::` classpath separator; a classpath
    /// identifier must follow. A third `:` is a dead state — `:::` is never valid.
    AfterColon2,
    /// Just consumed `-`; a `>` completes `->`, anything else is arithmetic minus.
    SawDash,
    /// Just consumed `|`; a second `|` is boolean `||`, anything else is the
    /// lambda-binder pipe and starts the body.
    SawPipe,
    /// Just consumed `=`; an optional second `=` completes `==` (vs. `let x =`).
    SawEq,
    /// Just consumed `!`; a `=` must follow to complete `!=`.
    SawBang,
    /// Just consumed `>`; an optional `=` completes `>=`.
    SawGt,
    /// Just consumed `<`; an optional `=` completes `<=`.
    SawLt,
    /// Just consumed `&`; a second `&` must follow to complete `&&`.
    SawAmp,
    /// Just consumed `~`: the Relation/Function API sigil (arm-R). A `[` opens a
    /// relation column-set (`project(~[…])`), and a bare identifier or a
    /// single-quoted string is a column reference (`~Week` / `~'Gross Credits'`).
    /// Nothing else — including whitespace — may follow, so `~ )` and `~~` die.
    SawTilde,
}

impl State {
    /// A stable name for this state, used in [`DecodeError::DeadState`] so a
    /// soundness failure names the exact production position that rejected a byte
    /// (`docs/spec/grammar.md`).
    ///
    /// [`DecodeError::DeadState`]: crate::DecodeError::DeadState
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            State::Start => "Start",
            State::ExpectSource => "ExpectSource",
            State::AfterBraceOpen => "AfterBraceOpen",
            State::BlockStmt => "BlockStmt",
            State::BlockStmtClose => "BlockStmtClose",
            State::InSourceIdent => "InSourceIdent",
            State::SourceColon => "SourceColon",
            State::SourceColon2 => "SourceColon2",
            State::SourceDash => "SourceDash",
            State::LetL => "LetL",
            State::LetLe => "LetLe",
            State::LetLet => "LetLet",
            State::ExpectBinder => "ExpectBinder",
            State::InBinder => "InBinder",
            State::AfterBinder => "AfterBinder",
            State::InMultiplicity => "InMultiplicity",
            State::ExpectBraceBinder => "ExpectBraceBinder",
            State::AfterColonWs => "AfterColonWs",
            State::ExpectValue => "ExpectValue",
            State::ExpectValueReq => "ExpectValueReq",
            State::AfterValue => "AfterValue",
            State::InIdent => "InIdent",
            State::SawNumSign => "SawNumSign",
            State::InNumberInt => "InNumberInt",
            State::NeedFracDigit => "NeedFracDigit",
            State::InNumberFrac => "InNumberFrac",
            State::SawExp => "SawExp",
            State::NeedExpDigit => "NeedExpDigit",
            State::InExp => "InExp",
            State::InStrLit { escaped: false } => "InStrLit",
            State::InStrLit { escaped: true } => "InStrLit(pendingQuote)",
            State::SawPercent => "SawPercent",
            State::InDateLit => "InDateLit",
            State::InMilestoneLit => "InMilestoneLit",
            State::AfterDollar => "AfterDollar",
            State::AfterDot => "AfterDot",
            State::AfterArrow => "AfterArrow",
            State::AfterColon => "AfterColon",
            State::AfterColon2 => "AfterColon2",
            State::SawDash => "SawDash",
            State::SawPipe => "SawPipe",
            State::SawEq => "SawEq",
            State::SawBang => "SawBang",
            State::SawGt => "SawGt",
            State::SawLt => "SawLt",
            State::SawAmp => "SawAmp",
            State::SawTilde => "SawTilde",
        }
    }

    /// A stable dense index in `0..`[`COUNT`](State::COUNT), so a per-state cache
    /// can be a plain `Vec` keyed by state (§4.2).
    ///
    /// The match is **exhaustive with no wildcard arm**: adding a `State` variant
    /// without extending this map is a compile error, not a silent cache
    /// mis-index (Risk R4, constitution §5 — the fix closes the whole class). The
    /// two [`InStrLit`](State::InStrLit) configurations are distinct automaton
    /// states, so they take distinct indices. `index_is_a_bijection` pins that the
    /// map is one-to-one onto `0..COUNT`.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            State::Start => 0,
            State::ExpectSource => 1,
            State::AfterBraceOpen => 2,
            State::BlockStmt => 3,
            State::BlockStmtClose => 4,
            State::InSourceIdent => 5,
            State::SourceColon => 6,
            State::SourceColon2 => 7,
            State::SourceDash => 8,
            State::LetL => 9,
            State::LetLe => 10,
            State::LetLet => 11,
            State::ExpectBinder => 12,
            State::InBinder => 13,
            State::AfterBinder => 14,
            State::InMultiplicity => 15,
            State::ExpectBraceBinder => 16,
            State::AfterColonWs => 17,
            State::ExpectValue => 18,
            State::ExpectValueReq => 19,
            State::AfterValue => 20,
            State::InIdent => 21,
            State::SawNumSign => 22,
            State::InNumberInt => 23,
            State::NeedFracDigit => 24,
            State::InNumberFrac => 25,
            State::InStrLit { escaped: false } => 26,
            State::InStrLit { escaped: true } => 27,
            State::SawPercent => 28,
            State::InDateLit => 29,
            State::AfterDollar => 30,
            State::AfterDot => 31,
            State::AfterArrow => 32,
            State::AfterColon => 33,
            State::AfterColon2 => 34,
            State::SawDash => 35,
            State::SawPipe => 36,
            State::SawEq => 37,
            State::SawBang => 38,
            State::SawGt => 39,
            State::SawLt => 40,
            State::SawAmp => 41,
            State::InMilestoneLit => 42,
            State::SawTilde => 43,
            State::SawExp => 44,
            State::NeedExpDigit => 45,
            State::InExp => 46,
        }
    }

    /// The number of distinct automaton states — the length a per-state cache
    /// (`Vec<_>` keyed by [`index`](State::index)) must have. One more than the
    /// largest [`index`](State::index).
    pub const COUNT: usize = 47;

    /// The lexeme class this state is *inside*, if any (`None` = an inter-lexeme
    /// or structural position).
    ///
    /// A lexeme is **open** while `lexeme_kind` is `Some(k)` and **closes** at the
    /// byte whose transition takes `Some(k)` to any other verdict. The L2 scope
    /// tracker uses this to buffer a multi-token identifier / string until it
    /// completes (so a byte-level-BPE fragmentation resolves and narrows against
    /// the *whole* lexeme, not a leading sub-token). The `::` classpath-separator
    /// states stay `Ident` so a source classpath never flushes mid-path.
    pub(crate) const fn lexeme_kind(self) -> Option<LexKind> {
        match self {
            State::InIdent
            | State::InSourceIdent
            | State::InBinder
            | State::SourceColon
            | State::SourceColon2 => Some(LexKind::Ident),
            State::SawNumSign
            | State::InNumberInt
            | State::NeedFracDigit
            | State::InNumberFrac
            | State::SawExp
            | State::NeedExpDigit
            | State::InExp => Some(LexKind::Number),
            State::SawPercent | State::InDateLit | State::InMilestoneLit => Some(LexKind::Date),
            State::InStrLit { .. } => Some(LexKind::Str),
            _ => None,
        }
    }
}

/// The four lexeme classes a partial query token can be *inside* (§6.4). The L2
/// scope tracker buffers a lexeme across token boundaries keyed on this class, so
/// a BPE-fragmented identifier or string is resolved and narrowed whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LexKind {
    /// An identifier or `::`-joined classpath.
    Ident,
    /// A numeric literal.
    Number,
    /// A single-quoted string literal.
    Str,
    /// A `%`-prefixed date/time literal.
    Date,
}

/// Every [`Frame`] kind — the whole stack alphabet. Used by [`Pda::probe`] to
/// decide whether a byte that dies against an *empty* local scratch would have
/// lived against *some* ambient frame (i.e. its admissibility is
/// stack-dependent).
const ALL_FRAMES: [Frame; 4] = [
    Frame::Paren,
    Frame::Bracket,
    Frame::Brace,
    Frame::BraceLambda,
];

/// A stack frame: an open delimiter awaiting its match.
///
/// The frame kind makes bracket matching **context-dependent** (§4.2): a `)`
/// closes only a [`Paren`](Frame::Paren), a `]` only a [`Bracket`](Frame::Bracket),
/// and a `}` only a [`Brace`](Frame::Brace); any other pairing is a dead state.
/// The three delimiter kinds are the whole stack alphabet — pipeline `->` chains
/// and lambda bodies need no resume marker because the [`State::ExpectValue`] /
/// [`State::AfterValue`] hubs already encode "what may come next" without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// An open `(` — a call's argument list or a parenthesised expression.
    Paren,
    /// An open `[` — a list literal or a `[mult]` multiplicity bracket.
    Bracket,
    /// An open `{` of a block query (`{|…}`). The `let`/`;`/`=` block rules key on
    /// this frame, so they never leak into a `join` brace lambda.
    Brace,
    /// An open `{` of a `join` brace lambda (`{r1: …[1], … | body}`) — a distinct
    /// frame from [`Brace`](Frame::Brace) so a lone `=` inside the lambda body is
    /// not mistaken for a block-query `let` binder.
    BraceLambda,
}

impl Frame {
    /// A stable name for this frame, used in [`DecodeError::DeadState`]'s
    /// `stack_top` field.
    ///
    /// [`DecodeError::DeadState`]: crate::DecodeError::DeadState
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Frame::Paren => "Paren",
            Frame::Bracket => "Bracket",
            Frame::Brace => "Brace",
            Frame::BraceLambda => "BraceLambda",
        }
    }
}

/// The outcome of feeding one byte to [`step`].
///
/// [`Pop`](Step::Pop) is only ever returned when the byte's closer matches the
/// current `stack_top`, so [`Pda`] can pop unconditionally; a mismatched or
/// missing opener yields [`Dead`](Step::Dead) instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Stay within the current frame; move to the given state.
    Next(State),
    /// Open a new delimiter: push the frame, move to the given state.
    Push(Frame, State),
    /// Close the current (matched) delimiter: pop the stack, move to the state.
    Pop(State),
    /// No valid continuation: the byte is rejected.
    Dead,
}

/// A single space, tab, newline, or carriage return: the inter-token whitespace
/// skipped between — never inside — tokens.
const WS: &[u8; 4] = b" \t\n\r";

/// The canonical inter-token *value boundary* byte (a space): the terminator
/// [`Pda::is_accepting`] feeds a candidate state to decide, *through [`step`]
/// itself*, whether the state has finished a value. A value-terminal lexical
/// state delegates a whitespace byte to [`State::AfterValue`]; a mid-token or
/// hub state does not. Deriving acceptance from `step` this way keeps a single
/// source of truth for terminality (constitution §4, DRY) — no hand-maintained
/// list of accepting states to drift.
const VALUE_BOUNDARY: u8 = b' ';

fn is_ws(byte: u8) -> bool {
    WS.contains(&byte)
}

pub(crate) const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Whether `byte` may continue an identifier — the byte-PDA's own boundary
/// predicate. Exposed to the L2 trie walk (`schema::trie`) so an "identifier
/// still open" verdict shares the automaton's exact notion of an identifier tail,
/// rather than re-deriving it (constitution §4, DRY).
pub(crate) const fn is_ident_tail(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The bytes that may appear inside a `%`-prefixed date/time literal: digits and
/// the `-`, `T`, `:` separators (`%2018-03-17T07:13:53`).
const fn is_date_char(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'-' | b'T' | b':')
}

/// Close `top` if `byte` is its matching closer, else [`Step::Dead`].
///
/// The one place delimiter matching is decided; both hubs route their `)`/`]`/`}`
/// here so the context-dependent pop lives in a single spot.
const fn close(top: Option<Frame>, byte: u8) -> Step {
    match (top, byte) {
        (Some(Frame::Paren), b')')
        | (Some(Frame::Bracket), b']')
        | (Some(Frame::Brace), b'}')
        | (Some(Frame::BraceLambda), b'}') => Step::Pop(State::AfterValue),
        _ => Step::Dead,
    }
}

/// The shared body of the two value-position hubs. `allow_close` distinguishes
/// [`State::ExpectValue`] (a term is optional, so a matching closer is legal) from
/// [`State::ExpectValueReq`] (a term is required, so a closer is a dead state); the
/// two states are otherwise byte-for-byte identical, so keeping one body honours
/// DRY (constitution §4) and guarantees they never drift apart.
fn value_position(stack_top: Option<Frame>, byte: u8, allow_close: bool) -> Step {
    let ws_state = if allow_close {
        State::ExpectValue
    } else {
        State::ExpectValueReq
    };
    match byte {
        b if is_ws(b) => Step::Next(ws_state),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        b if b.is_ascii_digit() => Step::Next(State::InNumberInt),
        b'-' => Step::Next(State::SawNumSign),
        // A leading `.` opens a fractional float (`.5`); at least one fractional
        // digit is then required (`.` alone dies), matching the engine's `.5` float.
        b'.' => Step::Next(State::NeedFracDigit),
        b'\'' => Step::Next(State::InStrLit { escaped: false }),
        b'%' => Step::Next(State::SawPercent),
        b'$' => Step::Next(State::AfterDollar),
        // A `~` is the Relation/Function API sigil (arm-R): a relation column-set
        // `~[…]` or a column reference `~Week` / `~'Gross Credits'`.
        b'~' => Step::Next(State::SawTilde),
        b'(' => Step::Push(Frame::Paren, State::ExpectValue),
        b'[' => Step::Push(Frame::Bracket, State::ExpectValue),
        // A `{` in value position opens a `join` brace lambda; it must begin with a
        // typed binder identifier (`{r1: …[1], … | body}`), so a literal body like
        // `{1}` is a dead state. Its own `Frame::BraceLambda` keeps the block-query
        // `let`/`;`/`=` rules from leaking into the lambda body.
        b'{' => Step::Push(Frame::BraceLambda, State::ExpectBraceBinder),
        // A bare `|` opens a zero-arg lambda body (`if(c, |x, |y)`); the body value
        // is required.
        b'|' => Step::Next(State::ExpectValueReq),
        // A `!` in value position is the unary boolean-NOT prefix
        // (`&& !$s.name->in(…)`); its operand is required.
        b'!' => Step::Next(State::ExpectValueReq),
        // A `*` is only ever a multiplicity token, valid solely as the sole content
        // of a `[…]` bracket (`TDSRow[*]`). It is legal only in a fresh bracket value
        // position (`allow_close`, i.e. right after `[`), never as an arithmetic or
        // argument value — so `take(*)` and `take(1 + *)` are dead states.
        b'*' if allow_close && stack_top == Some(Frame::Bracket) => {
            Step::Next(State::InMultiplicity)
        }
        b')' | b']' | b'}' if allow_close => close(stack_top, byte),
        _ => Step::Dead,
    }
}

/// The shared body of the two block-statement states. A block query is
/// `{| (let name = pipeline ;)* pipeline ;? }`, so a statement start admits a `let`
/// binding, a pipeline source (a classpath identifier or a `$`-var), or nothing
/// but whitespace before them. `allow_close` (the post-`;` position) additionally
/// admits the trailing `}`; the post-`{|` position does not, so an empty `{|}` is a
/// dead state. A bare literal statement (`{|42;}`) is rejected — a query result is a
/// pipeline, never a scalar.
fn block_stmt(stack_top: Option<Frame>, byte: u8, allow_close: bool) -> Step {
    let ws_state = if allow_close {
        State::BlockStmtClose
    } else {
        State::BlockStmt
    };
    match byte {
        b if is_ws(b) => Step::Next(ws_state),
        // `l` may begin the `let` keyword; it falls back to a source classpath that
        // merely starts with `l`.
        b'l' => Step::Next(State::LetL),
        b if is_ident_start(b) => Step::Next(State::InSourceIdent),
        b'$' => Step::Next(State::AfterDollar),
        b'}' if allow_close => close(stack_top, byte),
        _ => Step::Dead,
    }
}

fn step_start(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::Start),
        // A simple query opens with `|` on its pipeline source.
        b'|' => Step::Next(State::ExpectSource),
        // A block query opens with `{`, and the `|` of `{|` must follow.
        b'{' => Step::Push(Frame::Brace, State::AfterBraceOpen),
        _ => Step::Dead,
    }
}

// After a top-level `|` (a simple query's source) or a `let name =` binding's
// `=` (its right-hand-side pipeline source): the source is always an
// identifier classpath. Whitespace is skipped; anything but an identifier
// start is a dead state (`|42`, `|*`, `|( )`, `|$x` all die here). The
// identifier lands in [`InSourceIdent`], not the generic [`InIdent`], so a
// bare classpath without a `.all()`/`->` production (`|X `) cannot accept.
fn step_expect_source(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::ExpectSource),
        b if is_ident_start(b) => Step::Next(State::InSourceIdent),
        _ => Step::Dead,
    }
}

// After `{` opened a block query: only the `|` of `{|` (past optional
// whitespace) may follow, so `{X.all()…}` without the pipe is a dead state.
fn step_after_brace_open(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBraceOpen),
        b'|' => Step::Next(State::BlockStmt),
        _ => Step::Dead,
    }
}

// A block-query statement start (`{|` or after a `;`): a `let` binding, a
// pipeline source, or a `$`-var; `BlockStmtClose` additionally admits `}`.
fn step_block_stmt(stack_top: Option<Frame>, byte: u8) -> Step {
    block_stmt(stack_top, byte, false)
}

fn step_block_stmt_close(stack_top: Option<Frame>, byte: u8) -> Step {
    block_stmt(stack_top, byte, true)
}

fn step_expect_value(stack_top: Option<Frame>, byte: u8) -> Step {
    value_position(stack_top, byte, true)
}

fn step_expect_value_req(stack_top: Option<Frame>, byte: u8) -> Step {
    value_position(stack_top, byte, false)
}

fn step_after_value(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterValue),
        b'-' => Step::Next(State::SawDash),
        b'>' => Step::Next(State::SawGt),
        b'<' => Step::Next(State::SawLt),
        b'=' => Step::Next(State::SawEq),
        b'!' => Step::Next(State::SawBang),
        b'&' => Step::Next(State::SawAmp),
        b'|' => Step::Next(State::SawPipe),
        // Binary arithmetic: an operand is required, so a closer cannot follow.
        b'+' | b'*' | b'/' => Step::Next(State::ExpectValueReq),
        b'.' => Step::Next(State::AfterDot),
        b':' => Step::Next(State::AfterColon),
        b'(' => Step::Push(Frame::Paren, State::ExpectValue),
        b'[' => Step::Push(Frame::Bracket, State::ExpectValue),
        // A `,` separates list/argument elements: the next element is required
        // (no trailing `(a,)`).
        b',' if stack_top.is_some() => Step::Next(State::ExpectValueReq),
        // A `;` ends a block-query statement; the next `let` binding or the final
        // pipeline follows, but the block may also close immediately (`;}`), so
        // [`BlockStmtClose`] admits both a fresh statement and the trailing `}`.
        b';' if stack_top == Some(Frame::Brace) => Step::Next(State::BlockStmtClose),
        b')' | b']' | b'}' => close(stack_top, byte),
        _ => Step::Dead,
    }
}

fn step_in_ident(stack_top: Option<Frame>, byte: u8) -> Step {
    if is_ident_tail(byte) {
        Step::Next(State::InIdent)
    } else {
        step(State::AfterValue, stack_top, byte)
    }
}

// A pipeline source classpath. Unlike [`InIdent`], a source is not yet a
// completed value: it must be navigated by a `.` (routing into `AfterDot` —
// `.all()`, a property/getter, or a quoted member `X.'name'`), produced by an
// arm-A `->tableReference(…)` envelope (`->`), or continue across a `::`
// classpath separator. Anything else — whitespace, a closer, an operator — is
// a dead state, so a bare `|X ` never reaches an accepting configuration.
fn step_in_source_ident(byte: u8) -> Step {
    match byte {
        b if is_ident_tail(b) => Step::Next(State::InSourceIdent),
        // A source dot (`X.all()`, `X.'name'`) admits the same set as a value
        // navigation dot (ws / identifier / quoted string), so it shares
        // `AfterDot` — the Legend grammar draws no distinction.
        b'.' => Step::Next(State::AfterDot),
        b'-' => Step::Next(State::SourceDash),
        b':' => Step::Next(State::SourceColon),
        _ => Step::Dead,
    }
}

// A source-classpath `::` separator: the second `:` must follow immediately,
// and then an identifier, keeping the source in its own state across the
// whole classpath (`spider::geo::Db`).
fn step_source_colon(byte: u8) -> Step {
    if byte == b':' {
        Step::Next(State::SourceColon2)
    } else {
        Step::Dead
    }
}

fn step_source_colon2(byte: u8) -> Step {
    if is_ident_start(byte) {
        Step::Next(State::InSourceIdent)
    } else {
        Step::Dead
    }
}

// A `-` in source position is only ever the start of `->`; a source is never
// the left operand of arithmetic minus, so anything but `>` is a dead state.
fn step_source_dash(byte: u8) -> Step {
    if byte == b'>' {
        Step::Next(State::AfterArrow)
    } else {
        Step::Dead
    }
}

// `let`-keyword recognition at a block-statement start. Each byte either
// advances the keyword or, on any divergence, falls back to a source
// classpath that merely shares the prefix (`letters`, `let.foo`). The
// keyword is confirmed only by the whitespace that must separate it from the
// binder name (`let m = …`).
fn step_let_l(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'e' {
        Step::Next(State::LetLe)
    } else {
        step(State::InSourceIdent, stack_top, byte)
    }
}

fn step_let_le(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b't' {
        Step::Next(State::LetLet)
    } else {
        step(State::InSourceIdent, stack_top, byte)
    }
}

fn step_let_let(stack_top: Option<Frame>, byte: u8) -> Step {
    if is_ws(byte) {
        Step::Next(State::ExpectBinder)
    } else {
        step(State::InSourceIdent, stack_top, byte)
    }
}

// `let` seen: the binder name identifier, then the single `=` that opens the
// right-hand-side pipeline. A second bare name (`let m n =`) is a dead state.
fn step_expect_binder(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::ExpectBinder),
        b if is_ident_start(b) => Step::Next(State::InBinder),
        _ => Step::Dead,
    }
}

fn step_in_binder(byte: u8) -> Step {
    match byte {
        b if is_ident_tail(b) => Step::Next(State::InBinder),
        b if is_ws(b) => Step::Next(State::AfterBinder),
        b'=' => Step::Next(State::ExpectSource),
        _ => Step::Dead,
    }
}

fn step_after_binder(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBinder),
        b'=' => Step::Next(State::ExpectSource),
        _ => Step::Dead,
    }
}

// `[*]` multiplicity: only the closing `]` may follow the `*`.
fn step_in_multiplicity(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b']' {
        close(stack_top, byte)
    } else {
        Step::Dead
    }
}

// A `join` brace lambda must begin with a typed binder identifier
// (`{r1: …[1], … | body}`); a literal, digit, or opener body (`{1}`) is a
// dead state.
//
// ponytail (L1 residual, §5.6): the binder is only required to *start* with
// an identifier — a lambda missing its `|` body (`{r1: T[1]}`) or with an
// untyped binder still streams. Fully requiring the `binder(s) | body` shape
// needs per-frame phase tracking the byte machine deliberately omits; the
// compiler re-catches a bodyless join lambda, so it stays an L1 escape.
fn step_expect_brace_binder(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::ExpectBraceBinder),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        _ => Step::Dead,
    }
}

fn step_saw_num_sign(byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InNumberInt)
    } else if byte == b'.' {
        // A signed leading-dot float (`-.5`).
        Step::Next(State::NeedFracDigit)
    } else {
        Step::Dead
    }
}

fn step_in_number_int(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if b.is_ascii_digit() => Step::Next(State::InNumberInt),
        b'.' => Step::Next(State::NeedFracDigit),
        _ => step(State::AfterValue, stack_top, byte),
    }
}

fn step_need_frac_digit(byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InNumberFrac)
    } else {
        Step::Dead
    }
}

fn step_in_number_frac(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if b.is_ascii_digit() => Step::Next(State::InNumberFrac),
        // Scientific notation: an exponent is only legal *after* a fractional
        // part (`1.5e3`), never after a bare integer (`1e3`, which the engine
        // reads as an element reference, not a number).
        b'e' | b'E' => Step::Next(State::SawExp),
        _ => step(State::AfterValue, stack_top, byte),
    }
}

fn step_saw_exp(byte: u8) -> Step {
    match byte {
        b'+' | b'-' => Step::Next(State::NeedExpDigit),
        b if b.is_ascii_digit() => Step::Next(State::InExp),
        _ => Step::Dead,
    }
}

fn step_need_exp_digit(byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InExp)
    } else {
        Step::Dead
    }
}

fn step_in_exp(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InExp)
    } else {
        step(State::AfterValue, stack_top, byte)
    }
}

fn step_in_str_lit(escaped: bool, stack_top: Option<Frame>, byte: u8) -> Step {
    if escaped {
        // The previous byte was a `'`. A second `'` is a doubled quote
        // (stay in the body); anything else means the string already
        // closed, so re-dispatch this byte from `AfterValue`.
        if byte == b'\'' {
            Step::Next(State::InStrLit { escaped: false })
        } else {
            step(State::AfterValue, stack_top, byte)
        }
    } else if byte == b'\'' {
        Step::Next(State::InStrLit { escaped: true })
    } else {
        Step::Next(State::InStrLit { escaped: false })
    }
}

fn step_saw_percent(byte: u8) -> Step {
    if is_date_char(byte) {
        Step::Next(State::InDateLit)
    } else if byte.is_ascii_lowercase() {
        Step::Next(State::InMilestoneLit)
    } else {
        Step::Dead
    }
}

fn step_in_date_lit(stack_top: Option<Frame>, byte: u8) -> Step {
    // A `.` inside a date literal is the fractional-seconds separator
    // (`%2020-01-01T10:00:00.000`). Kept out of `is_date_char` so `%.` at the
    // sigil is still a dead state; admitted only once a date run is underway,
    // matching the existing flat-run over-approximation of date syntax.
    if is_date_char(byte) || byte == b'.' {
        Step::Next(State::InDateLit)
    } else {
        step(State::AfterValue, stack_top, byte)
    }
}

fn step_in_milestone_lit(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte.is_ascii_lowercase() {
        Step::Next(State::InMilestoneLit)
    } else {
        step(State::AfterValue, stack_top, byte)
    }
}

fn step_after_dollar(byte: u8) -> Step {
    if is_ident_start(byte) {
        Step::Next(State::InIdent)
    } else {
        Step::Dead
    }
}

fn step_after_dot(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterDot),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        // A quoted member/column name (`$x.'Gross Credits'`): a relation column
        // whose name is not a bare identifier. Reuse the string-literal body
        // (`''` doubling, §5.5); it closes into `AfterValue`, so the quoted
        // member behaves as a completed navigation value.
        b'\'' => Step::Next(State::InStrLit { escaped: false }),
        _ => Step::Dead,
    }
}

fn step_after_arrow(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterArrow),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        _ => Step::Dead,
    }
}

fn step_after_colon(byte: u8) -> Step {
    match byte {
        // Whitespace after the first `:` splits off into [`AfterColonWs`], where a
        // second `:` is no longer legal — `::` must be contiguous, so `meta: :pure`
        // dies while the typed binder `row: Type` still streams.
        b if is_ws(b) => Step::Next(State::AfterColonWs),
        b':' => Step::Next(State::AfterColon2),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        // An arm-R relation aggregate binds a column name to a lambda after a
        // `:` (`colName : {p,w,r|…}` window frame, `~[agg:{…}:…]`); the `{`
        // opens a brace lambda exactly as it does in value position.
        b'{' => Step::Push(Frame::BraceLambda, State::ExpectBraceBinder),
        _ => Step::Dead,
    }
}

fn step_after_colon_ws(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterColonWs),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        b'{' => Step::Push(Frame::BraceLambda, State::ExpectBraceBinder),
        _ => Step::Dead,
    }
}

fn step_after_colon2(byte: u8) -> Step {
    if is_ident_start(byte) {
        Step::Next(State::InIdent)
    } else {
        Step::Dead
    }
}

fn step_saw_dash(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'>' {
        Step::Next(State::AfterArrow)
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

fn step_saw_pipe(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'|' {
        Step::Next(State::ExpectValueReq)
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

fn step_saw_eq(byte: u8) -> Step {
    if byte == b'=' {
        Step::Next(State::ExpectValueReq)
    } else {
        Step::Dead
    }
}

fn step_saw_bang(byte: u8) -> Step {
    if byte == b'=' {
        Step::Next(State::ExpectValueReq)
    } else {
        Step::Dead
    }
}

fn step_saw_gt(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'=' {
        Step::Next(State::ExpectValueReq)
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

fn step_saw_lt(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'=' {
        Step::Next(State::ExpectValueReq)
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

fn step_saw_amp(byte: u8) -> Step {
    if byte == b'&' {
        Step::Next(State::ExpectValueReq)
    } else {
        Step::Dead
    }
}

fn step_saw_tilde(byte: u8) -> Step {
    match byte {
        b'[' => Step::Push(Frame::Bracket, State::ExpectValue),
        b'\'' => Step::Next(State::InStrLit { escaped: false }),
        b if is_ident_start(b) => Step::Next(State::InIdent),
        _ => Step::Dead,
    }
}

/// The pure transition function: given the current `state`, the `stack_top`
/// frame (if any), and the next `byte`, return the [`Step`] to take.
///
/// Pure and total — the same inputs always yield the same [`Step`], with no side
/// effects. Multi-byte operators are handled by delegating an already-consumed
/// first byte's continuation back into the hub state it belongs to (a tail call
/// to `step` itself), which is why this reads a stream one byte at a time with no
/// look-ahead.
#[must_use]
pub fn step(state: State, stack_top: Option<Frame>, byte: u8) -> Step {
    match state {
        State::Start => step_start(byte),
        State::ExpectSource => step_expect_source(byte),
        State::AfterBraceOpen => step_after_brace_open(byte),
        State::BlockStmt => step_block_stmt(stack_top, byte),
        State::BlockStmtClose => step_block_stmt_close(stack_top, byte),
        State::InSourceIdent => step_in_source_ident(byte),
        State::SourceColon => step_source_colon(byte),
        State::SourceColon2 => step_source_colon2(byte),
        State::SourceDash => step_source_dash(byte),
        State::LetL => step_let_l(stack_top, byte),
        State::LetLe => step_let_le(stack_top, byte),
        State::LetLet => step_let_let(stack_top, byte),
        State::ExpectBinder => step_expect_binder(byte),
        State::InBinder => step_in_binder(byte),
        State::AfterBinder => step_after_binder(byte),
        State::InMultiplicity => step_in_multiplicity(stack_top, byte),
        State::ExpectBraceBinder => step_expect_brace_binder(byte),
        State::AfterColonWs => step_after_colon_ws(byte),
        State::ExpectValue => step_expect_value(stack_top, byte),
        State::ExpectValueReq => step_expect_value_req(stack_top, byte),
        State::AfterValue => step_after_value(stack_top, byte),
        State::InIdent => step_in_ident(stack_top, byte),
        State::SawNumSign => step_saw_num_sign(byte),
        State::InNumberInt => step_in_number_int(stack_top, byte),
        State::NeedFracDigit => step_need_frac_digit(byte),
        State::InNumberFrac => step_in_number_frac(stack_top, byte),
        State::SawExp => step_saw_exp(byte),
        State::NeedExpDigit => step_need_exp_digit(byte),
        State::InExp => step_in_exp(stack_top, byte),
        State::InStrLit { escaped } => step_in_str_lit(escaped, stack_top, byte),
        State::SawPercent => step_saw_percent(byte),
        State::InDateLit => step_in_date_lit(stack_top, byte),
        State::InMilestoneLit => step_in_milestone_lit(stack_top, byte),
        State::AfterDollar => step_after_dollar(byte),
        State::AfterDot => step_after_dot(byte),
        State::AfterArrow => step_after_arrow(byte),
        State::AfterColon => step_after_colon(byte),
        State::AfterColon2 => step_after_colon2(byte),
        State::SawDash => step_saw_dash(stack_top, byte),
        State::SawPipe => step_saw_pipe(stack_top, byte),
        State::SawEq => step_saw_eq(byte),
        State::SawBang => step_saw_bang(byte),
        State::SawGt => step_saw_gt(stack_top, byte),
        State::SawLt => step_saw_lt(stack_top, byte),
        State::SawAmp => step_saw_amp(byte),
        State::SawTilde => step_saw_tilde(byte),
    }
}

/// The outcome of a [`Pda::probe`]: whether a candidate token's bytes keep the
/// automaton alive, and whether deciding that consulted the ambient stack.
///
/// `consulted_ambient` is the exact context-dependence classifier the mask cache
/// keys on (§4.2, Decision D5): it is `true` iff the probe died at a byte whose
/// admissibility would have *differed* had a frame sat beneath the token's own
/// (empty) local scratch — a bare closer `)]}` , or a `,`/`;`/`*` that needs an
/// enclosing frame. Such a token cannot be resolved from state alone and is
/// deferred to a per-step re-probe against the live stack. A token that stays
/// alive against an empty scratch is, by construction, context-*independent*:
/// every stack read it made was satisfied by a frame it had itself pushed, so no
/// ambient stack can change its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    /// Whether every byte was accepted (the automaton never died).
    pub alive: bool,
    /// Whether the verdict depended on the ambient (pre-existing) stack.
    pub consulted_ambient: bool,
}

/// The mutable driver over [`step`]: a current [`State`] and a [`Frame`] stack.
///
/// [`Pda`] owns no offset counter and reports no errors of its own — that is the
/// job of the [`DecoderSession`](crate::DecoderSession) that wraps it. It only
/// applies each [`Step`] and answers whether the stream so far is in an accepting
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pda {
    state: State,
    stack: Vec<Frame>,
}

impl Default for Pda {
    fn default() -> Self {
        Self::new()
    }
}

impl Pda {
    /// A fresh automaton positioned at [`State::Start`] with an empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Start,
            stack: Vec::new(),
        }
    }

    /// Feed one `byte`, advancing the state and stack.
    ///
    /// # Errors
    /// Returns [`DeadState`] — the automaton's `state` name and `stack_top` name
    /// at the point of rejection — when `byte` has no valid continuation. The
    /// automaton is left unchanged on error, so a caller may inspect it.
    pub fn advance(&mut self, byte: u8) -> Result<(), DeadState> {
        let top = self.stack.last().copied();
        match step(self.state, top, byte) {
            Step::Next(next) => {
                self.state = next;
                Ok(())
            }
            Step::Push(frame, next) => {
                self.stack.push(frame);
                self.state = next;
                Ok(())
            }
            Step::Pop(next) => {
                // `step` returns `Pop` only when the closer matched `top`, so the
                // stack is non-empty here.
                self.stack.pop();
                self.state = next;
                Ok(())
            }
            Step::Dead => Err(DeadState {
                state: self.state.name(),
                stack_top: top.map_or("none", Frame::name),
            }),
        }
    }

    /// Whether the stream so far is a complete query: **every frame closed AND
    /// the last token fully lexed at a value boundary**.
    ///
    /// Terminality is derived from the single source of truth [`step`], not a
    /// hand-maintained list: a configuration is accepting iff its stack is empty
    /// and feeding a value-boundary byte (`VALUE_BOUNDARY`, a space) from the
    /// current state lands in
    /// [`State::AfterValue`]. That auto-includes every value-terminal lexical
    /// state — [`AfterValue`](State::AfterValue) itself and the *closed-token*
    /// states [`InIdent`](State::InIdent), [`InNumberInt`](State::InNumberInt),
    /// [`InNumberFrac`](State::InNumberFrac), [`InDateLit`](State::InDateLit), and
    /// a closed string ([`InStrLit { escaped: true }`](State::InStrLit)) — and
    /// auto-excludes the rest: [`InSourceIdent`](State::InSourceIdent) (a bare
    /// `|X` source is *not* a completed value, by design), an open string
    /// ([`InStrLit { escaped: false }`](State::InStrLit)),
    /// [`InMultiplicity`](State::InMultiplicity), and the value hubs
    /// ([`ExpectValue`](State::ExpectValue)/[`ExpectValueReq`](State::ExpectValueReq)),
    /// which stay non-accepting.
    ///
    /// The rule reads [`step`] but never mutates it, so it can only ever *add*
    /// accepting configurations, never turn a live byte dead or clear a mask
    /// bit — gold soundness is unaffected (every gold query ends in `)` →
    /// [`AfterValue`](State::AfterValue), still accepting). Because the
    /// empty-stack guard holds, the only newly-reachable completion is a trailing
    /// top-level identifier (`|X.all()->name`); a top-level number/string/date
    /// never sits over an empty stack, so those stay non-accepting in practice.
    #[must_use]
    pub fn is_accepting(&self) -> bool {
        self.stack.is_empty()
            && matches!(
                step(self.state, None, VALUE_BOUNDARY),
                Step::Next(State::AfterValue)
            )
    }

    /// Reset to the initial configuration, retaining the stack's allocation
    /// (§9.1) for reuse across streams.
    pub fn reset(&mut self) {
        self.state = State::Start;
        self.stack.clear();
    }

    /// A PDA pinned at `state` with an **empty** stack — the base configuration
    /// the mask cache probes each candidate token from when it builds a state's
    /// context-independent survivor set (§4.2).
    #[must_use]
    pub fn at(state: State) -> Self {
        Self {
            state,
            stack: Vec::new(),
        }
    }

    /// The current automaton state — the key a per-state mask cache indexes by.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// The frame on top of the stack, or `None` for an empty stack.
    #[must_use]
    pub fn stack_top(&self) -> Option<Frame> {
        self.stack.last().copied()
    }

    /// The whole frame stack, bottom-to-top — the seed the L2 scope tracker's
    /// lexeme-boundary walk re-drives [`step`] over so an interior closer inside a
    /// merged token routes through the matching frame (a `)` needs its `Paren`).
    /// Read-only: the walk clones it into a scratch, never touching the live PDA.
    pub(crate) fn stack(&self) -> &[Frame] {
        &self.stack
    }

    /// Whether replaying `bytes` from the live configuration keeps the automaton
    /// alive, reusing `scratch` as the throwaway stack so no per-call heap
    /// allocation is needed. This is the per-step hot path (§4.3): it re-probes a
    /// deferred token against the *live* stack and, unlike [`probe`](Pda::probe),
    /// skips the context-dependence classification the build-time partition needs.
    #[must_use]
    pub fn admits(&self, bytes: &[u8], scratch: &mut Vec<Frame>) -> bool {
        scratch.clear();
        scratch.extend_from_slice(&self.stack);
        let mut state = self.state;
        for &byte in bytes {
            let top = scratch.last().copied();
            match step(state, top, byte) {
                Step::Next(next) => state = next,
                Step::Push(frame, next) => {
                    scratch.push(frame);
                    state = next;
                }
                // `step` yields `Pop` only when `top` matched the closer, so the
                // scratch is non-empty here.
                Step::Pop(next) => {
                    scratch.pop();
                    state = next;
                }
                Step::Dead => return false,
            }
        }
        true
    }

    /// Replay `bytes` over [`step`] without touching the live automaton, also
    /// classifying whether the verdict consulted the ambient stack — the
    /// build-time partition step (§4.2). `scratch` is reused (its prior contents
    /// discarded); seeding it from a [`Pda::at`] base (empty stack) is what
    /// exposes context dependence through [`Probe::consulted_ambient`]. The hot
    /// per-step path uses the leaner [`admits`](Pda::admits) instead.
    #[must_use]
    pub fn probe(&self, bytes: &[u8], scratch: &mut Vec<Frame>) -> Probe {
        scratch.clear();
        scratch.extend_from_slice(&self.stack);
        let mut state = self.state;
        for &byte in bytes {
            let top = scratch.last().copied();
            match step(state, top, byte) {
                Step::Next(next) => state = next,
                Step::Push(frame, next) => {
                    scratch.push(frame);
                    state = next;
                }
                Step::Pop(next) => {
                    scratch.pop();
                    state = next;
                }
                Step::Dead => {
                    // The byte died against the local scratch. If the scratch is
                    // empty and *some* enclosing frame would have kept the byte
                    // alive (a matched closer, or a `,`/`;`/`*` that needs a
                    // frame), the verdict is stack-dependent — defer it.
                    let consulted_ambient = scratch.is_empty()
                        && ALL_FRAMES
                            .iter()
                            .any(|&f| !matches!(step(state, Some(f), byte), Step::Dead));
                    return Probe {
                        alive: false,
                        consulted_ambient,
                    };
                }
            }
        }
        Probe {
            alive: true,
            consulted_ambient: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque, hash_map::Entry};

    use super::{
        ALL_FRAMES, Frame, LexKind, Pda, State, Step, WS, is_date_char, is_ident_start,
        is_ident_tail, step,
    };

    /// The deepest stack included in the bounded reachability regression.
    ///
    /// Three levels exercise genuine nesting while capping the theoretical graph
    /// at 3,995 configurations. Pushes beyond this depth are deliberately omitted:
    /// this is a bounded witness check, not a proof of unbounded reachability.
    const MAX_STACK_DEPTH: usize = 3;

    /// Empty stack plus every registered frame as a possible top.
    const TOP_COUNT: usize = ALL_FRAMES.len() + 1;

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    struct Config {
        state: State,
        stack: Vec<usize>,
    }

    type Predecessors = HashMap<Config, Option<(Config, u8)>>;

    struct Exploration {
        predecessors: Predecessors,
        state_witnesses: Vec<Option<Config>>,
        pushed_frames: [bool; ALL_FRAMES.len()],
        black_holes: Vec<Config>,
    }

    /// Every distinct automaton state, for the `index`/`COUNT` bijection check.
    /// [`State::index`]'s exhaustive match already makes a new variant a compile
    /// error; this list makes an index *collision or gap* a test failure too.
    const ALL_STATES: [State; State::COUNT] = [
        State::Start,
        State::ExpectSource,
        State::AfterBraceOpen,
        State::BlockStmt,
        State::BlockStmtClose,
        State::InSourceIdent,
        State::SourceColon,
        State::SourceColon2,
        State::SourceDash,
        State::LetL,
        State::LetLe,
        State::LetLet,
        State::ExpectBinder,
        State::InBinder,
        State::AfterBinder,
        State::InMultiplicity,
        State::ExpectBraceBinder,
        State::AfterColonWs,
        State::ExpectValue,
        State::ExpectValueReq,
        State::AfterValue,
        State::InIdent,
        State::SawNumSign,
        State::InNumberInt,
        State::NeedFracDigit,
        State::InNumberFrac,
        State::SawExp,
        State::NeedExpDigit,
        State::InExp,
        State::InStrLit { escaped: false },
        State::InStrLit { escaped: true },
        State::SawPercent,
        State::InDateLit,
        State::InMilestoneLit,
        State::AfterDollar,
        State::AfterDot,
        State::AfterArrow,
        State::AfterColon,
        State::AfterColon2,
        State::SawDash,
        State::SawPipe,
        State::SawEq,
        State::SawBang,
        State::SawGt,
        State::SawLt,
        State::SawAmp,
        State::SawTilde,
    ];

    fn seed_config() -> Config {
        Config {
            state: State::Start,
            stack: Vec::new(),
        }
    }

    fn registered_frame_index(frame: Frame) -> usize {
        ALL_FRAMES
            .iter()
            .position(|&registered| registered == frame)
            .expect("step pushed a frame missing from ALL_FRAMES")
    }

    fn top_frame(config: &Config) -> Option<Frame> {
        config.stack.last().map(|&index| ALL_FRAMES[index])
    }

    fn top_from_code(code: usize) -> Option<Frame> {
        if code == 0 {
            None
        } else {
            Some(ALL_FRAMES[code - 1])
        }
    }

    fn unique_live_transitions(state: State, top: Option<Frame>) -> Vec<(u8, Step)> {
        let mut transitions = Vec::new();
        for byte in 0..=u8::MAX {
            let transition = step(state, top, byte);
            if matches!(transition, Step::Dead)
                || transitions
                    .iter()
                    .any(|&(_, existing)| existing == transition)
            {
                continue;
            }
            transitions.push((byte, transition));
        }
        transitions
    }

    fn transition_rows() -> Vec<Vec<(u8, Step)>> {
        let mut rows = vec![Vec::new(); State::COUNT * TOP_COUNT];
        for state in ALL_STATES {
            for top_code in 0..TOP_COUNT {
                let row_index = state.index() * TOP_COUNT + top_code;
                rows[row_index] = unique_live_transitions(state, top_from_code(top_code));
            }
        }
        rows
    }

    fn bounded_successor(config: &Config, transition: Step) -> Option<Config> {
        let mut next = config.clone();
        match transition {
            Step::Dead => return None,
            Step::Next(state) => next.state = state,
            Step::Push(frame, state) => {
                let frame_index = registered_frame_index(frame);
                if next.stack.len() >= MAX_STACK_DEPTH {
                    return None;
                }
                next.stack.push(frame_index);
                next.state = state;
            }
            Step::Pop(state) => {
                assert!(
                    next.stack.pop().is_some(),
                    "step returned Pop for an empty bounded stack"
                );
                next.state = state;
            }
        }
        Some(next)
    }

    fn explore_bounded() -> Exploration {
        let rows = transition_rows();
        let seed = seed_config();
        let mut predecessors = HashMap::from([(seed.clone(), None)]);
        let mut queue = VecDeque::from([seed]);
        let mut state_witnesses = vec![None; State::COUNT];
        let mut pushed_frames = [false; ALL_FRAMES.len()];
        let mut black_holes = Vec::new();

        while let Some(config) = queue.pop_front() {
            if state_witnesses[config.state.index()].is_none() {
                state_witnesses[config.state.index()] = Some(config.clone());
            }
            let top_code = config.stack.last().map_or(0, |index| index + 1);
            let row_index = config.state.index() * TOP_COUNT + top_code;
            let mut has_bounded_successor = false;

            for &(byte, transition) in &rows[row_index] {
                let Some(next) = bounded_successor(&config, transition) else {
                    continue;
                };
                has_bounded_successor = true;
                if let Step::Push(frame, _) = transition {
                    pushed_frames[registered_frame_index(frame)] = true;
                }
                if let Entry::Vacant(entry) = predecessors.entry(next.clone()) {
                    entry.insert(Some((config.clone(), byte)));
                    queue.push_back(next);
                }
            }

            if !has_bounded_successor {
                black_holes.push(config);
            }
        }

        Exploration {
            predecessors,
            state_witnesses,
            pushed_frames,
            black_holes,
        }
    }

    fn witness_bytes(config: &Config, predecessors: &Predecessors) -> Vec<u8> {
        let mut reversed = Vec::new();
        let mut cursor = config;
        loop {
            match predecessors
                .get(cursor)
                .expect("every witness config has a predecessor entry")
            {
                Some((previous, byte)) => {
                    reversed.push(*byte);
                    cursor = previous;
                }
                None => {
                    reversed.reverse();
                    return reversed;
                }
            }
        }
    }

    fn replay_witness(bytes: &[u8]) -> Config {
        let mut config = seed_config();
        for &byte in bytes {
            let transition = step(config.state, top_frame(&config), byte);
            config = bounded_successor(&config, transition)
                .expect("a recorded bounded witness must replay within the cap");
        }
        config
    }

    fn frame_names(stack: &[usize]) -> Vec<&'static str> {
        stack
            .iter()
            .map(|&index| ALL_FRAMES[index].name())
            .collect()
    }

    #[test]
    fn index_is_a_bijection_onto_zero_to_count() {
        let mut seen = [false; State::COUNT];
        for state in ALL_STATES {
            let idx = state.index();
            assert!(idx < State::COUNT, "{} out of range: {idx}", state.name());
            assert!(!seen[idx], "index {idx} used twice (at {})", state.name());
            seen[idx] = true;
        }
        assert!(seen.iter().all(|&hit| hit), "index left a gap in 0..COUNT");
    }

    #[test]
    fn bounded_exploration_reaches_every_state_and_frame_without_black_holes() {
        let exploration = explore_bounded();

        for state in ALL_STATES {
            assert!(
                exploration.state_witnesses[state.index()].is_some(),
                "{} has no witness from Start within stack depth {}",
                state.name(),
                MAX_STACK_DEPTH,
            );
        }

        for config in exploration.state_witnesses.iter().flatten() {
            let witness = witness_bytes(config, &exploration.predecessors);
            assert_eq!(
                replay_witness(&witness),
                *config,
                "recorded witness did not replay to {} with stack {:?}",
                config.state.name(),
                frame_names(&config.stack),
            );
        }

        for (index, frame) in ALL_FRAMES.iter().enumerate() {
            assert!(
                exploration.pushed_frames[index],
                "{} was never pushed within stack depth {}",
                frame.name(),
                MAX_STACK_DEPTH,
            );
        }

        let black_holes: Vec<_> = exploration
            .black_holes
            .iter()
            .map(|config| (config.state.name(), frame_names(&config.stack)))
            .collect();
        assert!(
            black_holes.is_empty(),
            "reached configs without a live in-bound successor: {black_holes:?}",
        );
    }

    #[test]
    fn lexeme_kind_classifies_each_open_lexeme_and_none_elsewhere() {
        // Every state that is *inside* a lexeme reports its class, and every
        // inter-lexeme / structural state reports `None`. Enumerated per state so a
        // dropped match arm (or a replace-with-`None`) in `lexeme_kind` reddens
        // here — the L2 scope accumulator keys its buffering on exactly this map.
        for state in [
            State::InIdent,
            State::InSourceIdent,
            State::InBinder,
            State::SourceColon,
            State::SourceColon2,
        ] {
            assert_eq!(
                state.lexeme_kind(),
                Some(LexKind::Ident),
                "{} is inside an identifier",
                state.name()
            );
        }
        for state in [
            State::SawNumSign,
            State::InNumberInt,
            State::NeedFracDigit,
            State::InNumberFrac,
        ] {
            assert_eq!(
                state.lexeme_kind(),
                Some(LexKind::Number),
                "{} is inside a number",
                state.name()
            );
        }
        for state in [State::SawPercent, State::InDateLit, State::InMilestoneLit] {
            assert_eq!(
                state.lexeme_kind(),
                Some(LexKind::Date),
                "{} is inside a date",
                state.name()
            );
        }
        for state in [
            State::InStrLit { escaped: false },
            State::InStrLit { escaped: true },
        ] {
            assert_eq!(
                state.lexeme_kind(),
                Some(LexKind::Str),
                "an open string is inside a string"
            );
        }
        // A representative spread of non-lexeme states: the hubs, the operator
        // "saw first byte" states, and the separators must all be `None`, or the
        // `_ => None` fallback (and the replace-with-`None` mutant) goes uncaught.
        for state in [
            State::Start,
            State::ExpectValue,
            State::ExpectValueReq,
            State::AfterValue,
            State::AfterDot,
            State::AfterArrow,
            State::AfterColon,
            State::SawDash,
            State::SawTilde,
        ] {
            assert_eq!(
                state.lexeme_kind(),
                None,
                "{} is not inside any lexeme",
                state.name()
            );
        }
    }

    #[test]
    fn at_pins_a_state_over_an_empty_stack() {
        let pda = Pda::at(State::AfterValue);
        assert_eq!(pda.state(), State::AfterValue);
        assert_eq!(pda.stack_top(), None);
    }

    #[test]
    fn state_and_stack_top_track_the_live_automaton() {
        let mut pda = Pda::new();
        assert_eq!(pda.state(), State::Start);
        for &byte in b"|X.all(" {
            pda.advance(byte).expect("live");
        }
        // `(` pushed a Paren, and the machine sits in a value position.
        assert_eq!(pda.stack_top(), Some(Frame::Paren));
        assert_eq!(pda.state(), State::ExpectValue);
    }

    #[test]
    fn probe_leaves_the_live_automaton_untouched() {
        let mut pda = Pda::new();
        for &byte in b"|X.all()->take(1" {
            pda.advance(byte).expect("live");
        }
        let before = (pda.state(), pda.stack_top());
        let mut scratch = Vec::new();
        // A live probe of the matching `)` survives against the real Paren…
        assert!(pda.probe(b")", &mut scratch).alive);
        // …and a mismatched `]` dies — but neither mutates the automaton.
        assert!(!pda.probe(b"]", &mut scratch).alive);
        assert_eq!((pda.state(), pda.stack_top()), before);
    }

    #[test]
    fn probe_flags_a_bare_closer_as_context_dependent() {
        // From `AfterValue` over an empty stack, `)` dies but *would* have lived
        // against a Paren — its verdict is stack-dependent.
        let base = Pda::at(State::AfterValue);
        let mut scratch = Vec::new();
        let probe = base.probe(b")", &mut scratch);
        assert!(!probe.alive);
        assert!(probe.consulted_ambient);
    }

    #[test]
    fn probe_flags_a_separator_as_context_dependent() {
        // `,` needs *some* enclosing frame; over an empty stack it is deferred.
        let base = Pda::at(State::AfterValue);
        let mut scratch = Vec::new();
        assert!(base.probe(b",", &mut scratch).consulted_ambient);
    }

    #[test]
    fn probe_marks_a_state_only_death_as_context_independent() {
        // `.` then a digit dies in `AfterDot` regardless of any ambient frame.
        let base = Pda::at(State::AfterDot);
        let mut scratch = Vec::new();
        let probe = base.probe(b"5", &mut scratch);
        assert!(!probe.alive);
        assert!(!probe.consulted_ambient);
    }

    #[test]
    fn probe_marks_a_survivor_as_context_independent() {
        // An identifier byte lives from `AfterDot` and reads no stack.
        let base = Pda::at(State::AfterDot);
        let mut scratch = Vec::new();
        let probe = base.probe(b"name", &mut scratch);
        assert!(probe.alive);
        assert!(!probe.consulted_ambient);
    }

    /// Drive `bytes` through a fresh [`Pda`], returning it (or the first dead
    /// state) so a test can assert on the terminal configuration.
    fn run(bytes: &[u8]) -> Result<Pda, (usize, &'static str, &'static str)> {
        let mut pda = Pda::new();
        for (offset, &byte) in bytes.iter().enumerate() {
            if let Err(dead) = pda.advance(byte) {
                return Err((offset, dead.state, dead.stack_top));
            }
        }
        Ok(pda)
    }

    fn accepts(text: &str) -> bool {
        matches!(run(text.as_bytes()), Ok(pda) if pda.is_accepting())
    }

    fn dies(text: &str) -> bool {
        run(text.as_bytes()).is_err()
    }

    #[test]
    fn ws_constant_is_the_four_inter_token_spaces() {
        assert_eq!(WS, b" \t\n\r");
    }

    #[test]
    fn char_class_helpers_agree_with_grammar() {
        assert!(is_ident_start(b'a') && is_ident_start(b'_') && is_ident_start(b'Z'));
        assert!(!is_ident_start(b'0') && !is_ident_start(b'$'));
        assert!(is_ident_tail(b'0') && is_ident_tail(b'z') && is_ident_tail(b'_'));
        assert!(!is_ident_tail(b'-'));
        assert!(
            is_date_char(b'0') && is_date_char(b'-') && is_date_char(b'T') && is_date_char(b':')
        );
        assert!(!is_date_char(b'Z'));
    }

    #[test]
    fn after_dot_admits_a_quoted_member_name() {
        // A navigation dot may be followed by a single-quoted member/column name
        // (`$x.'Gross Credits'`), reusing the string-literal body. Engine-verified
        // (gap report response 4).
        assert!(matches!(
            step(State::AfterDot, None, b'\''),
            Step::Next(State::InStrLit { escaped: false })
        ));
        // Whole-query replays: a quoted member streams to an accepting state, its
        // name may hold spaces and doubled-quote escapes, and normal continuations
        // (comparison, chained `->`) follow.
        assert!(accepts("|X.all()->filter(x|$x.'Cnt' > 100)"));
        assert!(accepts("|X.all()->filter(x|$x.'Gross Credits' > 100)"));
        assert!(accepts("|X.all()->filter(x|$x.'a''b' > 0)"));
        assert!(accepts(
            "|X.all()->groupBy(~[k], ~'Cnt': x|$x.v : y|$y->count())->filter(x|$x.'Cnt' > 100)"
        ));
    }

    #[test]
    fn quoted_member_names_support_chained_navigation() {
        // The closed quoted member is a completed value, so a chained `->` call and a
        // further `.` navigation both follow.
        assert!(accepts("|X.all()->filter(x|$x.'Total GC'->toOne() > 0)"));
        assert!(accepts("|X.all()->filter(x|$x.'seg'.name == 'z')"));
    }

    #[test]
    fn quoted_member_names_reject_incomplete_navigation() {
        // An unclosed quote never reaches an accepting state.
        assert!(!accepts("|X.all()->filter(x|$x.'Cnt"));
        // A bare dot with no member is still a dead end.
        assert!(dies("|X.all()->filter(x|$x. > 0)"));
    }

    #[test]
    fn source_dots_admit_quoted_member_names() {
        // A quoted member is legal after a *source* dot too (`|X.'name'` parses on
        // the Legend engine) — the source and value dots share the admit-set, so it
        // must stream, not dead-state.
        assert!(accepts("|X.'name'"));
        assert!(accepts("|X.'name'->all()"));
        assert!(accepts("|demo::Reading.'Cnt'"));
        assert!(accepts("|X.all()"));
        // A dot still requires ws / identifier / quote: a bare digit or operator is a
        // dead end in both positions (the engine rejects `X.5` / `X.-y` too).
        assert!(dies("|X.5"));
        assert!(dies("|X.-y"));
    }

    #[test]
    fn start_admits_only_pipe_or_brace() {
        // A simple query opens with `|` on its source; a block query opens with
        // `{`, awaiting the `|` of `{|`.
        assert!(matches!(
            step(State::Start, None, b'|'),
            Step::Next(State::ExpectSource)
        ));
        assert!(matches!(
            step(State::Start, None, b'{'),
            Step::Push(Frame::Brace, State::AfterBraceOpen)
        ));
        assert!(matches!(step(State::Start, None, b'x'), Step::Dead));
        assert!(matches!(step(State::Start, None, b'('), Step::Dead));
    }

    #[test]
    fn a_top_level_source_must_be_an_identifier() {
        // The pipeline source is always a classpath; a bare literal, `$`-var,
        // star, or parenthesised expression in source position is a dead state.
        assert!(dies("|42 "));
        assert!(dies("|*"));
        assert!(dies("|( )"));
        assert!(dies("|'x'"));
        assert!(dies("|$x"));
        // …but an identifier source opens a real pipeline.
        assert!(accepts("|X.all()->take(1)"));
    }

    #[test]
    fn a_completed_term_is_not_followed_by_a_bare_identifier() {
        // Missing-arrow ident-salad dies: a fresh identifier may not abut a
        // completed term outside a block-query `let` binder.
        assert!(dies("|foo bar baz "));
        assert!(dies("|X.all() take(3)"));
        assert!(dies("|X.all()->take(1) take(2)"));
        // The one legal abutment — `let name` under a block query's brace — lives.
        assert!(accepts("{|let m = X.all()->take(1); $m->take(1);}"));
    }

    #[test]
    fn a_dangling_operator_before_a_closer_dies() {
        assert!(dies("|X.all()->take(1 +)"));
        assert!(dies("|X.all()->filter(x|$x.a && )"));
        assert!(dies("|X.all()->filter(x|$x.a || )"));
    }

    #[test]
    fn malformed_numeric_and_date_literals_die() {
        assert!(dies("|X.all()->take(-)"));
        assert!(dies("|X.all()->take(1.)"));
        assert!(dies("|X.all()->take(--5)"));
        assert!(dies("|X.all()->take(%)"));
        // A bare `.` / a `.` with no fractional digit dies; an exponent needs a digit.
        assert!(dies("|X.all()->filter(x|$x.v > .)"));
        assert!(dies("|X.all()->filter(x|$x.v > 1.5e)"));
        assert!(dies("|X.all()->filter(x|$x.v > 1.5e+)"));
        // …well-formed literals still stream.
        assert!(accepts("|X.all()->take(-5)"));
        assert!(accepts("|X.all()->filter(x|$x.v > 1.5)"));
    }

    #[test]
    fn extended_numeric_and_date_literals_stream() {
        // Engine-verified (Legend 4.113.0) legal literal forms the byte-PDA now
        // admits: leading-dot floats, scientific notation (decimal-point required),
        // and datetime fractional seconds.
        assert!(accepts("|X.all()->filter(x|$x.v > .5)"));
        assert!(accepts("|X.all()->filter(x|$x.v > -.5)"));
        assert!(accepts("|X.all()->filter(x|$x.v == 1.5e3)"));
        assert!(accepts("|X.all()->filter(x|$x.v == 1.5e-3)"));
        assert!(accepts("|X.all()->filter(x|$x.v == 1.5e+3)"));
        assert!(accepts("|X.all()->filter(x|$x.v == 1.5E3)"));
        assert!(accepts(
            "|X.all()->filter(x|$x.t == %2020-01-01T10:00:00.000)"
        ));
        // The engine reads a bare-integer exponent (`1e3`) as an element reference,
        // not a number, so the byte-PDA does NOT admit it as a numeric literal — an
        // exponent is only legal after a fractional part.
        assert!(dies("|X.all()->filter(x|$x.v == 1e3)"));
    }

    #[test]
    fn a_single_equals_is_dead_outside_a_let_binder() {
        // A lone `=` as a comparison operator under a `Paren` dies…
        assert!(dies("|X.all()->filter(x|$x.a = 1)"));
        // …but the `let name =` binder single `=` under a block brace is valid.
        assert!(accepts("{|let m = X.all()->take(1); $m->take(1);}"));
    }

    #[test]
    fn colon_runs_beyond_a_double_colon_die() {
        assert!(dies("|X:::Y.all()->take(1)"));
        // `::` classpath separators and the typed-binder `:` still stream.
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()->limit(1)"
        ));
    }

    #[test]
    fn a_block_query_requires_the_leading_pipe() {
        assert!(dies("{X.all()->take(1)}"));
        assert!(accepts("{|X.all()->take(1);}"));
    }

    #[test]
    fn whitespace_is_skipped_at_the_source_and_block_openers() {
        // Whitespace after the top-level `|`, after the block `{`, and after the
        // block's `{|` is inter-token space and is skipped before the source.
        assert!(accepts("| X.all()->take(1)"));
        assert!(accepts("{ |X.all()->take(1);}"));
        assert!(accepts("{ | X.all()->take(1);}"));
    }

    #[test]
    fn a_classpath_separator_carries_no_interior_whitespace() {
        // A single typed-binder `:` tolerates following whitespace (`row: Type`),
        // but a `::` separator does not (`meta::pure`, never `meta:: pure`).
        assert!(dies("|meta:: pure::Thing.all()->take(1)"));
        // A `:` (single or double) still demands an identifier, not a digit.
        assert!(dies("|X:5.all()->take(1)"));
        assert!(dies("|X::5.all()->take(1)"));
    }

    #[test]
    fn empty_stream_is_not_accepting() {
        assert!(!Pda::new().is_accepting());
        assert!(!accepts(""));
    }

    #[test]
    fn is_accepting_derives_terminality_from_step_per_state() {
        // Value-terminal lexical states over an empty stack accept at EOS: the
        // closed-token states plus the `AfterValue` hub. Enumerated white-box
        // (mirroring `index_is_a_bijection`) so a `step` change that drops a
        // terminal delegation reddens here.
        for terminal in [
            State::AfterValue,
            State::InIdent,
            State::InNumberInt,
            State::InNumberFrac,
            State::InDateLit,
            State::InMilestoneLit,
            State::InStrLit { escaped: true },
        ] {
            assert!(
                Pda::at(terminal).is_accepting(),
                "{} is value-terminal and must accept at EOS",
                terminal.name()
            );
        }
        // Non-terminal states must NOT accept: a bare source (`|X`), the value
        // hubs, an open string, and the `[*]` multiplicity slot. `InSourceIdent`
        // is excluded by design — a bare `|X` source is not a completed value.
        for open in [
            State::InSourceIdent,
            State::ExpectValue,
            State::ExpectValueReq,
            State::InStrLit { escaped: false },
            State::InMultiplicity,
            State::Start,
        ] {
            assert!(
                !Pda::at(open).is_accepting(),
                "{} is not a completed value and must not accept at EOS",
                open.name()
            );
        }
    }

    #[test]
    fn a_frame_still_open_is_never_accepting_even_at_a_terminal_state() {
        // The empty-stack guard is load-bearing: a completed number/ident sitting
        // under an open `(` is mid-query, not a complete stream.
        let mut pda = Pda::new();
        for &byte in b"|X.all()->take(3" {
            pda.advance(byte).expect("live");
        }
        // At `InNumberInt` with a Paren still open — a terminal *state* but a
        // non-empty stack, so not accepting.
        assert_eq!(pda.state(), State::InNumberInt);
        assert!(!pda.is_accepting());
    }

    #[test]
    fn a_trailing_top_level_identifier_completes() {
        // The one newly-reachable completion the EOS widening adds: a top-level
        // step whose last token is a bare identifier (`->name`) with every frame
        // already closed. `InIdent` over an empty stack now accepts.
        assert!(accepts("|X.all()->name"));
        let mut pda = Pda::new();
        for &byte in b"|X.all()->name" {
            pda.advance(byte).expect("live");
        }
        assert_eq!(pda.state(), State::InIdent);
        assert!(pda.stack_top().is_none());
        assert!(pda.is_accepting());
    }

    #[test]
    fn a_bare_source_identifier_never_completes() {
        // `|X` lands in `InSourceIdent`, which is deliberately non-accepting, and
        // a trailing space still dies there (ws → Dead), so a bare source is
        // neither complete nor live.
        let mut pda = Pda::new();
        for &byte in b"|X" {
            pda.advance(byte).expect("live");
        }
        assert_eq!(pda.state(), State::InSourceIdent);
        assert!(!pda.is_accepting());
        assert!(dies("|X "));
    }

    #[test]
    fn arm_c_source_and_project_accepts() {
        assert!(accepts("|X.all()->project([x|$x.name], ['n'])"));
    }

    #[test]
    fn arm_a_envelope_accepts() {
        assert!(accepts(
            "|db::Db->tableReference('default', 'T')->tableToTDS()->limit(5)"
        ));
    }

    #[test]
    fn bracket_context_dependence_rejects_crossed_closers() {
        // `(` opened, `]` cannot close a Paren.
        assert!(dies("|X.all()->take(2]"));
        // `[` opened, `)` cannot close a Bracket.
        assert!(dies("|X.all()->project([x|$x.n)"));
        // A closer with an empty stack is dead.
        assert!(dies("|X.all())"));
    }

    #[test]
    fn matched_nested_brackets_accept() {
        assert!(accepts(
            "|X.all()->groupBy([], [agg(x|$x.v, y|$y->sum())], ['s'])"
        ));
    }

    #[test]
    fn string_quote_doubling_is_consumed_in_body() {
        // The doubled `''` is one embedded quote, not a close-then-reopen.
        assert!(accepts("|X.all()->filter(x|$x.name == 'O''Brien')"));
        // An un-doubled closing quote ends the string; the trailing `)` closes.
        assert!(accepts("|X.all()->restrict('Rank')"));
    }

    #[test]
    fn parens_inside_a_string_do_not_touch_the_stack() {
        // `'COUNT()'` must not push/pop Paren frames.
        assert!(accepts(
            "|db::Db->tableReference('default', 'T')->tableToTDS()\
             ->groupBy([], agg('COUNT()', row: meta::pure::tds::TDSRow[1]|$row, \
             y: meta::pure::tds::TDSRow[*]|$y->count()))"
        ));
    }

    #[test]
    fn whitespace_is_skipped_between_tokens_only() {
        assert!(accepts("|X.all()\n  ->filter( x | $x.age > 18 )"));
        // …but a token is never split: a space inside a number literal leaves a
        // stray digit that `AfterValue` cannot resume.
        assert!(dies("|X.all()->take(1 0)"));
    }

    #[test]
    fn empty_key_group_by_accepts() {
        assert!(accepts(
            "|X.all()->groupBy([], [agg(x|$x.v, y|$y->count())], ['c'])"
        ));
    }

    #[test]
    fn typed_multiplicity_binder_accepts_one_and_star() {
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
        ));
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->groupBy([], agg('C', row: meta::pure::tds::TDSRow[1]|$row, \
             y: meta::pure::tds::TDSRow[*]|$y->count()))"
        ));
    }

    #[test]
    fn brace_multi_binder_join_accepts() {
        assert!(accepts(
            "|a::Db->tableReference('default','A')->tableToTDS()->join(\
             a::Db->tableReference('default','B')->tableToTDS(), \
             meta::relational::metamodel::join::JoinType.INNER, \
             {r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
             $r1.getInteger('x') == $r2.getInteger('y')})"
        ));
    }

    #[test]
    fn dollar_requires_an_identifier() {
        assert!(dies("|X.all()->filter(x|$)"));
        assert!(dies("|X.all()->filter(x|$5 > 1)"));
    }

    #[test]
    fn or_operator_is_distinct_from_the_lambda_pipe() {
        // First `|` is the binder pipe, `||` is boolean OR.
        assert!(accepts("|X.all()->filter(x|($x.a == 1) || ($x.b == 2))"));
    }

    #[test]
    fn bang_is_both_the_not_prefix_and_the_ne_operator() {
        // Unary NOT in value position (after `&&`).
        assert!(accepts(
            "|X.all()->filter(s|($s.a == 0) && !$s.name->in($xs))"
        ));
        // Binary `!=` in operator position (after a value).
        assert!(accepts("|X.all()->filter(x|$x.a != 1)"));
        // A lone `!` not completing `!=` in operator position is dead.
        assert!(dies("|X.all()->filter(x|$x.a ! 1)"));
    }

    #[test]
    fn block_query_with_let_binding_accepts() {
        assert!(accepts(
            "{|let m = X.all().pop->max(); Y.all()->filter(b|$b.v == $m)\
             ->project([x|$x.c], ['c']);}"
        ));
    }

    #[test]
    fn block_let_binder_whitespace_and_boundaries() {
        // `{|` alone cannot close — a block needs at least one statement.
        assert!(dies("{|}"));
        assert!(dies("{| }"));
        // The binder name tolerates extra surrounding whitespace, and `=` may abut it.
        assert!(accepts("{|let  m = X.all()->take(1);}")); // two spaces after `let`
        assert!(accepts("{|let m  = X.all()->take(1);}")); // two spaces before `=`
        assert!(accepts("{|let m=X.all()->take(1);}")); // `=` abuts the name
        // The binder name is an identifier, never a literal or a missing name.
        assert!(dies("{|let 5 = X.all()->take(1);}"));
        assert!(dies("{|let = X.all()->take(1);}"));
    }

    #[test]
    fn typed_binder_colon_whitespace_boundaries() {
        // A binder `:` may abut its type or carry one-or-more spaces before it.
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(row:meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
        ));
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(row:  meta::pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
        ));
        // A `::` separator must be contiguous: a double colon then a space dies.
        assert!(dies(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(row: meta:: pure::tds::TDSRow[1]|$row.getInteger('c') == 1)"
        ));
        // A binder `:` demands an identifier type, never a bare digit.
        assert!(dies(
            "|db::Db->tableReference('default','T')->tableToTDS()->filter(row:5|$row)"
        ));
    }

    #[test]
    fn brace_lambda_tolerates_whitespace_after_the_open() {
        // A space after the `{` is skipped before the required binder identifier.
        assert!(accepts(
            "|a::Db->tableReference('default','A')->tableToTDS()->join(\
             a::Db->tableReference('default','B')->tableToTDS(), \
             meta::relational::metamodel::join::JoinType.INNER, \
             { r1: meta::pure::tds::TDSRow[1], r2: meta::pure::tds::TDSRow[1]|\
             $r1.getInteger('x') == $r2.getInteger('y')})"
        ));
    }

    #[test]
    fn date_literal_operand_accepts() {
        assert!(accepts(
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(r: meta::pure::tds::TDSRow[1]|$r.getDateTime('d') < %2018-03-17T07:13:53)"
        ));
    }

    #[test]
    fn milestoning_literal_operand_accepts() {
        // `%latest` / `%latestdate` are symbolic milestoning literals usable as an
        // `.all(...)` argument, a milestoned `.PROP(...)` argument, and a
        // comparison operand (gap report G2).
        assert!(accepts("|X.all(%latest)->project([p|$p.n], ['n'])"));
        assert!(accepts("|X.all(%latest, %latest)->take(1)"));
        assert!(accepts("|X.all(%latestdate)->take(1)"));
        assert!(accepts(
            "|X.all()->filter(x|$x.FACET(%latest, %latest).seg == 'a')"
        ));
        // A bare `%latest` completes at end-of-stream (value-terminal).
        assert!(accepts("|X.all()->filter(x|$x.d < %latest)"));
    }

    #[test]
    fn a_milestoning_literal_is_lowercase_letters_after_the_percent() {
        // Bare `%` is still a dead state (the existing date-literal pin).
        assert!(dies("|X.all()->take(%)"));
        // The symbolic literal is lowercase letters only: an uppercase or digit
        // first byte after `%` is not a milestone symbol, and mid-literal a digit
        // or uppercase closes the lexeme — so `%latest1`/`%latestX` stop the token
        // at `%latest` and the trailing byte has no legal continuation here.
        assert!(dies("|X.all()->take(%Latest)"));
        assert!(dies("|X.all()->take(%latest1)"));
        assert!(dies("|X.all()->take(%latestX)"));
        // A milestone literal mid-lex (only `%l` so far) is not yet accepting.
        assert!(!accepts("|X.all()->filter(x|$x.d < %l"));
    }

    #[test]
    fn direct_step_covers_the_saw_percent_and_milestone_branches() {
        // `%` + lowercase opens the milestone lexeme; `%` + date char opens the
        // numeric date lexeme; `%` + anything else dies.
        assert!(matches!(
            step(State::SawPercent, None, b'l'),
            Step::Next(State::InMilestoneLit)
        ));
        assert!(matches!(
            step(State::SawPercent, None, b'2'),
            Step::Next(State::InDateLit)
        ));
        assert!(matches!(step(State::SawPercent, None, b'Z'), Step::Dead));
        assert!(matches!(step(State::SawPercent, None, b')'), Step::Dead));
        // The milestone lexeme accretes lowercase letters and delegates any other
        // byte to `AfterValue` (value-terminal), so a following `)` pops a frame.
        assert!(matches!(
            step(State::InMilestoneLit, None, b'a'),
            Step::Next(State::InMilestoneLit)
        ));
        assert!(matches!(
            step(State::InMilestoneLit, Some(Frame::Paren), b')'),
            Step::Pop(State::AfterValue)
        ));
        // A digit mid-literal delegates to `AfterValue`, where a bare digit dies.
        assert!(matches!(
            step(State::InMilestoneLit, None, b'1'),
            Step::Dead
        ));
    }

    #[test]
    fn arm_r_relation_api_accepts() {
        // The Relation/Function API family (gap report G1) — every seed from §4.1.
        assert!(accepts("|X.all()->project(~[Col: x|$x.a])"));
        assert!(accepts("|X.all()->project(~[A: x|$x.a, B: x|$x.b.c])"));
        assert!(accepts(
            "|X.all()->groupBy(~[K], ~'Agg': x|$x.v : y|$y->sum())"
        ));
        // Empty relation key `~[]` (aggregate-over-all), mirroring empty `[]`.
        assert!(accepts(
            "|X.all()->groupBy(~[], ~'Total': x|$x.v : y|$y->count())"
        ));
        assert!(accepts("|X.all()->sort([ascending(~A)])"));
        assert!(accepts("|X.all()->sort([ascending(~A), descending(~B)])"));
        assert!(accepts("|X.all()->rename(~old, ~new)"));
        // Window extend: `over(~…)` partition and a `{p,w,r|…}` frame lambda after a
        // spaced `agg: {…}` colon.
        assert!(accepts(
            "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg: {p,w,r|$r.v} : y|$y->sum()])"
        ));
        // …and the un-spaced `agg:{…}:y` colon form (a `{`/`y` right after `:`).
        assert!(accepts(
            "|X.all()->project(~[N: x|$x.a])->extend(over(~N), ~[agg:{p,w,r|$r.v}:y|$y->sum()])"
        ));
        // A full chain: project → grouped agg → sort, all arm-R.
        assert!(accepts(
            "|X.all()->project(~[W: x|$x.a])->groupBy(~[W], ~'S': x|$x.g : y|$y->sum())->sort([ascending(~W)])"
        ));
        // A quoted column name with spaces (`~'Gross Credits'`).
        assert!(accepts(
            "|X.all()->groupBy(~[Week], ~'Gross Credits': x|$x.g : y|$y->sum())"
        ));
    }

    #[test]
    fn a_tilde_sigil_must_be_followed_by_a_column_set_or_reference() {
        // `~` opens `~[`, `~ident`, or `~'str'`; nothing else — not whitespace, not
        // a closer, not another `~` — may follow.
        assert!(dies("|X.all()->project(~)"));
        assert!(dies("|X.all()->project(~ [Col: x|$x.a])"));
        assert!(dies("|X.all()->project(~~)"));
        assert!(dies("|X.all()->sort([ascending(~)])"));
        // A `~` is not a legal pipeline source.
        assert!(dies("|~.all()"));
    }

    #[test]
    fn saw_tilde_dispatches_column_sets_and_references() {
        assert!(matches!(
            step(State::SawTilde, None, b'['),
            Step::Push(Frame::Bracket, State::ExpectValue)
        ));
        assert!(matches!(
            step(State::SawTilde, None, b'\''),
            Step::Next(State::InStrLit { escaped: false })
        ));
        assert!(matches!(
            step(State::SawTilde, None, b'W'),
            Step::Next(State::InIdent)
        ));
        assert!(matches!(step(State::SawTilde, None, b' '), Step::Dead));
        assert!(matches!(step(State::SawTilde, None, b')'), Step::Dead));
    }

    #[test]
    fn arm_r_value_and_colon_states_dispatch() {
        // A `~` in value position opens the sigil state.
        assert!(matches!(
            step(State::ExpectValue, Some(Frame::Paren), b'~'),
            Step::Next(State::SawTilde)
        ));
        // A `{` after a typed/relation colon opens a brace lambda.
        assert!(matches!(
            step(State::AfterColon, Some(Frame::Bracket), b'{'),
            Step::Push(Frame::BraceLambda, State::ExpectBraceBinder)
        ));
        assert!(matches!(
            step(State::AfterColonWs, Some(Frame::Bracket), b'{'),
            Step::Push(Frame::BraceLambda, State::ExpectBraceBinder)
        ));
    }

    #[test]
    fn unterminated_string_or_open_paren_is_not_accepting() {
        assert!(!accepts("|X.all()->restrict('Rank"));
        assert!(!accepts("|X.all()->take(2"));
    }

    #[test]
    fn reset_returns_to_the_initial_configuration() {
        let mut pda = Pda::new();
        for &byte in b"|X.all()->take(2)" {
            pda.advance(byte).expect("live");
        }
        assert!(pda.is_accepting());
        pda.reset();
        assert!(!pda.is_accepting());
        assert!(pda.advance(b'x').is_err());
    }

    #[test]
    fn start_skips_leading_whitespace_before_the_opener() {
        assert!(accepts(" \n\t|X.all()->take(1)"));
    }

    #[test]
    fn top_level_separators_require_an_open_frame() {
        // A `,` or `;` is legal only inside a frame; with an empty stack it dies.
        assert!(dies("|X.all(),"));
        assert!(dies("|X.all();"));
    }

    #[test]
    fn dot_arrow_colon_skip_whitespace_then_demand_an_identifier() {
        // Whitespace after `.` / `->` is skipped, then an identifier is required.
        assert!(accepts("|X. all()->take(1)"));
        assert!(accepts("|X.all()->  take(1)"));
        // …and a non-identifier byte in that position is a dead state.
        assert!(dies("|X.all().5"));
        assert!(dies("|X.all()->5"));
        assert!(dies("|X::5"));
    }

    #[test]
    fn a_dead_state_names_its_state_and_top_frame() {
        // `.` then a digit dies in `AfterDot`, with the enclosing stack empty.
        assert_eq!(
            run(b"|X.all().5").expect_err("dies"),
            (9, "AfterDot", "none")
        );
        // A `,` with nothing to separate dies in `ExpectValue` under a `Paren`.
        assert_eq!(
            run(b"|X.all(,").expect_err("dies"),
            (7, "ExpectValue", "Paren")
        );
        // An unmatched closer dies with an empty stack (`none`).
        assert_eq!(
            run(b"|X.all())").expect_err("dies"),
            (8, "AfterValue", "none")
        );
    }
}
