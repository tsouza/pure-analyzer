//! The shared row type for the fused-nav-dot precision fixture
//! (`tests/fixtures/tokenizers/fused_precision.jsonl`).
//!
//! One row is one real-tokenizer case: the byte-level token strings a real
//! byte-level-BPE tokenizer emits for a legal/phantom member navigation, with the
//! index of the single token that fuses the navigation `.` with the member's first
//! character. The hermetic Tier-A replay (`fused_tokenizer_precision.rs`, default
//! features, no tokenizer crate) deserializes it and drives it through the shipped
//! decoder; the feature-gated Tier-B extractor (`fused_tokenizer_extract.rs`)
//! serializes it from the *actual* tokenizers and diffs, so the fixture cannot rot.
//! Both lanes share this one definition (constitution §4, DRY).

use serde::{Deserialize, Serialize};

/// What the decoder must do with the fused `.`+char token at `fused_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// The fused token's leading char begins no navigable member (scalar property
    /// nor association end), so the schema overlay must clear its vocab bit.
    Mask,
    /// The fused token is a real member navigation, so it must stay admissible.
    Admit,
}

/// One committed fused-precision case. Field order is the on-disk JSON order
/// (serde preserves declaration order), so a regenerated line is byte-identical to
/// its committed twin and the anti-rot diff is a plain string compare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusedCase {
    /// The tokenizer that produced this row (`qwen` or `gpt4`).
    pub tokenizer: String,
    /// The immutable model-repo revision the tokenizer was fetched at.
    pub revision: String,
    /// The schema fixture under `tests/fixtures/schemas` this case navigates.
    pub db: String,
    /// The legal partial query the tokens spell up to (but excluding) the nav dot.
    pub prefix: String,
    /// The real tokenizer's byte-level token strings for `prefix` + `.` + member.
    pub token_strings: Vec<String>,
    /// The index in `token_strings` of the token fusing `.` with the member's first
    /// char — the decision point the decoder is probed at.
    pub fused_index: usize,
    /// Whether that fused token must be masked or admitted.
    pub fused_expect: Expect,
    /// A human-readable description of what the case exercises.
    pub note: String,
}
