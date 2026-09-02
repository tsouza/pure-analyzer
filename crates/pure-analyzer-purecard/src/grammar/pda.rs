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
    /// Having just completed a term; an operator, separator, or closer may
    /// follow.
    AfterValue,
    /// Having just completed a **term-start identifier** (past any trailing
    /// whitespace) — a word that opened at a value position or continued a `::`
    /// classpath, and so is still a *package path* candidate. Everything
    /// [`AfterValue`](State::AfterValue) admits, plus a call's own `(` and the
    /// `::` that continues a classpath.
    ///
    /// The `(` binds to a name and to nothing else. A call applies a *function*,
    /// named by an identifier — the engine rejects a juxtaposed application off
    /// any other term (live: `|…::ModelList.all()(Float(…))` → "Unexpected token
    /// '('"). A `[…]` after a term is only ever the multiplicity of the type it
    /// annotates (`row: meta::pure::tds::TDSRow[1]`); Legend has no positional
    /// index at all, and says so in as many words (live:
    /// `{|…::ModelList.all()['MPG_T1_1']}` → "Bracket operation is not
    /// supported"). A list literal `[…]` and a relation column set `~[…]` open at
    /// a *value* position, never here, so neither is affected.
    AfterName,
    /// Having just completed a **member name** — an identifier reached through a
    /// `.` navigation, a `->` arrow call, or a `$` variable sigil. Identical to
    /// [`AfterName`](State::AfterName) except that a `::` may not follow: a
    /// classpath segment is never a property, a method, or a variable. Live:
    /// `…->join('x'.meta::pure::tds::TDSRow)`, `…->getInteger::foo` and
    /// `…!=$x.foo::bar` are each "no viable alternative", while
    /// `…!=mpg::getInteger` parses.
    AfterMemberName,
    /// Having just completed a **string literal**. Everything
    /// [`AfterValue`](State::AfterValue) admits, plus the `::` a quoted name may
    /// still carry — live-attested, `…!='europe'::makeId` and `…!='a b'::c` both
    /// parse, where the same `::` off a `)`, a number, a date or a `]` does not.
    /// A call's `(` stays out: an application off a string is a dead state
    /// (§5.6, issue #55 Phase 4).
    AfterStrLit,
    /// Inside a term-start identifier or classpath segment
    /// (`[A-Za-z_][A-Za-z0-9_]*`).
    InIdent,
    /// Inside a **member** identifier — the same byte class as
    /// [`InIdent`](State::InIdent), reached from a `.`, a `->` or a `$` instead
    /// of from a value position, and completing into
    /// [`AfterMemberName`](State::AfterMemberName) rather than
    /// [`AfterName`](State::AfterName).
    InMemberIdent,
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
    /// Inside a `%`-prefixed date literal's **date half**, on a digit and with no
    /// `:` seen yet (`%1974`, `%2018-03-17`). Value-terminal: a date literal ends
    /// on a digit, never on a separator (live: `%2018-`, `%2018-03-17T` and
    /// `%2018-03-17T07:` are each "no viable alternative").
    InDateLit,
    /// Just consumed a `-` or `T` in a date literal's date half; a digit must
    /// follow, so the literal cannot end — or continue — on the separator.
    DateSep,
    /// Inside a date literal's **time half**, on a digit and past at least one
    /// `:` (`%2018-03-17T07:13:53`). This is the only place a `.` may open the
    /// fractional seconds: `%1974.5`, `%0.0` and `%2018-03-17.000` are each dead
    /// against the pinned engine, while `%2018-03-17T07:13:53.000` parses.
    InDateTime,
    /// Just consumed a `-`, `T` or `:` in a date literal's time half; a digit
    /// must follow.
    DateTimeSep,
    /// Just consumed the `.` that opens a date literal's fractional seconds; at
    /// least one digit must follow, so a trailing `%2018-03-17T07:13:53.` dies.
    DateFrac,
    /// Inside a date literal's fractional-seconds digits — the literal's last
    /// field, so only more digits may follow (live: a second `.` is dead,
    /// `%2018-03-17T07:13:53.000.111` → "no viable alternative").
    InDateFrac,
    /// Consumed `%l`, the first byte of the engine's one symbolic milestoning
    /// literal. The chain spells `MILESTONE_LATEST` one state per byte, exactly
    /// as [`LetL`](State::LetL)/[`LetLe`](State::LetLe)/[`LetLet`](State::LetLet)
    /// spell the `let` keyword: a milestone symbol is a *keyword*, not an open
    /// lowercase run.
    MilestoneL,
    /// Consumed `%la`.
    MilestoneLa,
    /// Consumed `%lat`.
    MilestoneLat,
    /// Consumed `%late`.
    MilestoneLate,
    /// Consumed `%lates`.
    MilestoneLates,
    /// Completed the symbolic milestoning literal `%latest`. Distinct from
    /// [`InDateLit`](State::InDateLit) so a milestone symbol and a numeric date
    /// literal never share a byte class; value-terminal like `InDateLit`, which is
    /// what makes the trailing `date` of `%latestdate` a dead state.
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
    /// Just consumed a single `:` off a **name or a string literal** — either a
    /// typed-binder colon (`row: …[1]`) or the first `:` of a `::` classpath
    /// separator; a classpath identifier or a second `:` must follow.
    AfterColon,
    /// Just consumed a single `:` off **any other completed term**. Identical to
    /// [`AfterColon`](State::AfterColon) except that the `::` classpath separator
    /// is not among its continuations: a package path is spelled from a bare word
    /// or a quoted one, never off a call's `)`, a `]`, a number, a date, a
    /// `$`-variable or a navigated member. Live-attested both ways —
    /// `…!=mpg::getInteger`, `…!=meta::pure::tds::TDSRow` and
    /// `…!='europe'::makeId` parse, while `…!=f()::a`, `…!=[1]::a`, `…!=1::a`,
    /// `…!=$x::a`, `…!=$x.foo::a` and `…!=x->getInteger()::a` are each "no viable
    /// alternative at input '…::'". The typed-binder arms stay, because arm-R's
    /// second column colon legitimately follows a completed navigation
    /// (`~'Agg': x|$x.v : y|$y->sum()`).
    ///
    /// Those binder arms are all this state has left, so it is only *entered*
    /// where one of them can fire — where a call/collection/group/brace-lambda
    /// frame is open. With no binder slot the colon has no reading at all and
    /// dies on the colon itself, at the completed-term hub, rather than reaching
    /// a configuration from which every byte is dead.
    AfterValueColon,
    /// Just consumed the second `:` of a `::` classpath separator; a classpath
    /// identifier must follow. A third `:` is a dead state — `:::` is never valid.
    AfterColon2,
    /// Inside the identifier a typed-binder `:` opened — the binder's *type*
    /// classpath (`row: meta::pure::tds::TDSRow[1]|…`) or, in the arm-R column
    /// binding, the bound variable itself (`~'Total': y|$y->sum()`).
    ///
    /// Distinct from the generic [`InIdent`](State::InIdent) because a binder's
    /// right-hand side is not a free term: only the classpath's own `::`, its
    /// multiplicity `[…]`, and the lambda pipe `|` may follow it. Live-attested —
    /// `->extend(getFloat:row)`, `->extend(a:b.c[1]|1)` and the walk set's own
    /// `|language:fk1DefaultCountry.row[…]` are all "Unexpected token" against the
    /// pinned engine, while `extend(a:b[1]|1)`, `extend(a:b::c[1]|1)` and
    /// `groupBy(~[a:x|$x.b], ~'t':y|$y->sum())` all parse.
    InBinderType,
    /// A completed binder type/variable name, past its trailing whitespace
    /// (`row: TDSRow [1]|…`). The classpath may still resume here — `a:b ::c[1]|1`
    /// parses live — so only the `::`, the multiplicity `[`, and the pipe remain.
    AfterBinderType,
    /// Just consumed the first `:` of a `::` *inside* a binder's type classpath;
    /// a second `:` must follow immediately.
    BinderTypeColon,
    /// Just consumed the second `:` of a binder type classpath's `::`; a
    /// classpath identifier must follow, keeping the type in its own chain.
    BinderTypeColon2,
    /// Inside a binder type that has taken a `::` — so it is a **package path**,
    /// which settles the one ambiguity [`InBinderType`](State::InBinderType)
    /// carries. A path is never an arm-R column binding's bare variable, so the
    /// multiplicity the engine requires of a typed binder is no longer optional
    /// and the lambda pipe is a dead state: `->max(getFloat:row ::weight|…)` is
    /// "Unexpected token '|'. Valid alternatives: \['\[', '(', '<'\]", while
    /// `extend(a:b::c[1]|1)` parses.
    InBinderTypePath,
    /// A completed `::`-bearing binder type, past its trailing whitespace. Like
    /// [`AfterBinderType`](State::AfterBinderType) minus the pipe.
    AfterBinderTypePath,
    /// Right after the `[` of a typed binder's **multiplicity** — a bracket that
    /// holds `1`, `*`, or another integer (`mult`, §5.4) and nothing else.
    /// Distinct from the generic list bracket a `[` opens elsewhere, which is
    /// what let `row['europe']` stream (live: "Unexpected token '.'. Valid
    /// alternatives: ['|']").
    ExpectBinderMult,
    /// Inside a typed binder's integer multiplicity (`[1]`, `[0]`, `[12]`).
    InBinderMult,
    /// A complete multiplicity token (`*`, or an integer past its trailing
    /// whitespace); only the closing `]` remains.
    AfterBinderMultToken,
    /// The typed binder's multiplicity bracket has closed, so the binder is
    /// complete and owes its lambda: only the pipe may follow.
    AfterBinderMult,
    /// The body of a lambda a *binder colon* opened (`row: …[1]|$row.…`,
    /// `~'t': y|$y->sum()`). Identical to
    /// [`ExpectValueReq`](State::ExpectValueReq) except that a `|` is a dead
    /// state: the pipe just consumed was the binder's own, so a second one is
    /// neither a boolean `||` (the binder is not an operand) nor a zero-arg
    /// lambda (`if(c, |x, |y)` opens those after a `,`). Live-attested —
    /// `extend(a:b[1]||1)` and `extend(getFloat:row[2] ||desc)` are both
    /// "Unexpected token '||'. Valid alternatives: ['|']".
    ExpectLambdaBody,
    /// Just consumed `-`; a `>` completes `->`, anything else is arithmetic minus.
    SawDash,
    /// Just consumed `|` after a **name** or a string literal; a second `|` is
    /// boolean `||`, anything else is the lambda-binder pipe and starts the body.
    SawPipe,
    /// Just consumed `|` after a term that is **not** a name — a closed
    /// `(…)`/`[…]`, a number, a date, a `$`-variable, or a member name. A lambda
    /// binder is named by an identifier, so the only reading left here is the
    /// boolean `||`. Live-attested: `->filter(f()|1)`, `->filter(1|1)`,
    /// `->filter($x.a|1)`, `->filter(extend.min|'d')`, `->filter([1]|1)` and
    /// `->filter(%2018-01-01|1)` are each "no viable alternative at input '…|'",
    /// while `->filter(x|1)`, `->filter('a'|1)` and `->filter(a&&b|1)` parse.
    SawValuePipe,
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
            State::AfterName => "AfterName",
            State::AfterMemberName => "AfterMemberName",
            State::AfterStrLit => "AfterStrLit",
            State::InIdent => "InIdent",
            State::InMemberIdent => "InMemberIdent",
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
            State::DateSep => "DateSep",
            State::InDateTime => "InDateTime",
            State::DateTimeSep => "DateTimeSep",
            State::DateFrac => "DateFrac",
            State::InDateFrac => "InDateFrac",
            State::MilestoneL => "MilestoneL",
            State::MilestoneLa => "MilestoneLa",
            State::MilestoneLat => "MilestoneLat",
            State::MilestoneLate => "MilestoneLate",
            State::MilestoneLates => "MilestoneLates",
            State::InMilestoneLit => "InMilestoneLit",
            State::AfterDollar => "AfterDollar",
            State::AfterDot => "AfterDot",
            State::AfterArrow => "AfterArrow",
            State::AfterColon => "AfterColon",
            State::AfterValueColon => "AfterValueColon",
            State::AfterColon2 => "AfterColon2",
            State::InBinderType => "InBinderType",
            State::AfterBinderType => "AfterBinderType",
            State::BinderTypeColon => "BinderTypeColon",
            State::BinderTypeColon2 => "BinderTypeColon2",
            State::InBinderTypePath => "InBinderTypePath",
            State::AfterBinderTypePath => "AfterBinderTypePath",
            State::ExpectBinderMult => "ExpectBinderMult",
            State::InBinderMult => "InBinderMult",
            State::AfterBinderMultToken => "AfterBinderMultToken",
            State::AfterBinderMult => "AfterBinderMult",
            State::ExpectLambdaBody => "ExpectLambdaBody",
            State::SawDash => "SawDash",
            State::SawPipe => "SawPipe",
            State::SawValuePipe => "SawValuePipe",
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
            State::AfterName => 47,
            State::MilestoneL => 48,
            State::MilestoneLa => 49,
            State::MilestoneLat => 50,
            State::MilestoneLate => 51,
            State::MilestoneLates => 52,
            State::InBinderType => 53,
            State::AfterBinderType => 54,
            State::BinderTypeColon => 55,
            State::BinderTypeColon2 => 56,
            State::ExpectBinderMult => 57,
            State::InBinderMult => 58,
            State::AfterBinderMultToken => 59,
            State::AfterBinderMult => 60,
            State::ExpectLambdaBody => 61,
            State::AfterMemberName => 62,
            State::InMemberIdent => 63,
            State::AfterStrLit => 64,
            State::DateSep => 65,
            State::InDateTime => 66,
            State::DateTimeSep => 67,
            State::DateFrac => 68,
            State::InDateFrac => 69,
            State::SawValuePipe => 70,
            State::InBinderTypePath => 71,
            State::AfterBinderTypePath => 72,
            State::AfterValueColon => 73,
        }
    }

    /// The number of distinct automaton states — the length a per-state cache
    /// (`Vec<_>` keyed by [`index`](State::index)) must have. One more than the
    /// largest [`index`](State::index).
    pub const COUNT: usize = 74;

    /// Whether this state is a **completed-term hub** — an inter-lexeme position
    /// the automaton reaches by finishing a term, whichever kind of term it was.
    ///
    /// The four differ only in what may *follow* them (a call's `(`, a classpath
    /// `::`), never in whether a term is behind them, so every consumer that asks
    /// "is a term complete here" — [`Pda::is_accepting`], and the L2 scope
    /// tracker's operand rules — asks it once, here (constitution §4). A new
    /// terminal hub is then a one-line change, not a hunt for enumerations that
    /// silently take a rule dark.
    #[must_use]
    pub(crate) const fn completes_a_term(self) -> bool {
        matches!(
            self,
            State::AfterValue | State::AfterName | State::AfterMemberName | State::AfterStrLit
        )
    }

    /// Whether a `$` read here opens a **refVar sigil** — the states whose
    /// transition on `$` is [`State::AfterDollar`].
    ///
    /// An L1 fact the L2 overlay needs, so it lives beside the transition
    /// function that decides it rather than being restated in `schema/`: S2 can
    /// only mask an unbindable `$` at the anchor *before* the sigil is committed,
    /// and it must not mask a `$` that is merely a byte inside a string literal
    /// (`'a$b'`). The list is not just the two value hubs: a lambda body, a block
    /// statement, and the four operator states that may still grow into a longer
    /// operator (`- < > |`) all open a value on `$` too.
    /// `refvar_sigil_states_match_the_transition_function` recomputes the whole
    /// set from the transition function, so the two cannot drift.
    #[must_use]
    pub(crate) const fn opens_refvar_sigil(self) -> bool {
        matches!(
            self,
            State::ExpectValue
                | State::ExpectValueReq
                | State::ExpectLambdaBody
                | State::BlockStmt
                | State::BlockStmtClose
                | State::SawDash
                | State::SawPipe
                | State::SawLt
                | State::SawGt
        )
    }

    /// The lexeme class this state is *inside*, if any (`None` = an inter-lexeme
    /// or structural position).
    ///
    /// A lexeme is **open** while `lexeme_kind` is `Some(k)` and **closes** at the
    /// byte whose transition takes `Some(k)` to any other verdict. The L2 scope
    /// tracker uses this to buffer a multi-token identifier / string until it
    /// completes (so a byte-level-BPE fragmentation resolves and narrows against
    /// the *whole* lexeme, not a leading sub-token). The `::` classpath-separator
    /// states stay `Ident` so a source classpath never flushes mid-path, and so do
    /// the `let`-candidate states: each is entered on an identifier byte and falls
    /// back to `InSourceIdent` the moment the keyword diverges (`letters`,
    /// `let.foo`), so a path merely *beginning* with `l` is still one open lexeme.
    /// Reporting `None` for them closed the accumulation at that first byte and
    /// took N3 dark for the whole rest of such a path — confirmed live, the walk
    /// `{|l->pair(…)}` ("Can't find the packageable element 'l'").
    pub(crate) const fn lexeme_kind(self) -> Option<LexKind> {
        match self {
            State::InIdent
            | State::InMemberIdent
            | State::InSourceIdent
            | State::InBinder
            | State::InBinderType
            | State::SourceColon
            | State::SourceColon2
            | State::BinderTypeColon
            | State::BinderTypeColon2
            | State::InBinderTypePath
            | State::LetL
            | State::LetLe
            | State::LetLet => Some(LexKind::Ident),
            State::SawNumSign
            | State::InNumberInt
            | State::NeedFracDigit
            | State::InNumberFrac
            | State::SawExp
            | State::NeedExpDigit
            | State::InExp => Some(LexKind::Number),
            State::SawPercent
            | State::InDateLit
            | State::DateSep
            | State::InDateTime
            | State::DateTimeSep
            | State::DateFrac
            | State::InDateFrac
            | State::MilestoneL
            | State::MilestoneLa
            | State::MilestoneLat
            | State::MilestoneLate
            | State::MilestoneLates
            | State::InMilestoneLit => Some(LexKind::Date),
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
const ALL_FRAMES: [Frame; 5] = [
    Frame::Paren,
    Frame::Group,
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
    /// An open `(` that follows a *name* — a call's own argument list, the only
    /// `(` whose contents are a comma-separated element list.
    Paren,
    /// An open `(` at a *value* position — a parenthesised **group**, which
    /// holds one expression and so has no `,` to separate. Live-attested:
    /// `->limit((1,2))` and `->limit(1+(2,3))` are "no viable alternative",
    /// while `->limit((1))`, `->limit((x|1))` and `->limit((a:b[1]|1))` all
    /// parse — a group still opens a lambda and a typed-binder slot.
    Group,
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
            Frame::Group => "Group",
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

/// The engine's one symbolic milestoning literal, spelled without its `%` sigil.
///
/// Legend 4.113.0 lexes `%latest` as a single `LATEST_DATE` token and knows no
/// other `%`-plus-letters symbol: `%latestdate`, `%late` and `%foo` are all
/// "no viable alternative at input '.all(%'" against the pinned stack. The
/// `MilestoneL…` state chain spells this constant one byte at a time, and
/// `the_milestone_chain_spells_exactly_the_engine_symbol` pins the two together.
const MILESTONE_LATEST: &[u8] = b"latest";

/// Whether `top` is a frame whose contents are a **comma-separated element
/// list** — a call's argument list ([`Frame::Paren`]), a collection or
/// multiplicity bracket ([`Frame::Bracket`]), or a brace lambda's typed-binder
/// list ([`Frame::BraceLambda`]).
///
/// The three excluded configurations are the ones where a `,` has no list to
/// separate: an empty stack (a simple query's top level), [`Frame::Brace`] (a
/// block query, whose statements are separated by `;`, never `,`), and
/// [`Frame::Group`] (a parenthesised expression holds one term). All three are
/// live-attested engine rejections — `{|…::Countrylanguage.all(),'Language_T2'}`
/// → "Unexpected token ','. Valid alternatives: ['&&', '||', '==', '!=', '->',
/// '[', '.', ';', '+', '*', '-', '/', '<', '<=', '>', '>=']", and
/// `->limit((1,2))` / `->extend(('MPG_T2',extend))` → "no viable alternative".
///
/// A `,` inside a brace lambda's *body* is still admitted: the PDA cannot see
/// the binder pipe from the frame alone, and §4 forbids inventing a constraint
/// the corpus does not exercise.
const fn separates_elements(top: Option<Frame>) -> bool {
    matches!(
        top,
        Some(Frame::Paren | Frame::Bracket | Frame::BraceLambda)
    )
}

/// Whether `top` is a frame that opens a **lambda slot** — a position a lambda
/// binder's pipe or a typed binder's colon may sit in.
///
/// Every [`separates_elements`] frame, plus [`Frame::Group`]: a parenthesised
/// group takes no `,`, but it does hold a whole lambda (live: `->limit((x|1))`
/// and `->limit((a:b[1]|1))` both parse). The excluded configurations are the
/// ones where the query's own body is already open — an empty stack and a block
/// query's [`Frame::Brace`], whose statements are not binders.
const fn holds_a_lambda_slot(top: Option<Frame>) -> bool {
    separates_elements(top) || matches!(top, Some(Frame::Group))
}

/// Close `top` if `byte` is its matching closer, resuming in `resume`, else
/// [`Step::Dead`].
///
/// The one place delimiter matching is decided; every hub routes its `)`/`]`/`}`
/// here so the context-dependent pop lives in a single spot. Only the resume
/// state varies: a typed binder's multiplicity bracket owes its lambda pipe once
/// it closes, where every other closer yields a completed value.
const fn close_to(top: Option<Frame>, byte: u8, resume: State) -> Step {
    match (top, byte) {
        (Some(Frame::Paren | Frame::Group), b')')
        | (Some(Frame::Bracket), b']')
        | (Some(Frame::Brace), b'}')
        | (Some(Frame::BraceLambda), b'}') => Step::Pop(resume),
        _ => Step::Dead,
    }
}

/// Close `top` and resume at [`State::AfterValue`] — every delimiter but a typed
/// binder's multiplicity bracket yields a completed value when it closes.
const fn close(top: Option<Frame>, byte: u8) -> Step {
    close_to(top, byte, State::AfterValue)
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
        // A `(` here is a parenthesised *group*, not a call: it opens at a value
        // position, with no name in front of it to apply. Its own frame is what
        // keeps a `,` out of it (`separates_elements`).
        b'(' => Step::Push(Frame::Group, State::ExpectValue),
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
        // A lambda binder is named by an identifier, so a pipe off any other
        // completed term can only be the boolean `||`.
        b'|' => Step::Next(State::SawValuePipe),
        // Binary arithmetic: an operand is required, so a closer cannot follow.
        b'+' | b'*' | b'/' => Step::Next(State::ExpectValueReq),
        b'.' => Step::Next(State::AfterDot),
        // A `::` classpath separator binds to a name or a string literal, both of
        // which route their own `:` to [`State::AfterColon`]. Off every other
        // completed term the typed-binder colon is the only reading left, and it
        // needs a slot to bind in — so with no lambda slot open the colon has no
        // reading at all and dies on the colon itself, exactly where the engine
        // says it does (live: `|X.all():language*meta::pure::tds::TDSRow` →
        // "Unexpected token ':'").
        b':' if holds_a_lambda_slot(stack_top) => Step::Next(State::AfterValueColon),
        // A call's `(` and a multiplicity's `[` belong to a *name*, so they live
        // in [`State::AfterName`], not here.
        // A `,` separates list/argument elements: the next element is required
        // (no trailing `(a,)`). It needs an *element list* to separate, which only
        // a call/collection `(`/`[` or a brace lambda's binder list opens — never a
        // block query's own [`Frame::Brace`], whose statements are `;`-separated.
        // Live-attested: `{|…::Countrylanguage.all(),'Language_T2'}` →
        // "Unexpected token ','".
        b',' if separates_elements(stack_top) => Step::Next(State::ExpectValueReq),
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
        step(State::AfterName, stack_top, byte)
    }
}

fn step_in_member_ident(stack_top: Option<Frame>, byte: u8) -> Step {
    if is_ident_tail(byte) {
        Step::Next(State::InMemberIdent)
    } else {
        step(State::AfterMemberName, stack_top, byte)
    }
}

// A completed identifier: the one name-only continuation, then everything a
// completed term admits. Whitespace keeps the position a *name* position, so a
// call written `foo (x)` still streams — the constraint is on what the `(` may
// attach to, never on the spacing.
//
// A multiplicity `[` used to be admitted here too, for the typed binder's
// `row: meta::pure::tds::TDSRow[1]`. That is now its own
// [`State::ExpectBinderMult`] chain, which is the only construct in the emitted
// subset where a `[` follows a name at all — Legend has no positional index and
// says so (live: `{|…::ModelList.all()['MPG_T1_1']}` → "Bracket operation is
// not supported"). Keeping the arm left a `[` admissible off *any* identifier
// with nothing left to exercise it: issue #55 Phase 7's mutation shard caught it
// as an unkillable mutant, which is what a dead arm looks like.
fn step_after_name(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterName),
        b'(' => Step::Push(Frame::Paren, State::ExpectValue),
        // The two continuations a *term-start* name has and a completed value does
        // not: the lambda binder pipe that makes it a parameter, and the `::` that
        // continues a package path.
        b'|' => Step::Next(State::SawPipe),
        b':' => Step::Next(State::AfterColon),
        _ => step_after_value(stack_top, byte),
    }
}

// A completed *member* name — an identifier reached through a `.`, a `->` or a
// `$`. It carries a call's `(` (a milestoned property `$x.facet(%latest)`, an
// arrow step `->filter(…)`) but never the `::` of a classpath: live-attested,
// `…->join('x'.meta::pure::tds::TDSRow)`, `…->getInteger::foo` and
// `…!=$x.foo::bar` are each "no viable alternative at input '…::'".
fn step_after_member_name(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterMemberName),
        b'(' => Step::Push(Frame::Paren, State::ExpectValue),
        _ => step_after_value(stack_top, byte),
    }
}

// A completed string literal. Like a term-start name it may still take a `::`
// — live-attested, `…!='europe'::makeId` and `…!='a b'::c` both parse — but
// never a call's `(`, which binds to an identifier alone (§5.6, Phase 4).
fn step_after_str_lit(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterStrLit),
        b'|' => Step::Next(State::SawPipe),
        b':' => Step::Next(State::AfterColon),
        _ => step_after_value(stack_top, byte),
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
            step(State::AfterStrLit, stack_top, byte)
        }
    } else if byte == b'\'' {
        Step::Next(State::InStrLit { escaped: true })
    } else {
        Step::Next(State::InStrLit { escaped: false })
    }
}

// The `%` sigil opens exactly two literals, and each is pinned at its first
// byte: a *digit* opens the numeric date/time literal (`%2018-03-17T07:13:53`,
// and the engine accepts a bare year run down to `%1`), an `l` opens the
// `%latest` milestone keyword, and everything else is a dead state. The `-`/`T`/
// `:` separators are date *interior* bytes only — live-attested, `%-`, `%T` and
// `%:` are each "no viable alternative at input '…<%'".
fn step_saw_percent(byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InDateLit)
    } else if byte == MILESTONE_LATEST[0] {
        Step::Next(State::MilestoneL)
    } else {
        Step::Dead
    }
}

/// One link of the `%latest` keyword chain: the expected byte advances to
/// `next`, anything else is a dead state — the literal is a symbol, so a
/// divergent byte has not completed a value and cannot be re-dispatched.
fn milestone_link(expected: u8, next: State, byte: u8) -> Step {
    if byte == expected {
        Step::Next(next)
    } else {
        Step::Dead
    }
}

// A date literal's **date half**, on a digit. A `-`/`T` continues it, a `:`
// opens the time half, and the literal is terminal here — it ends on a digit.
// A `.` is *not* admitted: fractional seconds belong to the time half alone
// (live: `%1974.5`, `%0.0` and `%2018-03-17.000` are each "no viable
// alternative", while `%2018-03-17T07:13:53.000` parses).
fn step_in_date_lit(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if b.is_ascii_digit() => Step::Next(State::InDateLit),
        // `-` separates the date's own fields; `T` hands over to the time half.
        b'-' | b'T' => Step::Next(State::DateSep),
        b':' => Step::Next(State::DateTimeSep),
        _ => step(State::AfterValue, stack_top, byte),
    }
}

/// One field separator of a date literal: the field it opens owes at least one
/// digit, so the literal can neither end nor branch here. `next` is the digit
/// state of the half the separator leaves the literal in.
fn date_field(next: State, byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(next)
    } else {
        Step::Dead
    }
}

fn step_date_sep(byte: u8) -> Step {
    date_field(State::InDateLit, byte)
}

fn step_date_time_sep(byte: u8) -> Step {
    date_field(State::InDateTime, byte)
}

// A date literal's **time half**, on a digit and past at least one `:`. This is
// the only place fractional seconds may open.
fn step_in_date_time(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if b.is_ascii_digit() => Step::Next(State::InDateTime),
        // `:` separates the time's own fields and `-` opens a timezone offset
        // (`%2018-03-17T07:13:53-0500` parses). A `T` is dead: the date/time
        // handover happens once, so `%2018-03-17T07:13:53T1` and `%20:18T3` are
        // each "no viable alternative at input".
        b'-' | b':' => Step::Next(State::DateTimeSep),
        b'.' => Step::Next(State::DateFrac),
        _ => step(State::AfterValue, stack_top, byte),
    }
}

fn step_date_frac(byte: u8) -> Step {
    date_field(State::InDateFrac, byte)
}

// The fractional seconds are a date literal's last field: only more digits may
// follow, so a second `.` dies (live: `%2018-03-17T07:13:53.000.111`).
fn step_in_date_frac(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte.is_ascii_digit() {
        Step::Next(State::InDateFrac)
    } else {
        step(State::AfterValue, stack_top, byte)
    }
}

fn step_in_milestone_lit(stack_top: Option<Frame>, byte: u8) -> Step {
    step(State::AfterValue, stack_top, byte)
}

fn step_after_dollar(byte: u8) -> Step {
    if is_ident_start(byte) {
        Step::Next(State::InMemberIdent)
    } else {
        Step::Dead
    }
}

fn step_after_dot(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterDot),
        b if is_ident_start(b) => Step::Next(State::InMemberIdent),
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
        b if is_ident_start(b) => Step::Next(State::InMemberIdent),
        _ => Step::Dead,
    }
}

// A `:` that has just followed a completed term is either the first half of a
// `::` classpath separator — legal wherever a classpath is — or a **typed
// binder**'s own colon (`row: meta::pure::tds::TDSRow[1]|…`, arm-R's
// `~'total': y|…`). Like the lambda pipe it introduces, that binder needs an
// argument or element slot to sit in, i.e. exactly [`separates_elements`]'s
// frames: a block query's [`Frame::Brace`] top level takes statements, not
// binders, and a simple query's empty stack takes the pipeline itself.
// Live-attested at both, e.g.
// `{|…::Countrylanguage.all() && filter:average}` → "Unexpected token ':'".
fn step_after_colon(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        // A `::` separator must be contiguous, so it is decided before the
        // binder arms — and unguarded, since a value-position classpath
        // (`meta::relational::metamodel::join::JoinType`) is legal anywhere.
        b':' => Step::Next(State::AfterColon2),
        _ if !holds_a_lambda_slot(stack_top) => Step::Dead,
        // Whitespace after the first `:` splits off into [`AfterColonWs`], where a
        // second `:` is no longer legal — `::` must be contiguous, so `meta: :pure`
        // dies while the typed binder `row: Type` still streams.
        b if is_ws(b) => Step::Next(State::AfterColonWs),
        b if is_ident_start(b) => Step::Next(State::InBinderType),
        // An arm-R relation aggregate binds a column name to a lambda after a
        // `:` (`colName : {p,w,r|…}` window frame, `~[agg:{…}:…]`); the `{`
        // opens a brace lambda exactly as it does in value position.
        b'{' => Step::Push(Frame::BraceLambda, State::ExpectBraceBinder),
        _ => Step::Dead,
    }
}

// The same `:` off any *other* completed term — a call's `)`, a `]`, a number, a
// date, a `$`-variable or a navigated member. A `::` names a package path, and a
// package path is spelled from a bare word or a quoted one, so that one arm is
// withdrawn and everything else [`step_after_colon`] admits stays: arm-R's second
// column colon legitimately follows a completed navigation
// (`~'Agg': x|$x.v : y|$y->sum()`).
fn step_after_value_colon(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b':' {
        Step::Dead
    } else {
        step_after_colon(stack_top, byte)
    }
}

fn step_after_colon_ws(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterColonWs),
        b if is_ident_start(b) => Step::Next(State::InBinderType),
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

// A typed binder's right-hand side. Unlike the generic [`State::InIdent`] this
// name is not a free term: the binder owes a lambda, so only the type's own `::`
// continuation, its multiplicity bracket, and the pipe that opens the body may
// follow. Every other continuation is live-attested dead — `extend(getFloat:row)`,
// `extend(a:b.c[1]|1)`, `extend(a:b+1)` and `extend(a:'b'|1)` are all rejected by
// the pinned engine, and so are the `|language:fk1DefaultCountry.row[…]` and
// `all:id/'model_list'` shapes the walk set produced.
//
// ponytail (L1 residual, §5.6): the multiplicity itself is still optional here.
// A binder that carries one owes its pipe ([`State::AfterBinderMult`]), but one
// that goes straight to `|` is admitted, because the arm-R column binding
// legitimately has no multiplicity (`~'t': y|$y->sum()`) and the byte machine
// cannot see the `~` that distinguishes it from a typed lambda parameter.
fn step_in_binder_type(byte: u8) -> Step {
    match byte {
        b if is_ident_tail(b) => Step::Next(State::InBinderType),
        b':' => Step::Next(State::BinderTypeColon),
        b if is_ws(b) => Step::Next(State::AfterBinderType),
        b'[' => Step::Push(Frame::Bracket, State::ExpectBinderMult),
        b'|' => Step::Next(State::ExpectLambdaBody),
        _ => Step::Dead,
    }
}

// The binder's type name, past its trailing whitespace: the classpath may still
// resume across the gap (`row: meta ::pure::T[1]`, live-attested), and otherwise
// only the multiplicity and the pipe remain.
fn step_after_binder_type(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBinderType),
        b':' => Step::Next(State::BinderTypeColon),
        b'[' => Step::Push(Frame::Bracket, State::ExpectBinderMult),
        b'|' => Step::Next(State::ExpectLambdaBody),
        _ => Step::Dead,
    }
}

// A `::` inside a binder's type classpath (`row: meta::pure::tds::TDSRow[1]`):
// contiguous, and an identifier must follow, keeping the type in its own chain
// rather than releasing it into the generic value machinery.
fn step_binder_type_colon(byte: u8) -> Step {
    if byte == b':' {
        Step::Next(State::BinderTypeColon2)
    } else {
        Step::Dead
    }
}

fn step_binder_type_colon2(byte: u8) -> Step {
    if is_ident_start(byte) {
        Step::Next(State::InBinderTypePath)
    } else {
        Step::Dead
    }
}

// A binder type that has taken a `::`. The `::` settles what the name is — a
// package path, never an arm-R column binding's bare variable — so the
// multiplicity Legend requires of a typed binder is mandatory here and the
// lambda pipe is dead.
fn step_in_binder_type_path(byte: u8) -> Step {
    match byte {
        b if is_ident_tail(b) => Step::Next(State::InBinderTypePath),
        b':' => Step::Next(State::BinderTypeColon),
        b if is_ws(b) => Step::Next(State::AfterBinderTypePath),
        b'[' => Step::Push(Frame::Bracket, State::ExpectBinderMult),
        _ => Step::Dead,
    }
}

fn step_after_binder_type_path(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBinderTypePath),
        b':' => Step::Next(State::BinderTypeColon),
        b'[' => Step::Push(Frame::Bracket, State::ExpectBinderMult),
        _ => Step::Dead,
    }
}

// A typed binder's multiplicity bracket holds a `mult` and nothing else (§5.4:
// `1`, `*`, or another integer) — it is not the generic list bracket a `[` opens
// elsewhere. Live-attested: `->extend(getFloat:row['europe'] .'150'|…)` is
// "Unexpected token '.'. Valid alternatives: ['|']".
fn step_expect_binder_mult(byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::ExpectBinderMult),
        b if b.is_ascii_digit() => Step::Next(State::InBinderMult),
        b'*' => Step::Next(State::AfterBinderMultToken),
        _ => Step::Dead,
    }
}

fn step_in_binder_mult(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if b.is_ascii_digit() => Step::Next(State::InBinderMult),
        b if is_ws(b) => Step::Next(State::AfterBinderMultToken),
        b']' => close_to(stack_top, byte, State::AfterBinderMult),
        _ => Step::Dead,
    }
}

fn step_after_binder_mult_token(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBinderMultToken),
        b']' => close_to(stack_top, byte, State::AfterBinderMult),
        _ => Step::Dead,
    }
}

// The binder is complete and owes its lambda, so only the pipe — or, inside a
// `join` brace lambda's binder *list*, the `,` that introduces the next binder —
// may follow its multiplicity. Live-attested at both ends: `extend(a:b[1],c)`,
// `extend(a:b[1]->foo())` and `|language:fk1DefaultCountry['…'] && …` are all
// rejected, while `extend(a:b[1] | 1)` and the two-binder join lambda
// `{r1: …[1], r2: …[1]|…}` both parse.
fn step_after_binder_mult(stack_top: Option<Frame>, byte: u8) -> Step {
    match byte {
        b if is_ws(b) => Step::Next(State::AfterBinderMult),
        b'|' => Step::Next(State::ExpectLambdaBody),
        b',' if stack_top == Some(Frame::BraceLambda) => Step::Next(State::ExpectBraceBinder),
        _ => Step::Dead,
    }
}

// A binder lambda's body. The pipe that opened it was the binder's own, so a
// second `|` cannot be a boolean `||` here; every other byte is exactly the
// required-term value position, delegated rather than duplicated (§4, DRY).
fn step_expect_lambda_body(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'|' {
        Step::Dead
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

fn step_saw_dash(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'>' {
        Step::Next(State::AfterArrow)
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

// A `|` that has just followed a completed term is either the second byte of a
// boolean `||` — legal wherever an operator is — or a **lambda binder pipe**,
// which needs an argument slot to be a lambda *in*. That slot is always a call
// argument, a collection element, or a brace lambda's own body, i.e. exactly
// [`separates_elements`]'s frames. At a block query's [`Frame::Brace`] top level
// or on an empty stack (a simple query's top level) the query's own body is
// already open, so a second, bodiless pipe there is a dead state — live-attested
// (`{|…::Db->count('Edispl'.'150')|'AVG(Weight)'…}` → "Unexpected token '|'").
fn step_saw_pipe(stack_top: Option<Frame>, byte: u8) -> Step {
    if byte == b'|' {
        Step::Next(State::ExpectValueReq)
    } else if !holds_a_lambda_slot(stack_top) {
        Step::Dead
    } else {
        step(State::ExpectValueReq, stack_top, byte)
    }
}

// The same pipe, taken after a term that cannot name a binder: only the second
// `|` of a boolean `||` is left, so every other byte is a dead state.
fn step_saw_value_pipe(byte: u8) -> Step {
    if byte == b'|' {
        Step::Next(State::ExpectValueReq)
    } else {
        Step::Dead
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
        State::AfterName => step_after_name(stack_top, byte),
        State::AfterMemberName => step_after_member_name(stack_top, byte),
        State::AfterStrLit => step_after_str_lit(stack_top, byte),
        State::InIdent => step_in_ident(stack_top, byte),
        State::InMemberIdent => step_in_member_ident(stack_top, byte),
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
        State::DateSep => step_date_sep(byte),
        State::InDateTime => step_in_date_time(stack_top, byte),
        State::DateTimeSep => step_date_time_sep(byte),
        State::DateFrac => step_date_frac(byte),
        State::InDateFrac => step_in_date_frac(stack_top, byte),
        State::MilestoneL => milestone_link(MILESTONE_LATEST[1], State::MilestoneLa, byte),
        State::MilestoneLa => milestone_link(MILESTONE_LATEST[2], State::MilestoneLat, byte),
        State::MilestoneLat => milestone_link(MILESTONE_LATEST[3], State::MilestoneLate, byte),
        State::MilestoneLate => milestone_link(MILESTONE_LATEST[4], State::MilestoneLates, byte),
        State::MilestoneLates => milestone_link(MILESTONE_LATEST[5], State::InMilestoneLit, byte),
        State::InMilestoneLit => step_in_milestone_lit(stack_top, byte),
        State::AfterDollar => step_after_dollar(byte),
        State::AfterDot => step_after_dot(byte),
        State::AfterArrow => step_after_arrow(byte),
        State::AfterColon => step_after_colon(stack_top, byte),
        State::AfterValueColon => step_after_value_colon(stack_top, byte),
        State::AfterColon2 => step_after_colon2(byte),
        State::InBinderType => step_in_binder_type(byte),
        State::AfterBinderType => step_after_binder_type(byte),
        State::BinderTypeColon => step_binder_type_colon(byte),
        State::BinderTypeColon2 => step_binder_type_colon2(byte),
        State::InBinderTypePath => step_in_binder_type_path(byte),
        State::AfterBinderTypePath => step_after_binder_type_path(byte),
        State::ExpectBinderMult => step_expect_binder_mult(byte),
        State::InBinderMult => step_in_binder_mult(stack_top, byte),
        State::AfterBinderMultToken => step_after_binder_mult_token(stack_top, byte),
        State::AfterBinderMult => step_after_binder_mult(stack_top, byte),
        State::ExpectLambdaBody => step_expect_lambda_body(stack_top, byte),
        State::SawDash => step_saw_dash(stack_top, byte),
        State::SawPipe => step_saw_pipe(stack_top, byte),
        State::SawValuePipe => step_saw_value_pipe(byte),
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
    /// [`State::AfterValue`] or its name-position twin [`State::AfterName`]
    /// (which an identifier's own trailing whitespace lands in). That
    /// auto-includes every value-terminal lexical
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
                Step::Next(next) if next.completes_a_term()
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

/// Every distinct automaton state — the single source of truth both the
/// in-crate `index`/`COUNT` bijection check and external state-coverage
/// tests read, so there is never a second, independently-maintained copy to
/// drift out of sync. `#[doc(hidden)]`: this is test-support surface, not
/// part of the crate's documented public contract (excluded from the
/// `cargo public-api` snapshot), but a plain private `pub(crate)` cannot
/// cross the crate boundary integration tests under `tests/` compile behind.
#[doc(hidden)]
pub const ALL_STATES: [State; State::COUNT] = [
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
    State::AfterName,
    State::AfterMemberName,
    State::AfterStrLit,
    State::InIdent,
    State::InMemberIdent,
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
    State::DateSep,
    State::InDateTime,
    State::DateTimeSep,
    State::DateFrac,
    State::InDateFrac,
    State::MilestoneL,
    State::MilestoneLa,
    State::MilestoneLat,
    State::MilestoneLate,
    State::MilestoneLates,
    State::InMilestoneLit,
    State::AfterDollar,
    State::AfterDot,
    State::AfterArrow,
    State::AfterColon,
    State::AfterValueColon,
    State::AfterColon2,
    State::InBinderType,
    State::AfterBinderType,
    State::BinderTypeColon,
    State::BinderTypeColon2,
    State::InBinderTypePath,
    State::AfterBinderTypePath,
    State::ExpectBinderMult,
    State::InBinderMult,
    State::AfterBinderMultToken,
    State::AfterBinderMult,
    State::ExpectLambdaBody,
    State::SawDash,
    State::SawPipe,
    State::SawValuePipe,
    State::SawEq,
    State::SawBang,
    State::SawGt,
    State::SawLt,
    State::SawAmp,
    State::SawTilde,
];

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque, hash_map::Entry};

    use super::{
        ALL_FRAMES, ALL_STATES, Frame, LexKind, MILESTONE_LATEST, Pda, State, Step, WS,
        is_ident_start, is_ident_tail, step,
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
    fn refvar_sigil_states_match_the_transition_function() {
        // `opens_refvar_sigil` is a hand-written list; this recomputes it from the
        // transition function over every (state, stack top) pair, so adding a `$`
        // transition anywhere without listing the state here reddens the gate
        // instead of silently taking S2's sigil mask dark at that position.
        for state in ALL_STATES {
            let opens = ALL_FRAMES
                .iter()
                .map(|frame| Some(*frame))
                .chain(std::iter::once(None))
                .any(|top| matches!(step(state, top, b'$'), Step::Next(State::AfterDollar)));
            assert_eq!(
                opens,
                state.opens_refvar_sigil(),
                "opens_refvar_sigil disagrees with the transition function at {state:?}"
            );
        }
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
            State::LetL,
            State::LetLe,
            State::LetLet,
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
        // The date halves' separators differ: a `T` hands over to the time half,
        // so it may open a field in the date half and never in the time half.
        assert!(matches!(
            step(State::InDateLit, None, b'T'),
            Step::Next(State::DateSep)
        ));
        assert!(matches!(step(State::InDateTime, None, b'T'), Step::Dead));
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
        assert_eq!(pda.state(), State::InMemberIdent);
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
        // `%latest` is the engine's one symbolic milestoning literal, usable as an
        // `.all(...)` argument and a milestoned `.PROP(...)` argument (gap report
        // G2, re-attested live in issue #55 Phase 7).
        assert!(accepts("|X.all(%latest)->project([p|$p.n], ['n'])"));
        assert!(accepts("|X.all(%latest, %latest)->take(1)"));
        assert!(accepts(
            "|X.all()->filter(x|$x.FACET(%latest, %latest).seg == 'a')"
        ));
        // A bare `%latest` completes at end-of-stream (value-terminal). The engine
        // admits the symbol only in a milestoning argument slot, so this is a
        // residual L1 over-approximation (§5.6), not an engine-attested shape.
        assert!(accepts("|X.all()->filter(x|$x.d < %latest)"));
    }

    #[test]
    fn a_milestoning_literal_is_exactly_the_latest_symbol() {
        // Bare `%` is still a dead state (the existing date-literal pin).
        assert!(dies("|X.all()->take(%)"));
        // Anything but the `%latest` keyword dies at the byte that diverges from
        // it — including `%latestdate`, which the pinned engine rejects outright.
        assert!(dies("|X.all()->take(%Latest)"));
        assert!(dies("|X.all()->take(%latest1)"));
        assert!(dies("|X.all()->take(%latestX)"));
        assert!(dies("|X.all(%latestdate)->take(1)"));
        assert!(dies("|X.all(%late)->take(1)"));
        assert!(dies("|X.all(%foo)->take(1)"));
        // A milestone literal mid-keyword is not yet accepting.
        assert!(!accepts("|X.all()->filter(x|$x.d < %l"));
        assert!(!accepts("|X.all()->filter(x|$x.d < %lates"));
    }

    /// The `MilestoneL…` chain and [`MILESTONE_LATEST`] are one fact stated
    /// twice; walking the constant through the chain pins them together, so a
    /// state added, dropped, or mis-linked cannot silently change the symbol.
    #[test]
    fn the_milestone_chain_spells_exactly_the_engine_symbol() {
        let mut state = State::SawPercent;
        for (offset, &byte) in MILESTONE_LATEST.iter().enumerate() {
            let Step::Next(next) = step(state, None, byte) else {
                panic!("byte {offset} of the milestone symbol must advance the chain");
            };
            assert!(
                !Pda::at(state).is_accepting(),
                "{} is mid-keyword and must not accept",
                state.name()
            );
            // Past the sigil — which the numeric date literal shares — every other
            // byte at every link is a dead state: the symbol is a keyword, so a
            // divergence has not completed a value to re-dispatch.
            if offset > 0 {
                for other in 0u8..=255 {
                    if other != byte {
                        assert!(
                            matches!(step(state, None, other), Step::Dead),
                            "{}: byte {other:#04x} must not continue the keyword",
                            state.name()
                        );
                    }
                }
            }
            state = next;
        }
        assert_eq!(state, State::InMilestoneLit);
        assert!(Pda::at(state).is_accepting());
    }

    #[test]
    fn direct_step_covers_the_saw_percent_branch() {
        // `%l` opens the milestone keyword, a digit opens the numeric date lexeme,
        // and every other byte — another lowercase letter and the date separators
        // included — dies at the sigil.
        assert!(matches!(
            step(State::SawPercent, None, b'l'),
            Step::Next(State::MilestoneL)
        ));
        assert!(matches!(
            step(State::SawPercent, None, b'2'),
            Step::Next(State::InDateLit)
        ));
        for dead in *b"aZ)-T:" {
            assert!(matches!(step(State::SawPercent, None, dead), Step::Dead));
        }
    }

    #[test]
    fn a_completed_milestone_literal_is_value_terminal() {
        // It delegates every byte to `AfterValue`, so a following `)` pops a frame
        // while a letter (the `d` of `%latestdate`) or a digit dies.
        assert!(matches!(
            step(State::InMilestoneLit, Some(Frame::Paren), b')'),
            Step::Pop(State::AfterValue)
        ));
        for dead in *b"d1" {
            assert!(matches!(
                step(State::InMilestoneLit, None, dead),
                Step::Dead
            ));
        }
    }

    /// A typed binder's right-hand side owes a lambda, so only the type's own
    /// `::`, its multiplicity `[`, and the pipe may follow it — every other
    /// continuation is a dead state, live-attested against the pinned engine.
    /// The `::` also resumes across whitespace *before* it (`a:b ::c`) but not
    /// after (`a:b:: c`), exactly as `AfterColon2` already had it everywhere else.
    #[test]
    fn a_typed_binder_type_admits_only_a_classpath_a_multiplicity_and_a_pipe() {
        for text in [
            "|db::Db->tableReference('default','T')->tableToTDS()\
             ->filter(row: meta::pure::tds::TDSRow[1]|$row.getInteger('c') > 1)",
            "|X.all()->extend(a:b[1]|1)",
            "|X.all()->extend(a:b::c[1]|1)",
            "|X.all()->extend(a:b ::c[1]|1)",
            "|X.all()->extend(a:b[*]|1)",
            "|X.all()->extend(a:b[12]|1)",
            "|X.all()->extend(a :b [1]|1)",
            "|X.all()->extend(a:b[ 1 ] | 1)",
            // The arm-R column binding omits the multiplicity (`~'t': y|…`).
            "|X.all()->groupBy(~[a:x|$x.b],~'t':y|$y->sum())",
        ] {
            assert!(accepts(text), "engine-legal binder refused: {text:?}");
        }
        for text in [
            "|X.all()->extend(getFloat:row)",
            "|X.all()->extend(a:b.c[1]|1)",
            "|X.all()->extend(a:b['europe']|1)",
            "|X.all()->extend(a:b[1x]|1)",
            "|X.all()->extend(a:b[**]|1)",
            "|X.all()->extend(a:b[]|1)",
            "|X.all()->extend(a:b[1],c)",
            "|X.all()->extend(a:b[1]->foo())",
            "|X.all()->extend(a:b[1]&&1)",
            "|X.all()->extend(a:b+1)",
            "|X.all()->extend(a:b/1)",
            "|X.all()->extend(a:'b'|1)",
            "|X.all()->extend(a:b : c[1]|1)",
            "|X.all()->extend(a:b:::c[1]|1)",
            "|X.all()->extend(a:b:: c[1]|1)",
        ] {
            assert!(dies(text), "the recogniser still streams {text:?}");
        }
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
