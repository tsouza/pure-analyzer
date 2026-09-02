//! Spider structural corpus replay — the broad L2 structural gate.
//!
//! Derives cases from every schema in `corpus/spider_schemas/` (158 Spider DBs) and
//! replays each through a schema-aware [`DecoderSession`], asserting three things:
//!
//! - **Soundness is absolute.** Every real member, chained navigation, and legal
//!   fused nav-dot must stream / be admitted — zero failures, no allowlist. A
//!   soundness failure is a masked-gold bug (constitution §3, §8.6): fix the
//!   decoder, never the corpus.
//! - **Precision holds except where documented.** Every phantom must be masked,
//!   except cases carrying a [`GapKind`] tag — known L1 over-approximations tracked
//!   for a follow-up fix. Leaks ⟺ tags: an untagged leak reddens the gate, and a
//!   tagged case that *stops* leaking also reddens (a stale allowlist entry), so a
//!   fix cannot land silently.
//! - **The corpus is pinned.** Exact case counts per family are constants; a schema
//!   or generator change that moves them is a visible, reviewed diff (no thresholds,
//!   constitution §3).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

#[path = "support/lex.rs"]
mod lex;
#[path = "support/spider_corpus.rs"]
mod spider_corpus;

use lex::lex;
use purecard::{CompiledGrammar, DecoderSession, Schema, Vocab};
use spider_corpus::{Case, Check, GapKind, generate};

/// A lexeme-granularity vocabulary over a set of queries plus injected probe tokens,
/// with an id lookup — the schema-aware analogue of `differential_l1`'s `vocab_for`,
/// built here directly so the gate needs no schema-heavy L2 harness (which would
/// drag an unused `load_schema` into this target).
struct CaseVocab {
    ids: BTreeMap<Vec<u8>, u32>,
    vocab: Vocab,
}

impl CaseVocab {
    fn build(queries: &[&str], extras: &[Vec<u8>]) -> Self {
        let mut ids = BTreeMap::new();
        let mut tokens: Vec<Vec<u8>> = Vec::new();
        let mut add = |tok: Vec<u8>| {
            if let std::collections::btree_map::Entry::Vacant(e) = ids.entry(tok.clone()) {
                e.insert(tokens.len() as u32);
                tokens.push(tok);
            }
        };
        for q in queries {
            for tok in lex(q) {
                add(tok);
            }
        }
        for extra in extras {
            add(extra.clone());
        }
        Self {
            ids,
            vocab: Vocab::from_byte_tokens(tokens),
        }
    }

    fn id_of(&self, token: &[u8]) -> Option<u32> {
        self.ids.get(token).copied()
    }
}

/// Total generated cases across all 158 schemas.
const EXPECTED_TOTAL: usize = 31633;
/// Soundness cases (real members / navigations / legal fused dots) — must all pass.
const EXPECTED_SOUNDNESS: usize = 9284;
/// Precision cases the decoder currently masks correctly (no gap tag). Includes
/// the 833 `N1_prefix` strict-prefix phantoms that were a documented leak until
/// the overlay learned to terminate an open identifier (`docs/spec/schema.md`
/// §6.5): a boundary token is now masked at a cursor that has only reached a
/// strict prefix of a legal name.
const EXPECTED_PRECISION_CLEAN: usize = 22326;
/// Known N2 ambiguous-scalar-end leaks (`$x.<scalar-and-end>.<phantom>`).
const EXPECTED_N2_AMBIG_LEAKS: usize = 23;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/spider_schemas")
}

fn schema_json(db: &str) -> String {
    let path = schema_dir().join(format!("{db}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Whether the decoder does what a case asserts. `Ok(())` = held; `Err(reason)` =
/// violated (a masked real token, or an admitted phantom), with a diagnostic.
fn holds(session: &mut DecoderSession<'_>, vocab: &CaseVocab, case: &Case) -> Result<(), String> {
    match &case.check {
        Check::Streams | Check::DeadEnds => {
            for (step, token) in lex(&case.query).into_iter().enumerate() {
                let id = vocab
                    .id_of(&token)
                    .unwrap_or_else(|| panic!("token not in vocab: {token:?} ({})", case.id));
                if !session.allowed_mask().test(id) {
                    // Dead-ended here.
                    return match case.check {
                        Check::DeadEnds => Ok(()),
                        _ => Err(format!(
                            "masked real token at step {step} in {}",
                            case.query
                        )),
                    };
                }
                session
                    .accept_token(id)
                    .map_err(|e| format!("rejected token at step {step}: {e}"))?;
            }
            let complete = session.is_complete();
            match case.check {
                Check::Streams if complete => Ok(()),
                Check::Streams => Err(format!("did not complete: {}", case.query)),
                // DeadEnds but every token was admissible and the stream completed:
                // the phantom leaked.
                Check::DeadEnds if complete => Err(format!("phantom streamed: {}", case.query)),
                Check::DeadEnds => Ok(()),
                // The outer match already narrowed to Streams/DeadEnds; a
                // ProbeAdmitted/ProbeMasked case can't reach this arm. Fails
                // the test with a clear message instead of `unreachable!()`
                // (banned workspace-wide by `no-postponed-markers`, now in
                // scope for tests too — see docs/decisions, migrate-purecard).
                _ => Err(format!(
                    "case {} has check {:?}, not Streams/DeadEnds, but reached the \
                     Streams/DeadEnds arm — a bug in this test's own dispatch, not the decoder",
                    case.id, case.check
                )),
            }
        }
        Check::ProbeAdmitted(probe) | Check::ProbeMasked(probe) => {
            for token in lex(&case.query) {
                let id = vocab.id_of(&token).unwrap_or_else(|| {
                    panic!("prefix token not in vocab: {token:?} ({})", case.id)
                });
                session
                    .accept_token(id)
                    .map_err(|e| format!("prefix token rejected: {e}"))?;
            }
            let id = vocab.id_of(probe).expect("probe token in vocab");
            let admitted = session.allowed_mask().test(id);
            match &case.check {
                Check::ProbeAdmitted(_) if admitted => Ok(()),
                Check::ProbeAdmitted(_) => Err(format!("legal fused dot masked: {}", case.id)),
                Check::ProbeMasked(_) if !admitted => Ok(()),
                Check::ProbeMasked(_) => Err(format!("phantom fused dot admitted: {}", case.id)),
                // Same reasoning as the Streams/DeadEnds arm above: the outer
                // match already narrowed to ProbeAdmitted/ProbeMasked.
                other => Err(format!(
                    "case {} has check {other:?}, not a probe variant, but reached the \
                     probe-only arm — a bug in this test's own dispatch, not the decoder",
                    case.id
                )),
            }
        }
    }
}

/// One case's classified outcome.
struct Outcome {
    soundness: bool,
    gap: Option<GapKind>,
    held: bool,
    reason: Option<String>,
    id: String,
}

fn run_all() -> Vec<Outcome> {
    let cases = generate(&schema_dir());
    // Group by DB so one vocabulary + compiled grammar + schema serves every case
    // in that database (compilation is the cost; queries share a lexeme universe).
    let mut by_db: BTreeMap<String, Vec<Case>> = BTreeMap::new();
    for case in cases {
        by_db.entry(case.db.clone()).or_default().push(case);
    }

    let mut outcomes = Vec::new();
    for (db, cases) in &by_db {
        let queries: Vec<&str> = cases.iter().map(|c| c.query.as_str()).collect();
        let extras: Vec<Vec<u8>> = cases
            .iter()
            .filter_map(|c| match &c.check {
                Check::ProbeAdmitted(p) | Check::ProbeMasked(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        let vocab = CaseVocab::build(&queries, &extras);
        let grammar = CompiledGrammar::compile(vocab.vocab.clone());
        let schema = Schema::from_json(&schema_json(db)).expect("delivered schema parses");
        let mut session =
            DecoderSession::with_schema(&grammar, schema).expect("grammar is fixed-engine");
        for case in cases {
            session.reset();
            let result = holds(&mut session, &vocab, case);
            outcomes.push(Outcome {
                soundness: case.check.is_soundness(),
                gap: case.gap,
                held: result.is_ok(),
                reason: result.err(),
                id: case.id.clone(),
            });
        }
    }
    outcomes
}

#[test]
fn spider_corpus_replays_with_pinned_soundness_and_documented_precision() {
    let outcomes = run_all();

    // (1) Soundness is absolute: no real construct may be masked.
    let masked_gold: Vec<&Outcome> = outcomes.iter().filter(|o| o.soundness && !o.held).collect();
    assert!(
        masked_gold.is_empty(),
        "L2 SOUNDNESS: {} real construct(s) masked (fix the decoder, never the corpus):\n{}",
        masked_gold.len(),
        masked_gold
            .iter()
            .take(10)
            .map(|o| format!("  {}: {}", o.id, o.reason.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // (2) Precision, untagged cases: no undocumented leak.
    let undocumented: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| !o.soundness && o.gap.is_none() && !o.held)
        .collect();
    assert!(
        undocumented.is_empty(),
        "L2 PRECISION: {} undocumented phantom leak(s) (fix, or tag with a GapKind + count):\n{}",
        undocumented.len(),
        undocumented
            .iter()
            .take(10)
            .map(|o| format!("  {}: {}", o.id, o.reason.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // (3) Tagged cases must still leak: a stale allowlist entry (now masked) reddens
    // so a fix cannot land without removing its tag and lowering the count.
    let stale: Vec<&Outcome> = outcomes
        .iter()
        .filter(|o| o.gap.is_some() && o.held)
        .collect();
    assert!(
        stale.is_empty(),
        "STALE GAP: {} tagged case(s) now masked correctly — remove the tag and lower the pinned count:\n{}",
        stale.len(),
        stale
            .iter()
            .take(10)
            .map(|o| format!("  {}", o.id))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // (4) Exact counts pin the corpus (no thresholds).
    let total = outcomes.len();
    let soundness = outcomes.iter().filter(|o| o.soundness).count();
    let precision_clean = outcomes
        .iter()
        .filter(|o| !o.soundness && o.gap.is_none())
        .count();
    let n2_ambig = outcomes
        .iter()
        .filter(|o| o.gap == Some(GapKind::N2AmbiguousScalarEnd))
        .count();

    assert_eq!(total, EXPECTED_TOTAL, "total case count moved");
    assert_eq!(soundness, EXPECTED_SOUNDNESS, "soundness case count moved");
    assert_eq!(
        precision_clean, EXPECTED_PRECISION_CLEAN,
        "clean-precision case count moved"
    );
    assert_eq!(
        n2_ambig, EXPECTED_N2_AMBIG_LEAKS,
        "N2 ambiguous-scalar-end leak count moved"
    );
}
