//! Tier-B: the fused-nav-dot precision fixture extractor and anti-rot diff.
//!
//! Runs ONLY under `--features fused-extract` (heavy: loads the ACTUAL Qwen2.5-Coder
//! and GPT-4/cl100k_base byte-level BPE tokenizers). For each seeded case it encodes
//! the legal/phantom member navigation with the real tokenizer, locates the single
//! token that fuses the navigation `.` with the member's first character, and
//! rebuilds the committed fixture row. In the default (verify) mode it diffs the
//! re-extraction against `tests/fixtures/tokenizers/fused_precision.jsonl` and fails
//! on any drift — so a tokenizer change or a stale fixture is caught out-of-band by
//! the nightly `qwen-oracle.yml` workflow, never silently. With
//! `WRITE_FUSED_FIXTURE=1` it regenerates the committed fixture instead (how the
//! vendored file is produced in the first place).
//!
//! This is the extraction backstop for the hermetic Tier-A replay
//! (`fused_tokenizer_precision.rs`), which reads the vendored fixture with no
//! tokenizer dependency and no network — the per-PR precision gate.
//!
//! GPT-4's cl100k_base stands in for the "GPT-family" byte-level BPE tokenizer: the
//! *classic* GPT-2 pre-tokenizer isolates a lone `.` from the following letters, so
//! it structurally cannot fuse a nav dot with a member and proves nothing about
//! fused-token precision. cl100k_base is the same byte-unicode lineage evolved to
//! the code-aware regex that *does* fuse (`.theme`), so Qwen2.5-Coder + cl100k_base
//! are two independent real tokenizers that both exercise the fused decision point.
#![cfg(feature = "fused-extract")]
#![forbid(unsafe_code)]

use std::path::PathBuf;

use tokenizers::Tokenizer;

#[path = "support/byte_decode.rs"]
mod byte_decode;
#[path = "support/fused_fixture.rs"]
mod fused_fixture;

use byte_decode::{gpt2_byte_decoder, true_bytes};
use fused_fixture::{Expect, FusedCase};

/// A real byte-level-BPE tokenizer the fixture is extracted from, with the immutable
/// model-repo revision it is pinned to and the env var carrying its `tokenizer.json`
/// path (the `just fused-tokenizers` recipe fetches each into `target/`).
struct TokSpec {
    name: &'static str,
    revision: &'static str,
    env: &'static str,
}

/// Qwen2.5-Coder-7B-Instruct — the same pin the real-Qwen oracle uses.
const QWEN: TokSpec = TokSpec {
    name: "qwen",
    revision: "c03e6d358207e414f1eca0bb1891e29f1db0e242",
    env: "QWEN_TOKENIZER_JSON",
};

/// GPT-4's cl100k_base, mirrored as an HF `tokenizer.json` by `Xenova/gpt-4`.
const GPT4: TokSpec = TokSpec {
    name: "gpt4",
    revision: "1d9f1f1b1fae88c0e4df1dab0a397f8de6229075",
    env: "GPT4_TOKENIZER_JSON",
};

/// Every tokenizer the fixture spans — both must fuse the nav dot.
const TOKENIZERS: &[&TokSpec] = &[&QWEN, &GPT4];

/// One seeded case, tokenizer-independent: it is extracted through *every*
/// tokenizer, so both names appear in the fixture with the same coverage.
struct Seed {
    db: &'static str,
    /// The legal partial query up to (excluding) the nav dot — its last token is a
    /// bound variable, so the coming `.<member>` is a member navigation.
    prefix: &'static str,
    /// The member spelled after the dot. For an admit case a real navigable member;
    /// for a mask case a phantom whose first char begins no navigable member.
    member: &'static str,
    expect: Expect,
    note: &'static str,
}

/// The seeds. Balanced across three schemas and mask/admit, including an
/// association-end admit (a navigable member that is *not* a scalar property). Each
/// mask member's first character is verified absent from the class's navigable set
/// (scalar properties ∪ association ends) so it is a genuine phantom, not an
/// over-approximation the decoder legitimately admits.
const SEEDS: &[Seed] = &[
    Seed {
        db: "concert_singer",
        prefix: "|spider::concert_singer::model::default::Concert.all()->filter(c|$c",
        member: "theme",
        expect: Expect::Admit,
        note: "Concert.theme: a real scalar property fused with the nav dot stays admissible",
    },
    Seed {
        db: "concert_singer",
        prefix: "|spider::concert_singer::model::default::Concert.all()->filter(c|$c",
        member: "fk0DefaultStadium",
        expect: Expect::Admit,
        note: "Concert.fk0DefaultStadium: an association-end navigation fused with the nav dot stays admissible",
    },
    Seed {
        db: "concert_singer",
        prefix: "|spider::concert_singer::model::default::Concert.all()->filter(c|$c",
        member: "maker",
        expect: Expect::Mask,
        note: "Concert.maker: 'm' begins no Concert member, so the fused phantom is masked",
    },
    Seed {
        db: "car_1",
        prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x",
        member: "cylinders",
        expect: Expect::Admit,
        note: "CarsData.cylinders: a real scalar property fused with the nav dot stays admissible",
    },
    Seed {
        db: "car_1",
        prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x",
        member: "horsepower",
        expect: Expect::Admit,
        note: "CarsData.horsepower: a real scalar property fused with the nav dot stays admissible",
    },
    Seed {
        db: "car_1",
        prefix: "|spider::car_1::model::default::CarsData.all()->filter(x|$x",
        member: "sallary",
        expect: Expect::Mask,
        note: "CarsData.sallary: 's' begins no CarsData member, so the fused phantom is masked",
    },
    Seed {
        db: "world_1",
        prefix: "|spider::world_1::model::default::Country.all()->filter(x|$x",
        member: "population",
        expect: Expect::Admit,
        note: "Country.population: a real scalar property fused with the nav dot stays admissible",
    },
    Seed {
        db: "world_1",
        prefix: "|spider::world_1::model::default::Country.all()->filter(x|$x",
        member: "mayor",
        expect: Expect::Mask,
        note: "Country.mayor: 'm' begins no Country member, so the fused phantom is masked",
    },
];

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tokenizers/fused_precision.jsonl")
}

fn load_tokenizer(spec: &TokSpec) -> Tokenizer {
    let path = std::env::var(spec.env).unwrap_or_else(|_| {
        panic!(
            "set {} to {}'s tokenizer.json path (run via `just fused-tokenizers`)",
            spec.env, spec.name
        )
    });
    Tokenizer::from_file(&path).unwrap_or_else(|e| panic!("load {path}: {e}"))
}

/// Extract one fixture row: encode `prefix.member`, verify the byte-level tokens
/// reconstruct the text exactly, and find the token that fuses the nav dot with the
/// member's first char (the first token after the prefix's exact token boundary).
fn extract(spec: &TokSpec, tok: &Tokenizer, seed: &Seed) -> FusedCase {
    let dec = gpt2_byte_decoder();
    let text = format!("{}.{}", seed.prefix, seed.member);
    let encoding = tok
        .encode(text.as_str(), false)
        .unwrap_or_else(|e| panic!("{} encodes {text:?}: {e}", spec.name));
    let token_strings: Vec<String> = encoding.get_tokens().to_vec();

    // The byte-level tokens must reconstruct the text exactly, or the decoder would
    // be fed bytes the model never emitted.
    let rebuilt: Vec<u8> = token_strings
        .iter()
        .flat_map(|t| true_bytes(t, &dec))
        .collect();
    assert_eq!(
        rebuilt,
        text.as_bytes(),
        "{} token bytes do not reconstruct {text:?}",
        spec.name
    );

    // Walk to the exact prefix token boundary; the next token is the fused dot.
    let prefix_bytes = seed.prefix.as_bytes();
    let mut acc: Vec<u8> = Vec::new();
    let mut fused_index = None;
    for (i, t) in token_strings.iter().enumerate() {
        if acc == prefix_bytes {
            let tb = true_bytes(t, &dec);
            assert!(
                tb.first() == Some(&b'.') && tb.len() >= 2,
                "{} does not fuse the nav dot for {:?}: token at the boundary is {t:?} — pick a different member/prefix",
                spec.name,
                seed.note
            );
            fused_index = Some(i);
            break;
        }
        acc.extend_from_slice(&true_bytes(t, &dec));
    }
    let fused_index = fused_index.unwrap_or_else(|| {
        panic!(
            "{} has no token boundary at the end of the prefix for {:?} (the last prefix char fused with the dot) — pick a different prefix",
            spec.name, seed.note
        )
    });

    FusedCase {
        tokenizer: spec.name.to_owned(),
        revision: spec.revision.to_owned(),
        db: seed.db.to_owned(),
        prefix: seed.prefix.to_owned(),
        token_strings,
        fused_index,
        fused_expect: seed.expect,
        note: seed.note.to_owned(),
    }
}

/// Re-extract every row (tokenizer-major, then seed order) as JSONL text.
fn regenerate() -> String {
    let mut lines = Vec::new();
    for spec in TOKENIZERS {
        let tok = load_tokenizer(spec);
        for seed in SEEDS {
            let case = extract(spec, &tok, seed);
            lines.push(serde_json::to_string(&case).expect("serialize fused case"));
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

#[test]
fn fused_fixture_matches_the_real_tokenizers() {
    let regenerated = regenerate();
    let path = fixture_path();

    if std::env::var("WRITE_FUSED_FIXTURE").is_ok() {
        std::fs::write(&path, &regenerated)
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        return;
    }

    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read committed fixture {}: {e}", path.display()));
    if regenerated == committed {
        return;
    }

    // Report the first drifting row for a legible failure.
    let drift = regenerated
        .lines()
        .zip(committed.lines())
        .enumerate()
        .find(|(_, (a, b))| a != b);
    let detail = match drift {
        Some((i, (a, b))) => format!("first drift at row {i}:\n  committed: {b}\n  extracted: {a}"),
        None => format!(
            "row count changed: committed {}, extracted {}",
            committed.lines().count(),
            regenerated.lines().count()
        ),
    };
    panic!(
        "fused-precision fixture drift: re-extraction from the real tokenizers differs from the committed fixture.\n{detail}\nRegenerate with `just fused-tokenizers-write` and review the diff."
    );
}
