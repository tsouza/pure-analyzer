//! Per-step masking benchmarks (`docs/spec/architecture.md` §4, G3) — the shipped
//! M5 performance baseline locked behind the CodSpeed regression guard.
//!
//! The mask is consumed on the model's critical path — one call per generated
//! token over a ~150k vocabulary — so it must never bottleneck the forward pass:
//! the target is **≤ a few hundred µs/token** (Decision D4). Everything except the
//! timed routine (the `CompiledGrammar`, `Vocab`, `Schema`, and any prefix drive)
//! is built once outside the loop, and the per-state cache is pre-warmed unless a
//! bench deliberately measures its cold build.
//!
//! Four families:
//!
//! - **`allowed_mask`** — steady-state per-step mask at three configurations
//!   (shallow / deep-stack / identifier-position);
//! - **`accept_token`** — per-step whole-token advance;
//! - **`cache_win`** — cold first-visit partition build vs warm cached copy (the
//!   M2 win);
//! - **`l2_overhead`** — `with_schema` vs `new` at an identifier position (the
//!   L2-vs-L1 narrowing delta).
#![forbid(unsafe_code)]

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use purecard::{ByteRecognizer, CompiledGrammar, DecoderSession, Schema};

#[path = "../tests/support/l2.rs"]
mod l2;
#[path = "../tests/support/lex.rs"]
mod lex;
#[path = "../tests/support/synth.rs"]
mod synth;

use synth::synthetic_vocab;

/// A production-scale synthetic vocabulary — the ~150k tokens the budget math
/// (§4) is stated against.
const VOCAB_SIZE: usize = 150_000;

/// Byte prefixes that drive the session to each measured configuration.
const SHALLOW_PREFIX: &[u8] = b"|X.all()";
const DEEP_PREFIX: &[u8] = b"|X.all()->groupBy([agg(x|$x.v,";
const IDENT_PREFIX: &[u8] = b"|X.all()->";

/// The id of the single-space token in [`synthetic_vocab`]: the space is at
/// index 6 of the walker `ALPHABET`, and the vocabulary enumerates the length-1
/// tokens first in alphabet order, so token 6 is `b" "`. Accepting it from
/// [`AfterValue`] over an empty stack keeps the session in `AfterValue`, so it can
/// be fed repeatedly to measure a per-step token advance without dead-ending.
const SPACE_TOKEN_ID: u32 = 6;

/// A fixture database with a committed schema, and one of its arm-C gold queries,
/// for the L2 overhead bench. Hardcoding one gold query (rather than loading the
/// corpus) keeps the bench free of the oracle harness's corpus loader — and its
/// test submodule's `cfg(test)` imports, which `cargo bench` would flag.
const L2_BENCH_DB: &str = "concert_singer";
const L2_BENCH_QUERY: &str = "|spider::concert_singer::model::default::Stadium.all()\
    ->project([x|$x.capacity, x|$x.average], ['max_capacity','average'])\
    ->sort('max_capacity', SortDirection.DESC)->take(1)";

fn drive<'g>(grammar: &'g CompiledGrammar, prefix: &[u8]) -> DecoderSession<'g> {
    let mut session = DecoderSession::new(grammar);
    for &byte in prefix {
        session
            .accept_byte(byte)
            .expect("benchmark prefix must be live");
    }
    // Pre-warm the lazy per-state cache so the loop measures the steady-state
    // per-step cost, not the one-time first-visit build (Risk R2).
    let _ = session.allowed_mask();
    session
}

fn bench_allowed_mask(c: &mut Criterion) {
    let grammar = CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE));

    let mut group = c.benchmark_group("allowed_mask");
    for (name, prefix) in [
        ("shallow", SHALLOW_PREFIX),
        ("deep_stack", DEEP_PREFIX),
        ("identifier_position", IDENT_PREFIX),
    ] {
        let mut session = drive(&grammar, prefix);
        group.bench_function(name, |b| {
            b.iter(|| {
                let mask = black_box(&mut session).allowed_mask();
                black_box(mask.len())
            });
        });
    }
    group.finish();
}

fn bench_accept_token(c: &mut Criterion) {
    let grammar = CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE));
    // Drive past the source to `AfterValue`, where a whitespace token is a stable
    // self-loop — repeated accepts stay in `AfterValue`, isolating the per-token
    // advance cost.
    let mut session = drive(&grammar, SHALLOW_PREFIX);
    let mut group = c.benchmark_group("accept_token");
    group.bench_function("whitespace_step", |b| {
        b.iter(|| {
            black_box(&mut session)
                .accept_token(SPACE_TOKEN_ID)
                .expect("whitespace is a self-loop at AfterValue");
        });
    });
    group.finish();
}

fn bench_cache_win(c: &mut Criterion) {
    let warm_grammar = CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE));
    let mut warm_session = drive(&warm_grammar, SHALLOW_PREFIX);

    let mut group = c.benchmark_group("cache_win");
    // Warm: the state's partition is already cached, so a step is a word-wise copy
    // plus the (empty) deferred re-probe.
    group.bench_function("warm", |b| {
        b.iter(|| {
            let mask = black_box(&mut warm_session).allowed_mask();
            black_box(mask.len())
        });
    });
    // Cold: a fresh grammar whose cache is empty, so the first `allowed_mask` at
    // the shallow state must *build* the partition (probe all ~150k tokens). The
    // vocabulary + grammar construction is the unmeasured `iter_batched` setup;
    // only the drive-and-first-mask is timed. This quantifies the M2 win: cold is
    // the partition build, warm is the cached copy.
    group.sample_size(10);
    group.bench_function("cold", |b| {
        b.iter_batched(
            || CompiledGrammar::compile(synthetic_vocab(VOCAB_SIZE)),
            |grammar| {
                let mut session = DecoderSession::new(&grammar);
                for &byte in SHALLOW_PREFIX {
                    session.accept_byte(byte).expect("prefix must be live");
                }
                black_box(session.allowed_mask().len())
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn bench_l2_overhead(c: &mut Criterion) {
    let schema = l2::load_schema(L2_BENCH_DB);
    let token_vocab = l2::TokenVocab::build(&[L2_BENCH_QUERY], &[]);
    let grammar = CompiledGrammar::compile(token_vocab.vocab());
    // Lex the query and find its first `->` token, so both sessions are measured
    // at the same identifier (step-method) position — where L2 narrowing is active.
    let tokens = l2::lex(L2_BENCH_QUERY);
    let arrow_step = tokens
        .iter()
        .position(|t| t.as_slice() == b"->")
        .expect("a gold pipeline has a `->` step");

    // Drive a session (schema-aware or not) up to and including the first `->`,
    // pre-warming the mask, then return it positioned for the timed `allowed_mask`.
    let drive_to_arrow = |schema: Option<Schema>| {
        let mut session = match schema {
            Some(s) => DecoderSession::with_schema(&grammar, s),
            None => DecoderSession::new(&grammar),
        };
        for token in tokens.iter().take(arrow_step + 1) {
            let id = token_vocab
                .id_of(token)
                .expect("first query's token is in its own vocab");
            session
                .accept_token(id)
                .expect("gold prefix token is admissible");
        }
        let _ = session.allowed_mask();
        session
    };

    let mut l1_session = drive_to_arrow(None);
    let mut l2_session = drive_to_arrow(Some(schema.clone()));

    let mut group = c.benchmark_group("l2_overhead");
    group.bench_function("l1_new", |b| {
        b.iter(|| black_box(black_box(&mut l1_session).allowed_mask().len()));
    });
    group.bench_function("l2_with_schema", |b| {
        b.iter(|| black_box(black_box(&mut l2_session).allowed_mask().len()));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_allowed_mask,
    bench_accept_token,
    bench_cache_win,
    bench_l2_overhead
);
criterion_main!(benches);
