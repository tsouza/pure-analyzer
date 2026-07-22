//! Tier-1 synthetic BPE-split soundness lane (adversarial-review B1/B2).
//!
//! The shipped L2 lanes tokenize gold at **lexeme granularity** (`support/lex.rs`),
//! under which every identifier is exactly one token — so the whole-token
//! exact-match narrower trivially admits it. Real byte-level BPE (Qwen2.5-Coder)
//! fragments a schema identifier across several tokens: `countryName` arrives as
//! `country` + `Name`, a classpath source as many chunks, a column string
//! `'MaxRevenue'` as `'` / `Max` / `Revenue` / `'`. This lane reproduces that
//! fragmentation **without a tokenizer dependency** by splitting each gold
//! identifier / classpath / string lexeme into chunks and replaying the chunk-id
//! stream through the real session.
//!
//! It runs two lanes over the same split vocabulary:
//!
//! - **L1** (`DecoderSession::new`) — the pure syntactic mask. Byte-liveness keeps
//!   a partial identifier alive, so every gold chunk is admissible: this lane is
//!   sound today and must stay sound.
//! - **L1+L2** (`DecoderSession::with_schema`) — the schema overlay. A prefix-aware
//!   narrower keeps a token that can still *reach* a legal name; the leading chunk
//!   of every schema identifier must survive exactly as its whole-token form does.
//!
//! Soundness must hold at BPE granularity, not merely at lexeme granularity. If a
//! gold chunk reddens the L1+L2 lane, the narrower is masking a token the model
//! must emit — fix the narrower, never weaken the assertion (§8.6).
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[path = "support/bpe.rs"]
mod bpe;
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
use corpus::load_gold;
use fixture_dbs::FIXTURE_DBS;
use l2::{lex, load_schema};
use purecard::{CompiledGrammar, DecoderSession, Vocab};

/// Total in-scope gold queries (the 8 fixtures) — mirrors `l2_soundness.rs`. A
/// named constant, not a threshold: a mis-count reddens the gate.
const IN_SCOPE_TOTAL: usize = 269;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/gold_queries.jsonl")
}

/// Fragment one gold lexeme into BPE-style chunks whose concatenation is the
/// lexeme's bytes exactly.
///
/// Identifiers / classpaths are split into thirds (so their **leading** chunk is
/// a strict prefix of a schema name); string literals split as `'` / inner / `'`;
/// numbers, dates, operators, keywords, and punctuation stay whole (real BPE does
/// not fragment those in a soundness-relevant way).
fn fragment(lexeme: &[u8]) -> Vec<Vec<u8>> {
    match lexeme.first() {
        Some(b'\'') => split_string(lexeme),
        Some(&b) if b.is_ascii_alphabetic() || b == b'_' => split_thirds(lexeme),
        _ => vec![lexeme.to_vec()],
    }
}

/// Split an identifier's bytes into up to three non-empty chunks at thirds.
fn split_thirds(bytes: &[u8]) -> Vec<Vec<u8>> {
    let n = bytes.len();
    if n < 2 {
        return vec![bytes.to_vec()];
    }
    let a = (n / 3).max(1);
    let b = (2 * n / 3).max(a + 1).min(n);
    let mut chunks = vec![bytes[..a].to_vec(), bytes[a..b].to_vec()];
    if b < n {
        chunks.push(bytes[b..].to_vec());
    }
    chunks
}

/// Split a `'…'` string literal as `'` / inner-thirds / `'` (the N6 column split).
fn split_string(bytes: &[u8]) -> Vec<Vec<u8>> {
    // A well-formed string literal has at least the two surrounding quotes.
    if bytes.len() < 2 || bytes[0] != b'\'' || bytes[bytes.len() - 1] != b'\'' {
        return vec![bytes.to_vec()];
    }
    let inner = &bytes[1..bytes.len() - 1];
    let mut chunks = vec![b"'".to_vec()];
    if !inner.is_empty() {
        chunks.extend(split_thirds(inner));
    }
    chunks.push(b"'".to_vec());
    chunks
}

/// The split token-id stream of `query`: its lexemes fragmented and mapped to
/// dense ids via `ids`.
fn split_ids(query: &str, ids: &BTreeMap<Vec<u8>, u32>) -> Vec<u32> {
    let mut stream = Vec::new();
    for lexeme in lex(query) {
        for chunk in fragment(&lexeme) {
            let id = ids
                .get(&chunk)
                .unwrap_or_else(|| panic!("chunk not in split vocab: {chunk:?}"));
            stream.push(*id);
        }
    }
    stream
}

/// A synthetic BPE vocabulary over the fragmented chunks of `queries`, deduped
/// into dense ids (mirroring `TokenVocab::build`), with EOS one past the last id.
fn build_split_vocab(queries: &[&str]) -> (Vocab, BTreeMap<Vec<u8>, u32>) {
    let mut ids: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for query in queries {
        for lexeme in lex(query) {
            for chunk in fragment(&lexeme) {
                if !ids.contains_key(&chunk) {
                    ids.insert(chunk.clone(), tokens.len() as u32);
                    tokens.push(chunk);
                }
            }
        }
    }
    let eos = tokens.len() as u32;
    (Vocab::from_byte_tokens(tokens, eos), ids)
}

/// Group the in-scope gold `pure_text` by database (so each db's split vocab and
/// schema are built once).
fn in_scope_by_db() -> BTreeMap<String, Vec<String>> {
    let mut by_db: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in load_gold(&corpus_path()).expect("open the committed gold corpus") {
        let record = item.expect("gold corpus line parses");
        if FIXTURE_DBS.contains(&record.db_id.as_str()) {
            by_db
                .entry(record.db_id)
                .or_default()
                .push(record.pure_text);
        }
    }
    by_db
}

#[test]
fn split_chunks_concatenate_to_the_original_query() {
    // The fragmentation must partition every lexeme exactly (no byte dropped or
    // duplicated) or the whole lane is measuring the wrong bytes.
    let sample = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.horsepower == '150')";
    let mut rebuilt = Vec::new();
    for lexeme in lex(sample) {
        for chunk in fragment(&lexeme) {
            assert!(!chunk.is_empty(), "a chunk is never empty");
            rebuilt.extend_from_slice(&chunk);
        }
    }
    assert_eq!(rebuilt, sample.as_bytes());
}

#[test]
fn fragmentation_actually_splits_a_schema_identifier() {
    // Non-vacuity: the lane only exercises B1 if identifiers really fragment.
    assert!(
        fragment(b"countryName").len() >= 2,
        "an identifier must split into multiple chunks"
    );
    // A column string splits around both quotes, keeping each quote its own chunk.
    let col = fragment(b"'MaxRevenue'");
    assert!(
        col.len() >= 3,
        "a column string yields quote / inner / quote"
    );
    assert_eq!(col.first().map(Vec::as_slice), Some(b"'".as_slice()));
    assert_eq!(col.last().map(Vec::as_slice), Some(b"'".as_slice()));
    // The leading chunk of a schema name is a strict prefix — the exact token the
    // whole-lexeme narrower wrongly cleared.
    let lead = &fragment(b"countryName")[0];
    assert!(b"countryName".starts_with(lead.as_slice()) && lead != b"countryName");
}

#[test]
fn l1_lane_streams_every_split_gold_soundly() {
    // L1 (no schema): byte-liveness keeps a partial identifier alive, so every
    // gold chunk is admissible. This lane is sound today and must stay sound.
    let mut total = 0usize;
    let mut failures = Vec::new();
    for (_db, queries) in in_scope_by_db() {
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
        let (vocab, ids) = build_split_vocab(&refs);
        let eos = vocab.len() as u32;
        let grammar = CompiledGrammar::compile(vocab);
        for query in &refs {
            let mut session = DecoderSession::new(&grammar);
            if let Err(reason) = replay_tokens(&mut session, &split_ids(query, &ids), eos) {
                failures.push(format!("{reason}\n  {query}"));
            }
            total += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "L1 BPE soundness:\n{}",
        failures.join("\n")
    );
    assert_eq!(total, IN_SCOPE_TOTAL, "in-scope query count");
}

#[test]
fn l1_l2_lane_streams_every_split_gold_soundly() {
    // L1+L2 (schema overlay): the prefix-aware narrower must keep the leading
    // chunk of every schema identifier / classpath / column string. On the
    // whole-token exact-match narrower this reddens on the first fragmented name.
    let mut total = 0usize;
    let mut failures = Vec::new();
    let mut seen_dbs = BTreeSet::new();
    for (db_id, queries) in in_scope_by_db() {
        seen_dbs.insert(db_id.clone());
        let schema = load_schema(&db_id);
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
        let (vocab, ids) = build_split_vocab(&refs);
        let eos = vocab.len() as u32;
        let grammar = CompiledGrammar::compile(vocab);
        for query in &refs {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone());
            if let Err(reason) = replay_tokens(&mut session, &split_ids(query, &ids), eos) {
                failures.push(format!("[{db_id}] {reason}\n  {query}"));
            }
            total += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "L1+L2 BPE soundness ({} of {} queries failed):\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
    assert_eq!(total, IN_SCOPE_TOTAL, "in-scope query count");
    assert_eq!(
        seen_dbs.len(),
        FIXTURE_DBS.len(),
        "every fixture db exercised"
    );
}

// --- Cross-boundary (lexeme-straddling) merge lane (audit-2 H1/H2) ---------------
//
// The clean lane above fragments *within* a lexeme, so no token ever straddles a
// lexeme boundary. Real Qwen BPE routinely merges across one: a column string's
// closing quote fused to its delimiter (`'MaxRevenue')`), a navigation dot fused
// to the next identifier (`.count`), an open paren fused to a string's opening
// quote (`('`). This pass glues exactly those three seams over the *same* gold
// corpus, so the identical queries run both clean and straddling.

/// The three cross-boundary BPE merge shapes. Each glues the last chunk of one
/// lexeme to the first chunk of the next across a lexeme seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merge {
    /// H1: a `Str`-closing `'` fused to the following `)` or `,` (`')` / `',`).
    CloseQuoteDelim,
    /// H2: a `.` fused to the leading chunk of the next identifier (`.co`).
    DotIdent,
    /// H2: a `(` fused to a string's opening `'` (`('`).
    OpenQuote,
}

/// Every merge shape enabled — the fully-straddling lane.
const ALL_MERGES: &[Merge] = &[Merge::CloseQuoteDelim, Merge::DotIdent, Merge::OpenQuote];

/// One fragmented chunk tagged with its whole source lexeme, so a merge pass can
/// recognise the seams between adjacent lexemes.
struct Tagged {
    bytes: Vec<u8>,
    lex: Vec<u8>,
    first: bool,
    last: bool,
}

/// Fragment `query` into chunks, each tagged with its source lexeme and its
/// position within it — the flat, boundary-aware stream a merge pass folds over.
fn tagged_chunks(query: &str) -> Vec<Tagged> {
    let mut out = Vec::new();
    for lexeme in lex(query) {
        let chunks = fragment(&lexeme);
        let n = chunks.len();
        for (i, chunk) in chunks.into_iter().enumerate() {
            out.push(Tagged {
                bytes: chunk,
                lex: lexeme.clone(),
                first: i == 0,
                last: i + 1 == n,
            });
        }
    }
    out
}

/// Whether a lexeme's bytes begin an identifier.
fn is_ident_lex(lex: &[u8]) -> bool {
    lex.first()
        .is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_')
}

/// Whether the seam between `a` (the last chunk of its lexeme) and `b` (the first
/// chunk of the next) matches an enabled merge shape.
fn seam_matches(a: &Tagged, b: &Tagged, merges: &[Merge]) -> bool {
    merges.iter().any(|m| match m {
        Merge::CloseQuoteDelim => {
            a.lex.first() == Some(&b'\'') && a.bytes == b"'" && (b.lex == b")" || b.lex == b",")
        }
        Merge::DotIdent => a.lex == b"." && is_ident_lex(&b.lex),
        Merge::OpenQuote => a.lex == b"(" && b.lex.first() == Some(&b'\'') && b.bytes == b"'",
    })
}

/// Glue the corpus's fragmented chunks across every enabled seam, yielding a token
/// stream that straddles lexeme boundaries. `merges` empty reproduces the clean
/// (never-straddling) stream exactly, so one axis drives both lanes.
fn merged_tokens(query: &str, merges: &[Merge]) -> Vec<Vec<u8>> {
    let chunks = tagged_chunks(query);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chunks.len() {
        if i + 1 < chunks.len()
            && chunks[i].last
            && chunks[i + 1].first
            && seam_matches(&chunks[i], &chunks[i + 1], merges)
        {
            let mut glued = chunks[i].bytes.clone();
            glued.extend_from_slice(&chunks[i + 1].bytes);
            out.push(glued);
            i += 2;
        } else {
            out.push(chunks[i].bytes.clone());
            i += 1;
        }
    }
    out
}

/// A synthetic BPE vocabulary over the merged tokens of `queries`, deduped into
/// dense ids, EOS one past the last.
fn build_merged_vocab(queries: &[&str], merges: &[Merge]) -> (Vocab, BTreeMap<Vec<u8>, u32>) {
    let mut ids: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for query in queries {
        for tok in merged_tokens(query, merges) {
            if !ids.contains_key(&tok) {
                ids.insert(tok.clone(), tokens.len() as u32);
                tokens.push(tok);
            }
        }
    }
    let eos = tokens.len() as u32;
    (Vocab::from_byte_tokens(tokens, eos), ids)
}

/// The merged token-id stream of `query`.
fn merged_id_stream(query: &str, merges: &[Merge], ids: &BTreeMap<Vec<u8>, u32>) -> Vec<u32> {
    merged_tokens(query, merges)
        .iter()
        .map(|t| {
            *ids.get(t)
                .unwrap_or_else(|| panic!("merged token not in vocab: {t:?}"))
        })
        .collect()
}

#[test]
fn merged_chunks_concatenate_to_the_original_query() {
    // Bytes must be preserved under merging, or the lane measures the wrong bytes.
    let sample = "|spider::car_1::model::default::CarsData.all()\
        ->filter(x|$x.horsepower == '150')";
    let rebuilt: Vec<u8> = merged_tokens(sample, ALL_MERGES).concat();
    assert_eq!(rebuilt, sample.as_bytes());
    // …and the clean projection (no merges) equals the plain fragment stream.
    let clean: Vec<u8> = merged_tokens(sample, &[]).concat();
    assert_eq!(clean, sample.as_bytes());
}

#[test]
fn merging_actually_straddles_lexeme_boundaries() {
    // Non-vacuity: the lane only exercises H1/H2 if tokens really straddle. Over
    // the whole in-scope corpus, each shape must appear at least once.
    let mut close_quote = 0usize;
    let mut dot_ident = 0usize;
    let mut open_quote = 0usize;
    for (_db, queries) in in_scope_by_db() {
        for query in &queries {
            for tok in merged_tokens(query, ALL_MERGES) {
                let quoted = tok.first() == Some(&b'\'');
                if quoted && (tok.last() == Some(&b')') || tok.last() == Some(&b',')) {
                    close_quote += 1;
                } else if tok.first() == Some(&b'.') && tok.len() > 1 {
                    dot_ident += 1;
                } else if tok.first() == Some(&b'(') && tok.last() == Some(&b'\'') {
                    open_quote += 1;
                }
            }
        }
    }
    assert!(close_quote > 0, "H1 close-quote+delim straddle must occur");
    assert!(dot_ident > 0, "H2 dot+ident straddle must occur");
    assert!(open_quote > 0, "H2 open-paren+quote straddle must occur");
}

#[test]
fn l1_merged_lane_streams_every_straddling_gold_soundly() {
    // L1 (no schema): byte-liveness keeps a straddling token alive just as it keeps
    // a fragment alive — this lane is sound today and must stay sound.
    let mut total = 0usize;
    let mut failures = Vec::new();
    for (_db, queries) in in_scope_by_db() {
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
        let (vocab, ids) = build_merged_vocab(&refs, ALL_MERGES);
        let eos = vocab.len() as u32;
        let grammar = CompiledGrammar::compile(vocab);
        for query in &refs {
            let mut session = DecoderSession::new(&grammar);
            if let Err(reason) = replay_tokens(
                &mut session,
                &merged_id_stream(query, ALL_MERGES, &ids),
                eos,
            ) {
                failures.push(format!("{reason}\n  {query}"));
            }
            total += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "L1 cross-boundary soundness:\n{}",
        failures.join("\n")
    );
    assert_eq!(total, IN_SCOPE_TOTAL, "in-scope query count");
}

#[test]
fn l1_l2_merged_lane_streams_every_straddling_gold_soundly() {
    // L1+L2 (schema overlay) at cross-boundary granularity: the killer property.
    // Under the whole-token narrower a merged closing quote (`'col')`) corrupts the
    // N6 emitted-column set and a later column reference is Diverge-cleared (H1);
    // the lexeme-boundary scope walk records the true column bytes, so this greens.
    let mut total = 0usize;
    let mut failures = Vec::new();
    let mut seen_dbs = BTreeSet::new();
    for (db_id, queries) in in_scope_by_db() {
        seen_dbs.insert(db_id.clone());
        let schema = load_schema(&db_id);
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();
        let (vocab, ids) = build_merged_vocab(&refs, ALL_MERGES);
        let eos = vocab.len() as u32;
        let grammar = CompiledGrammar::compile(vocab);
        for query in &refs {
            let mut session = DecoderSession::with_schema(&grammar, schema.clone());
            if let Err(reason) = replay_tokens(
                &mut session,
                &merged_id_stream(query, ALL_MERGES, &ids),
                eos,
            ) {
                failures.push(format!("[{db_id}] {reason}\n  {query}"));
            }
            total += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "L1+L2 cross-boundary soundness ({} of {} queries failed):\n{}",
        failures.len(),
        total,
        failures.join("\n")
    );
    assert_eq!(total, IN_SCOPE_TOTAL, "in-scope query count");
    assert_eq!(
        seen_dbs.len(),
        FIXTURE_DBS.len(),
        "every fixture db exercised"
    );
}

/// Update the "inside a string literal" flag across an accepted token's bytes,
/// honouring `''` doubling — so the coverage probe can attribute a constrained
/// step to the N6 (column) family or the identifier family.
fn advance_in_str(mut in_str: bool, bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            if in_str && bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            in_str = !in_str;
        }
        i += 1;
    }
    in_str
}

/// Count the L2-constraining steps (where the schema overlay clears at least one
/// token L1 admitted) over a gold stream, split into the column (N6) family and
/// the identifier (N1/N2/N3) family. Both sessions share the grammar and accept
/// the same gold ids, so their masks are directly comparable.
fn count_constraining(
    grammar: &CompiledGrammar,
    schema: &purecard::Schema,
    stream: &[u32],
    bytes_of: impl Fn(u32) -> Vec<u8>,
) -> (usize, usize) {
    let mut l1 = DecoderSession::new(grammar);
    let mut l2 = DecoderSession::with_schema(grammar, schema.clone());
    let mut in_str = false;
    let (mut column, mut ident) = (0usize, 0usize);
    for &id in stream {
        let tok = bytes_of(id);
        let constrained = {
            let l2_mask = l2.allowed_mask().clone();
            let l1_mask = l1.allowed_mask();
            l1_mask.iter_ones().any(|bit| !l2_mask.test(bit))
        };
        if constrained {
            if in_str || tok.first() == Some(&b'\'') {
                column += 1;
            } else {
                ident += 1;
            }
        }
        // Accept in both so the two sessions stay in lockstep.
        l1.accept_token(id).expect("gold admissible under L1");
        l2.accept_token(id).expect("gold admissible under L1+L2");
        in_str = advance_in_str(in_str, &tok);
    }
    (column, ident)
}

#[test]
fn h2_coverage_probe_quantifies_rule_firing_under_straddle() {
    // Non-failing probe (audit-2 H2): how often L2 actually constrains, clean vs
    // straddling, split by rule family. Straddle suppresses some firing that no
    // fix can recover (a merged token legitimately spans two anchors), so this
    // reports the *real* serving coverage rather than the optimistic synthetic one.
    let (mut cc_clean, mut ci_clean) = (0usize, 0usize);
    let (mut cc_merged, mut ci_merged) = (0usize, 0usize);
    for (db_id, queries) in in_scope_by_db() {
        let schema = load_schema(&db_id);
        let refs: Vec<&str> = queries.iter().map(String::as_str).collect();

        let (clean_vocab, clean_ids) = build_merged_vocab(&refs, &[]);
        let clean_bytes: Vec<Vec<u8>> = {
            let mut v = vec![Vec::new(); clean_ids.len()];
            for (tok, &id) in &clean_ids {
                v[id as usize] = tok.clone();
            }
            v
        };
        let clean_grammar = CompiledGrammar::compile(clean_vocab);
        for query in &refs {
            let (c, i) = count_constraining(
                &clean_grammar,
                &schema,
                &merged_id_stream(query, &[], &clean_ids),
                |id| clean_bytes[id as usize].clone(),
            );
            cc_clean += c;
            ci_clean += i;
        }

        let (m_vocab, m_ids) = build_merged_vocab(&refs, ALL_MERGES);
        let m_bytes: Vec<Vec<u8>> = {
            let mut v = vec![Vec::new(); m_ids.len()];
            for (tok, &id) in &m_ids {
                v[id as usize] = tok.clone();
            }
            v
        };
        let m_grammar = CompiledGrammar::compile(m_vocab);
        for query in &refs {
            let (c, i) = count_constraining(
                &m_grammar,
                &schema,
                &merged_id_stream(query, ALL_MERGES, &m_ids),
                |id| m_bytes[id as usize].clone(),
            );
            cc_merged += c;
            ci_merged += i;
        }
    }
    // Emit the measured coverage (visible with `--nocapture`).
    println!(
        "H2 coverage probe (constraining steps):\n  \
         clean : column(N6)={cc_clean:>4}  ident(N1/N2/N3)={ci_clean:>4}\n  \
         merged: column(N6)={cc_merged:>4}  ident(N1/N2/N3)={ci_merged:>4}\n  \
         retained: column={:.0}%  ident={:.0}%",
        pct(cc_merged, cc_clean),
        pct(ci_merged, ci_clean),
    );
    // Sanity, not a threshold: clean L2 must fire somewhere (the corpus has N6 and
    // member/source narrowing), and the fix must retain some firing under straddle.
    assert!(cc_clean + ci_clean > 0, "clean L2 must constrain somewhere");
    assert!(
        cc_merged + ci_merged > 0,
        "the boundary fix must retain L2 firing under straddle"
    );
}

/// Percentage `num/den` (0 when `den` is 0), for the coverage report.
fn pct(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        100.0 * num as f64 / den as f64
    }
}
