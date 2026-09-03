//! State-coverage proof for issue #59's "record stable coverage of
//! states, ... fail on uncovered shipped rules" bullet, the residual half
//! `schema_walk_construct_coverage.rs` left open: that file's own doc
//! comment explains why it stays on frame/construct proxies rather than
//! `L2Position`/rule identity (private, the L2 overlay's scoping boundary) —
//! but `grammar::pda::State` carries no such boundary. It was already
//! `pub`, just not *enumerable* from outside the crate (the exhaustive
//! `ALL_STATES` list lived in `pda.rs`'s own `#[cfg(test)] mod tests`,
//! backing only the in-crate `index`/`COUNT` bijection check). Promoting
//! that one list to `#[doc(hidden)] pub` (no public-API-surface growth —
//! excluded from the `cargo public-api` snapshot) gives this file the same
//! source of truth, rather than a second, independently-maintained copy.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::PathBuf;

#[path = "support/corpus.rs"]
mod corpus;
#[path = "support/error.rs"]
mod error;
#[path = "support/fixture_dbs.rs"]
mod fixture_dbs;
#[path = "support/l2.rs"]
mod l2;
#[path = "support/lex.rs"]
mod lex;

use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{TokenVocab, load_schema};
use purecard::grammar::pda::{ALL_STATES, State};
use purecard::{CompiledGrammar, DecoderSession};
use schema_walker::generate_schema_walks;

/// The structural bytes offered as their own single-byte token, in addition to
/// a db's gold-query lexemes, so a walk can explore beyond verbatim replay —
/// the same set the other coverage/completeness files use, kept as its own
/// copy per `fixture_dbs.rs`'s documented house convention (never partially
/// reuse a support module's neighbor).
const STRUCTURAL_BYTES: &[u8] = b"abXY1_ |{}()[].,;:$%'-><=!&+*/";

/// States `ALL_STATES` lists that this corpus is not expected to reach, each
/// with the concrete reason — a state absent here that the test still finds
/// unvisited is a real regression, not documented residue.
///
/// - `LetL`/`LetLe`: block-query syntax (`{|let m = …; …}`). The walker is
///   fundamentally class-anchored (`ClassPath.all()->…`, `docs/spec/grammar.md`
///   §5's arm-C shape) — it never opens a block query at all, so no state
///   reachable only through one can appear. This test samples the PDA at token
///   *boundaries*, and every `let` in a fixture vocabulary is one atomic
///   lexeme, so a walk could only land on `LetL`/`LetLe` by ending a token on a
///   bare `l`/`le`. `LetL` was visited that way until issue #55's Phase 2:
///   those states are inside a source identifier (each falls back to
///   `InSourceIdent` the moment the keyword diverges), but `lexeme_kind`
///   reported them as inter-lexeme, so N3 released at the first `l` and a bare
///   `l` read as a finished source — the live-rejected walk `{|l->pair(…)}`
///   ("Can't find the packageable element 'l'"). With the accumulation kept
///   open, `l` is a strict prefix of `let` and every boundary token after it is
///   masked, so the only route this generator had into either state is gone.
///   `LetLet` stays reachable: `let` is a whole name in N3's own trie.
/// - `SawExp`/`NeedExpDigit`/`InExp`: scientific-notation numeric literals
///   (`1e5`). None of the 8 `FIXTURE_DBS` gold corpora (Spider-derived SQL
///   translations) contain one, and the walker only draws numeric tokens from
///   corpus lexemes plus `STRUCTURAL_BYTES`, neither of which supplies an `e`
///   exponent shape.
/// - `SawTilde`, and the nine states only `SawTilde` reaches
///   (`InRelColIdent`/`InRelColStrLit`/`AfterRelColName`/`ExpectRelColSpec`/
///   `ExpectRelColSpecReq`, issue #361's narrower arm-R colName positions, plus
///   `AfterRelColColon`/`InRelColLambdaBinder`/`AfterRelColLambdaBinder`, issue
///   #368's narrower arm-R binder-after-colon positions): the arm-R
///   Relation/Function API sigil (`~Col`, `~[…]`). None of the 8 `FIXTURE_DBS`
///   gold corpora contain an arm-R construct (arm-R is exercised elsewhere,
///   e.g. `l2_precision.rs`'s hand-written queries, not through this
///   generator).
/// - `MilestoneL`…`MilestoneLates`/`InMilestoneLit`: the `%latest` keyword chain
///   (issue #55 Phase 7). `InMilestoneLit` *was* reached — through `%a`,
///   `%filter`, `%limit` and friends, every one of which the pinned engine
///   rejects at the `%` ("no viable alternative at input '.all(%'"). Now that
///   L1 spells the engine's one symbol exactly, reaching the chain needs a
///   vocabulary that can spell `latest` after a `%`: none of the 8 fixture
///   vocabularies can — `STRUCTURAL_BYTES` offers no `l`/`t`/`e`/`s`, and no
///   gold lexeme is a prefix-aligned fragment of the keyword. This is a
///   property of these test vocabularies, not of the grammar — the chain is
///   driven end to end by the modern-dialect seed corpus
///   (`modern_dialect_soundness.rs`), by `precision_reject.rs`, and byte by
///   byte by `pda`'s own `the_milestone_chain_spells_exactly_the_engine_symbol`.
const EXPECTED_UNREACHABLE: &[&str] = &[
    "LetL",
    "LetLe",
    "SawExp",
    "NeedExpDigit",
    "InExp",
    "SawTilde",
    "InRelColIdent",
    "InRelColStrLit",
    "InRelColStrLit(pendingQuote)",
    "AfterRelColName",
    "ExpectRelColSpec",
    "ExpectRelColSpecReq",
    "AfterRelColColon",
    "InRelColLambdaBinder",
    "AfterRelColLambdaBinder",
    "MilestoneL",
    "MilestoneLa",
    "MilestoneLat",
    "MilestoneLate",
    "MilestoneLates",
    "InMilestoneLit",
];

/// Hand-written walks replayed **alongside** the generated corpus, pinning states
/// the random walker reaches only by chance.
///
/// The generator samples from the L2 mask at every step, so any change to the
/// overlay reshuffles every draw it makes: a state can drop out of the sampled
/// corpus without becoming unreachable. `DateFrac` (the `.` that opens a date
/// literal's fractional seconds, inside `.all(…)`'s argument list, which N3b's
/// argument rule admits as a milestone date) was covered that way until issue
/// #275's S2 sigil mask removed the `$`-before-any-binder branches the sampler
/// used to spend draws on. It is still reachable — this walk reaches it — so it is
/// pinned here rather than moved into [`EXPECTED_UNREACHABLE`], which may only
/// ever shrink (constitution §7: a gate is never self-lowered).
///
/// `InMultiplicity` is here for exactly the same reason, and its own
/// [`EXPECTED_UNREACHABLE`] entry is retired by it: that entry said in as many
/// words that the state was reached "incidentally" and that "this entry would
/// disappear again on the next reshuffle". It is reachable through a lambda body's
/// `[*]` — this walk reaches it — so a pin is the better remedy than a residue
/// note, and the list shrinks by one.
///
/// `SourceColon`/`SourceColon2` (the `::` separator *inside* a source
/// classpath, as a token-boundary state) join them for issue #367's own
/// reason: the S2 sigil exemption `SourceMethodArg`/`AfterDollar` now needs
/// (`schema/scope.rs`'s `masks_unbound_sigil` and `opening_position`)
/// reshuffles the sampler's draws through every `$`-adjacent decision point,
/// and one of the newly-sampled walks happens to type a `let` binder's own
/// value as a fresh classpath (`let a = a::a`, `docs/spec/grammar.md`'s
/// `pipeline | scalarExpr` binder-value shape, admitted since issue #352):
/// `ExpectBinderValue`/`InBinderValueIdent` land on the *same* shared
/// `source_ident` transition function `InSourceIdent` does (`grammar/pda.rs`),
/// but — unlike the primary pipeline source — N3's classpath-continuation rule
/// was never extended to narrow a binder value's own classpath, so nothing
/// masks the `:` that opens `SourceColon` there. That gap is real (a
/// fabricated `let a = spider::…::Battle::phantom` binder value streams
/// unmasked) but is a distinct N3 scoping question from this issue's
/// milestoning-date fix, tracked separately as issue #371 rather than folded
/// in here (constitution §6): this pin only proves the state is — and, absent
/// that follow-up, remains — reachable, exactly as `DateFrac` above.
///
/// Each walk is a token list over the db's own vocabulary, and every token must be
/// admissible where it sits: a stale probe reddens the lane instead of quietly
/// covering nothing.
const PROBE_WALKS: &[(&str, &[&[u8]])] = &[
    (
        "battle_death",
        &[
            b"|",
            b"spider::battle_death::model::default::Battle",
            b".",
            b"all",
            b"(",
            b"%",
            b"1",
            b":",
            b"1",
            b".",
        ],
    ),
    (
        "battle_death",
        &[
            b"|",
            b"spider::battle_death::model::default::Battle",
            b".",
            b"all",
            b"(",
            b")",
            b"-",
            b">",
            b"filter",
            b"(",
            b"a",
            b"|",
            b"[",
            b"*",
        ],
    ),
    (
        // Issue #367 / #371: a `let` binder's own value is a classpath
        // grammar shares with the primary pipeline source
        // (`ExpectBinderValue` -> `InBinderValueIdent` -> `SourceColon` ->
        // `SourceColon2` -> `InSourceIdent`, the same `source_ident`
        // transition function `grammar/pda.rs` uses for both) — `let` is a
        // whole gold-corpus lexeme only in `concert_singer` among the 8
        // `FIXTURE_DBS`.
        "concert_singer",
        &[
            b"{", b"|", b"let", b" ", b"a", b" ", b"=", b"a", b":", b":", b"a",
        ],
    ),
];

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

fn grammar_and_schema(db_id: &str) -> (CompiledGrammar, purecard::Schema, TokenVocab) {
    let extra: Vec<Vec<u8>> = STRUCTURAL_BYTES.iter().map(|&byte| vec![byte]).collect();
    let queries: Vec<String> = load_gold(&corpus_path())
        .expect("open the committed gold corpus")
        .filter_map(Result::ok)
        .filter(|record| record.db_id == db_id)
        .map(|record| record.pure_text)
        .collect();
    assert!(
        !queries.is_empty(),
        "no gold queries for fixture db {db_id}"
    );
    let refs: Vec<&str> = queries.iter().map(String::as_str).collect();

    let vocab = TokenVocab::build(&refs, &extra);
    let grammar = CompiledGrammar::compile(vocab.vocab());
    let schema = load_schema(db_id);
    (grammar, schema, vocab)
}

/// Every [`State`] reachable through the schema-walk generator's own
/// constructs is visited at least once across the whole generated corpus
/// (every fixture schema's [`generate_schema_walks`] output) — the residual
/// half of #59's "states... coverage" bullet
/// `schema_walk_construct_coverage.rs` left open. [`EXPECTED_UNREACHABLE`]
/// documents exactly which states this specific corpus cannot reach and why;
/// a state missing from *both* the visited set and that list is a real
/// coverage regression.
#[test]
fn every_reachable_pda_state_is_visited_at_least_once() {
    let mut visited: HashSet<State> = HashSet::new();
    for &db_id in FIXTURE_DBS {
        let (grammar, schema, _vocab) = grammar_and_schema(db_id);
        let walks = generate_schema_walks(&grammar, &schema);
        for walk in &walks {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone())
                .expect("grammar is fixed-engine");
            visited.insert(
                session
                    .pda()
                    .expect("fixed-engine grammar always exposes its Pda")
                    .state(),
            );
            for &id in walk {
                session
                    .accept_token(id)
                    .expect("a walk's own token is always admissible");
                visited.insert(
                    session
                        .pda()
                        .expect("fixed-engine grammar always exposes its Pda")
                        .state(),
                );
            }
        }
    }

    for (db_id, walk) in PROBE_WALKS {
        let (grammar, schema, vocab) = grammar_and_schema(db_id);
        let mut session =
            DecoderSession::with_schema(&grammar, schema.clone()).expect("grammar is fixed-engine");
        for token in *walk {
            let id = vocab
                .id_of(token)
                .unwrap_or_else(|| panic!("probe token not in the {db_id} vocabulary: {token:?}"));
            assert!(
                session.allowed_mask().test(id),
                "probe walk token {token:?} is masked in {db_id} — the probe is stale"
            );
            session
                .accept_token(id)
                .expect("a probe walk's own token is admissible");
            visited.insert(
                session
                    .pda()
                    .expect("fixed-engine grammar always exposes its Pda")
                    .state(),
            );
        }
    }

    let missing: Vec<&str> = ALL_STATES
        .iter()
        .filter(|state| !visited.contains(state))
        .map(|state| state.name())
        .collect();
    let mut missing_sorted = missing.clone();
    missing_sorted.sort_unstable();
    let mut expected_sorted = EXPECTED_UNREACHABLE.to_vec();
    expected_sorted.sort_unstable();
    assert_eq!(
        missing_sorted, expected_sorted,
        "unvisited state set drifted from EXPECTED_UNREACHABLE — update the \
         documented list (with a reason) if this is an intentional corpus/walker \
         change, otherwise this is a real coverage regression"
    );
}
