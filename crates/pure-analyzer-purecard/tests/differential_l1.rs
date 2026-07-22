//! Differential gate: L1 vs. the real Legend engine grammar.
//!
//! `corpus/differential_l1.jsonl` holds query strings each labeled with the Legend
//! engine's *grammar* verdict — `parse_ok` / `parse_fail` — frozen offline by
//! `just label-differential` (which POSTs to a running engine; see
//! `scripts/label-differential.mjs`). This test replays that frozen corpus against
//! **L1 only** — no engine at CI time (the decoder core is pure, constitution §1).
//!
//! The load-bearing property is **soundness**: L1 must admit every query the engine
//! parses (`legend parse_ok ⟹ L1 accepts`), except a small, documented set of
//! constructs where L1 is *deliberately stricter* than the permissive engine
//! grammar ([`KNOWN_DIVERGENCES`]). A new `parse_ok` query that L1 rejects and is
//! not in that allowlist reddens this gate — exactly the class that let the
//! `|X.'name'` regression slip through review before this harness existed.
//!
//! The engine's `grammarToJson` is *grammar-permissive*: it parses `5abc` / `1_000`
//! as element references (`packageableElementPtr`), deferring existence-checks to a
//! later phase. L1, a constrained decoder, deliberately rejects that residue rather
//! than let the model emit garbage where a value belongs — hence the allowlist,
//! not a blanket "match the engine". The engine and L1 both target Legend 4.113.0
//! (see `docs/spec/grammar.md`), so the labels are version-faithful.
#![forbid(unsafe_code)]

use std::path::PathBuf;

#[path = "support/lex.rs"]
mod lex;

use purecard::{ByteRecognizer, CompiledGrammar, DecoderSession, Vocab};

/// Queries the engine's permissive grammar parses but L1 deliberately rejects.
/// Each is an intentional "L1 stricter than the engine" case, **not** a soundness
/// bug — recorded here so the gate stays green while any *new* divergence fails.
///
/// This is the training-side decision (B): L1 targets the *intended query dialect*
/// and is deliberately stricter than the permissive engine. A bare/number-shaped
/// element-reference operand is exactly the hallucination class constrained
/// decoding exists to catch, so L1 rejects it as out-of-dialect residue rather
/// than mirror the engine's permissive over-parse. The first
/// two are that element-reference residue (the engine reads a number-glued-to-
/// letters / underscore-number as a `packageableElementPtr`). The third is a
/// zero-param lambda `{|…}`, engine-legal but a low-value construct L1 does not
/// model. (Qualified/dotted enum-ref operands like `Type.Meeting` are legal Pure
/// and stay admitted — see `enum_ref_operands_stay_admitted`.)
const KNOWN_DIVERGENCES: &[&str] = &[
    "|Firm.all()->filter(x|$x.n == 1_000)",
    "|Person.all()->filter(x|$x.age > 5abc)",
    "|Person.all()->filter({|$x.age})",
];

/// One corpus row: a query and the engine's frozen grammar verdict.
struct Row {
    query: String,
    legend: String,
    line: usize,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/differential_l1.jsonl")
}

fn load_corpus() -> Vec<Row> {
    let text = std::fs::read_to_string(corpus_path()).expect("read differential corpus");
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, line)| {
            let v: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("line {}: {e}", i + 1));
            let legend = v["legend"].as_str().expect("legend").to_owned();
            // A typo like "parse_ok " (trailing space) or "ok" would otherwise be
            // silently skipped by the `!= "parse_ok"` guard, dropping a row from the
            // soundness loop unnoticed. Only the two known verdicts are admissible.
            assert!(
                matches!(legend.as_str(), "parse_ok" | "parse_fail"),
                "line {}: unknown legend verdict {legend:?} (expected parse_ok or parse_fail)",
                i + 1
            );
            Row {
                query: v["q"].as_str().expect("q").to_owned(),
                legend,
                line: i + 1,
            }
        })
        .collect()
}

/// Build a lexeme-granularity [`Vocab`] over exactly `query`'s own tokens. The
/// grammar-only differential lane needs no schema, so it lexes the query directly
/// instead of pulling in the schema-heavy L2 harness.
fn vocab_for(query: &str) -> Vocab {
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for tok in lex::lex(query) {
        if !tokens.contains(&tok) {
            tokens.push(tok);
        }
    }
    let eos = tokens.len() as u32;
    Vocab::from_byte_tokens(tokens, eos)
}

/// Does L1 accept `query` as a complete stream (grammar-only, no schema)?
fn l1_accepts(query: &str) -> bool {
    let grammar = CompiledGrammar::compile(vocab_for(query));
    let mut session = DecoderSession::new(&grammar);
    for byte in query.bytes() {
        if session.accept_byte(byte).is_err() {
            return false;
        }
    }
    session.is_complete()
}

#[test]
fn l1_admits_every_engine_legal_query_outside_the_documented_allowlist() {
    let corpus = load_corpus();
    // Floors, both well under the current corpus, guard against a truncated or
    // wholesale-relabelled corpus passing vacuously (see MIN_PARSE_OK below).
    const MIN_CORPUS_ROWS: usize = 150;
    assert!(
        corpus.len() >= MIN_CORPUS_ROWS,
        "differential corpus unexpectedly small ({}, floor {MIN_CORPUS_ROWS}); a truncated corpus hides gaps",
        corpus.len()
    );
    // The soundness loop only exercises `parse_ok` rows, so a mass-relabel to
    // `parse_fail` (e.g. a bad engine run flipping every verdict) would make it
    // vacuously green while passing the total-size check. Floor the `parse_ok`
    // population too, well under the current count, so that failure mode reddens.
    const MIN_PARSE_OK: usize = 120;
    let parse_ok = corpus.iter().filter(|r| r.legend == "parse_ok").count();
    assert!(
        parse_ok >= MIN_PARSE_OK,
        "differential corpus has only {parse_ok} parse_ok rows (floor {MIN_PARSE_OK}); \
         a wholesale relabel would make the soundness loop vacuous"
    );
    let mut violations = Vec::new();
    for row in &corpus {
        if row.legend != "parse_ok" {
            continue;
        }
        if l1_accepts(&row.query) {
            continue;
        }
        // Engine parses it, L1 rejects it: allowed only if documented.
        if !KNOWN_DIVERGENCES.contains(&row.query.as_str()) {
            violations.push(format!("  line {}: {}", row.line, row.query));
        }
    }
    assert!(
        violations.is_empty(),
        "L1 SOUNDNESS: {} engine-legal quer{} rejected by L1 and not in KNOWN_DIVERGENCES \
         (a grammar change dropped a legal construct — fix L1 or, if intentional, \
         document it in KNOWN_DIVERGENCES):\n{}",
        violations.len(),
        if violations.len() == 1 { "y" } else { "ies" },
        violations.join("\n")
    );
}

#[test]
fn enum_ref_operands_stay_admitted() {
    // The one construct L1 must NOT tighten away when rejecting the bare-identifier
    // operand residue (decision B): a qualified or dotted enum/element reference in
    // operand position (`== Type.Meeting`, `== pkg::E.VALUE`) is legal Pure and
    // stays admitted. A future attempt to
    // reject the bare-identifier residue that also broke these would redden here.
    for query in [
        "|X.all()->filter(x|$x.type == Type.Meeting)",
        "|X.all()->filter(x|$x.emp == EmployeeType.CONTRACT)",
        "|X.all()->filter(x|$x.seg == model::domain::ClientSegmentationL1.HEDGE_FUNDS)",
    ] {
        assert!(
            l1_accepts(query),
            "enum-ref operand must stay admitted (decision B): {query}"
        );
    }
}

#[test]
fn the_divergence_allowlist_has_no_stale_entries() {
    // Every allowlisted query must be in the corpus, labeled `parse_ok`, and still
    // actually rejected by L1 — else the allowlist is masking nothing (or masking a
    // now-fixed case) and should shrink.
    let corpus = load_corpus();
    for &query in KNOWN_DIVERGENCES {
        let row = corpus
            .iter()
            .find(|r| r.query == query)
            .unwrap_or_else(|| panic!("KNOWN_DIVERGENCES query not in corpus: {query}"));
        assert_eq!(
            row.legend, "parse_ok",
            "allowlisted query is not engine-legal (so it needs no allowlist entry): {query}"
        );
        assert!(
            !l1_accepts(query),
            "allowlisted query now ACCEPTED by L1 — remove it from KNOWN_DIVERGENCES: {query}"
        );
    }
}
