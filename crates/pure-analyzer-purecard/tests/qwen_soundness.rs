//! Real-Qwen L2 soundness oracle (audit-2 C1) — the gold-standard check.
//!
//! Runs ONLY under `--features qwen-oracle` (heavy: loads the actual Qwen2.5-Coder
//! tokenizer and replays the whole gold corpus token-by-token through the real
//! byte-level BPE). It is a `just qwen-oracle` local/on-demand gate and the
//! nightly `qwen-oracle.yml` workflow, **not** a per-PR gate. This is what the
//! synthetic `bpe_split_soundness` reproducer
//! approximates: it asserts L2 stays sound against the *actual* tokenizer merge
//! boundaries (the H1/H2 class), where a token can straddle a lexeme boundary
//! (`'MaxRevenue')`, `.count`) in ways a lex-then-split proxy never produces.
//!
//! Set `QWEN_TOKENIZER_JSON` to the tokenizer.json path (the `just qwen-oracle`
//! recipe fetches it into `target/qwen/`).
#![cfg(feature = "qwen-oracle")]
#![forbid(unsafe_code)]

use std::path::PathBuf;

use tokenizers::Tokenizer;

use purecard::{CompiledGrammar, DecoderSession, Vocab};

#[path = "support/bpe.rs"]
mod bpe;
#[path = "support/byte_decode.rs"]
mod byte_decode;
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

use bpe::replay_tokens;
use byte_decode::{gpt2_byte_decoder, true_bytes};
use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::load_schema;

/// Qwen's in-vocab stop ids (M2): the model's real EOS ids live *inside* the
/// vocabulary, distinct from PureCARD's reserved EOS bit at `vocab.len()`.
const QWEN_ENDOFTEXT: u32 = 151643;
const QWEN_IM_END: u32 = 151645;

/// The number of ids to fold before probing the M2 special-admissibility
/// invariant — enough to leave the Start state and be mid-query.
const MID_QUERY_STEPS: usize = 2;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Build a `Vocab` over the real Qwen vocabulary in id order, mapping each id to
/// its true emitted bytes. EOS is the reserved bit at `vocab.len()` (M2: the host
/// maps the model's in-vocab stop id onto this bit; no real stop id is placed in
/// the byte table).
fn build_qwen_vocab(tok: &Tokenizer) -> Vocab {
    let dec = gpt2_byte_decoder();
    let vocab_map = tok.get_vocab(true); // String -> id (incl. added/special)
    let size = vocab_map.len();
    // `get_vocab` returns a HashMap, so id contiguity is not a contract. A holey
    // id space would leave empty byte slots and silently poison the oracle's
    // ground truth, so fail fast on any out-of-range, duplicate, or unfilled id.
    let mut tokens: Vec<Option<Vec<u8>>> = vec![None; size];
    for (s, id) in vocab_map {
        let idx = id as usize;
        assert!(
            idx < size,
            "Qwen vocab id {idx} >= size {size}: non-dense id space breaks the oracle"
        );
        assert!(tokens[idx].is_none(), "duplicate Qwen vocab id {idx}");
        tokens[idx] = Some(true_bytes(&s, &dec));
    }
    let tokens: Vec<Vec<u8>> = tokens
        .into_iter()
        .enumerate()
        .map(|(idx, t)| {
            t.unwrap_or_else(|| {
                panic!("Qwen vocab id {idx} unfilled: holey id space poisons the oracle")
            })
        })
        .collect();
    let eos = size as u32;
    Vocab::from_byte_tokens(tokens, eos)
}

/// Tokenize one query with the real tokenizer (no chat-template specials).
fn encode(tok: &Tokenizer, text: &str) -> Vec<u32> {
    tok.encode(text, false)
        .expect("real tokenizer encodes gold Pure")
        .get_ids()
        .to_vec()
}

fn load_tokenizer() -> Tokenizer {
    let path = std::env::var("QWEN_TOKENIZER_JSON")
        .expect("set QWEN_TOKENIZER_JSON to the tokenizer.json path (run via `just qwen-oracle`)");
    Tokenizer::from_file(&path).unwrap_or_else(|e| panic!("load {path}: {e}"))
}

#[test]
fn l1_streams_every_real_qwen_gold_soundly() {
    let tok = load_tokenizer();
    let vocab = build_qwen_vocab(&tok);
    let eos = vocab.eos();
    let grammar = CompiledGrammar::from_spec("", vocab);

    let mut total = 0usize;
    let mut failures = Vec::new();
    for item in load_gold(&corpus_path()).expect("open gold corpus") {
        let record = item.expect("gold line parses");
        let ids = encode(&tok, &record.pure_text);
        let mut sess = DecoderSession::new(&grammar);
        if let Err(why) = replay_tokens(&mut sess, &ids, eos) {
            failures.push(format!(
                "{} [{}]: {why}",
                record.source_id, record.pure_text
            ));
        }
        total += 1;
    }
    println!(
        "L1 real-Qwen: {total} gold queries replayed, {} masked",
        failures.len()
    );
    assert!(
        failures.is_empty(),
        "L1 masked a gold token under the real Qwen tokenizer ({} cases):\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn l1_l2_streams_every_in_scope_real_qwen_gold_soundly() {
    let tok = load_tokenizer();
    let vocab = build_qwen_vocab(&tok);
    let grammar = CompiledGrammar::from_spec("", vocab);

    let mut total = 0usize;
    let mut narrowed = 0usize; // coarse H2 coverage: L2 cleared >=1 bit at some step
    let mut failures = Vec::new();
    for item in load_gold(&corpus_path()).expect("open gold corpus") {
        let record = item.expect("gold line parses");
        if !FIXTURE_DBS.contains(&record.db_id.as_str()) {
            continue;
        }
        let ids = encode(&tok, &record.pure_text);
        let schema = load_schema(&record.db_id);
        let mut l1 = DecoderSession::new(&grammar);
        let mut l2 = DecoderSession::with_schema(&grammar, schema);
        let mut did_narrow = false;
        let mut failed = None;
        for (step, &id) in ids.iter().enumerate() {
            let l1_admits = l1.allowed_mask().test(id);
            let l2_admits = l2.allowed_mask().test(id);
            // Coarse coverage: any step where the schema mask is a strict subset.
            if l1_admits && !l2_narrows_equal(&mut l1, &mut l2) {
                did_narrow = true;
            }
            if l1_admits && !l2_admits {
                failed = Some(format!("step {step} id {id}: L1 admits, L2 masks"));
                break;
            }
            let _ = l1.accept_token(id);
            if l2.accept_token(id).is_err() {
                failed = Some(format!("step {step} id {id}: L2 rejected"));
                break;
            }
        }
        if let Some(why) = failed {
            failures.push(format!(
                "{} [{}]: {why}",
                record.source_id, record.pure_text
            ));
        } else if !l2.is_complete() {
            failures.push(format!(
                "{} [{}]: L2 incomplete at EOS",
                record.source_id, record.pure_text
            ));
        }
        if did_narrow {
            narrowed += 1;
        }
        total += 1;
    }
    println!(
        "L1+L2 real-Qwen: {total} in-scope gold replayed, {narrowed} had L2 narrowing, {} masked",
        failures.len(),
    );
    assert!(
        failures.is_empty(),
        "L1+L2 masked a gold token under the real Qwen tokenizer ({} cases):\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// Whether the two masks agree at the current step (a cheap narrowing probe:
/// unequal means L2 cleared at least one bit).
fn l2_narrows_equal(l1: &mut DecoderSession<'_>, l2: &mut DecoderSession<'_>) -> bool {
    l1.allowed_mask() == l2.allowed_mask()
}

#[test]
fn qwen_specials_are_never_admissible_mid_query() {
    // M2: the in-vocab stop ids and other specials must never be admissible inside
    // a query — only the reserved EOS bit at vocab.len() signals completion. Their
    // literal `<|...|>` bytes dead-end the byte-PDA, so their vocab bit stays clear.
    let tok = load_tokenizer();
    let vocab = build_qwen_vocab(&tok);
    let eos = vocab.eos();
    let grammar = CompiledGrammar::from_spec("", vocab);

    // Drive a real gold query a couple of steps in, then probe.
    let sample = load_gold(&corpus_path())
        .expect("open gold corpus")
        .filter_map(Result::ok)
        .find(|r| r.arm == "A")
        .expect("an arm-A gold query");
    let ids = encode(&tok, &sample.pure_text);
    let mut sess = DecoderSession::new(&grammar);
    for &id in ids.iter().take(MID_QUERY_STEPS) {
        assert!(sess.allowed_mask().test(id), "gold prefix admissible");
        sess.accept_token(id).expect("accept gold prefix");
    }
    assert!(!sess.is_complete(), "still mid-query");
    let mask = sess.allowed_mask();
    assert!(
        !mask.test(QWEN_ENDOFTEXT),
        "<|endoftext|> must not be admissible mid-query"
    );
    assert!(
        !mask.test(QWEN_IM_END),
        "<|im_end|> must not be admissible mid-query"
    );
    // EOS is the reserved bit, distinct from every in-vocab id.
    assert_ne!(eos, QWEN_ENDOFTEXT);
    assert_ne!(eos, QWEN_IM_END);
}
