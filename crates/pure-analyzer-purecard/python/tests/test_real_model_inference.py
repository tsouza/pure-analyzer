"""Real-model inference proof for issue #58: drives an actual causal LM
forward pass, masks its logits token by token through the shipped `purecard`
wheel, and (via the paired Rust `real_model_legend_compile` test) compiles the
constrained output with the pinned Legend stack. This is the literal gap the
issue's own context note names: "no test drives a real model forward pass,
masks logits token by token, finalizes a query, and compiles that exact
result with Legend."

Opt-in only, like `test_session.py`'s hermetic suite is not: this needs the
pinned model weights fetched by `just qwen-infer-model-fetch` (never
committed — constitution §2) and is not part of `just test-python` / `just
ci`. Run via `just real-model-infer` (this file alone) or `just
test-real-model` (this file, then the live Legend compile-check, matching the
`just test-legend` pattern). A missing model directory is a hard failure with
an actionable message, never a silent skip (issue #58 bullet 5).
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest

import purecard

from support.mask_bits import bit_set as _bit_set
from support.real_model import (
    build_prompt_ids,
    build_qwen_vocab,
    generate,
    load_model_and_tokenizer,
    write_generated_queries_jsonl,
    write_traces_jsonl,
)

_REPO_ROOT = Path(__file__).resolve().parents[4]
_CRATE_ROOT = Path(__file__).resolve().parents[2]
_SCHEMAS_DIR = _CRATE_ROOT / "tests" / "fixtures" / "schemas"
_FIXTURES_PATH = Path(__file__).resolve().parent / "fixtures" / "real_model_prompts.json"
_ARTIFACT_DIR = _REPO_ROOT / "target" / "purecard" / "real-model"

MODEL_DIR = os.environ.get(
    "PURECARD_QWEN_INFER_DIR", str(_REPO_ROOT / "target" / "purecard" / "qwen-infer")
)


def _load_fixtures() -> dict:
    return json.loads(_FIXTURES_PATH.read_text(encoding="utf-8"))


def _load_schema_json(db_id: str) -> str:
    return (_SCHEMAS_DIR / f"{db_id}.json").read_text(encoding="utf-8")


@pytest.fixture(scope="module")
def model_bundle():
    if not Path(MODEL_DIR).is_dir():
        raise RuntimeError(
            f"real-model weights not found at {MODEL_DIR!r}. Run `just "
            "qwen-infer-model-fetch` first (this lane never downloads on its own, "
            "and never silently skips — issue #58 bullet 5)."
        )
    tokenizer, model, device = load_model_and_tokenizer(MODEL_DIR)
    vocab_bytes = build_qwen_vocab(tokenizer)
    grammar = purecard.compile_grammar("", vocab_bytes)
    return {
        "tokenizer": tokenizer,
        "model": model,
        "device": device,
        "vocab_bytes": vocab_bytes,
        "grammar": grammar,
    }


def test_byte_vocab_round_trips_the_real_tokenizer(model_bundle):
    """The Python byte-vocab port (`support.byte_decode`, ported from
    `tests/support/byte_decode.rs`) recovers the exact bytes a known gold
    query's real tokens encode to — the soundness prerequisite
    `docs/spec/architecture.md` §9.3 names ("the host is responsible for
    supplying the correct raw bytes per token id")."""
    tokenizer = model_bundle["tokenizer"]
    vocab_bytes = model_bundle["vocab_bytes"]
    text = "|spider::concert_singer::model::default::Concert.all()"
    ids = tokenizer.encode(text, add_special_tokens=False)
    decoded = b"".join(vocab_bytes[i] for i in ids).decode("utf-8")
    assert decoded == text


def test_reset_and_error_propagation_against_the_real_vocabulary(model_bundle):
    """Bullet 2's reset/error-propagation contract, exercised against the real
    151k-token Qwen vocabulary (not a hand-crafted one): a session can be
    reused via `reset()`, and a token the mask clears is rejected by
    `accept_token` without perturbing session state (`ffi.rs`'s documented
    rollback contract). Drives the gold ids directly (no model forward pass
    needed) so this stays fast and independent of model quality — the
    property under test is the binding's contract, not generation quality.
    """
    tokenizer = model_bundle["tokenizer"]
    vocab_bytes = model_bundle["vocab_bytes"]
    grammar = model_bundle["grammar"]
    gold_text = "|spider::pets_1::model::default::Student.all()"
    gold_ids = tokenizer.encode(gold_text, add_special_tokens=False)

    session = purecard.Session(grammar)
    assert not session.is_complete()
    for token_id in gold_ids:
        session.accept_token(token_id)
    assert session.is_complete()

    session.reset()
    assert not session.is_complete()
    for token_id in gold_ids:
        session.accept_token(token_id)
    assert session.is_complete(), "the reset session must accept the same stream again"

    mask_before = session.allowed_mask()
    complete_before = session.is_complete()
    disallowed = next(
        token_id
        for token_id in range(len(vocab_bytes))
        if not _bit_set(mask_before, token_id)
    )
    with pytest.raises(purecard.PureCARDError):
        session.accept_token(disallowed)
    assert session.is_complete() == complete_before
    assert session.allowed_mask() == mask_before, "a rejected token must leave the session untouched"


def test_generation_across_all_fixtures_and_modes(model_bundle):
    """The main proof: for every deterministic fixture, run a real forward
    pass through unconstrained, L1 (grammar-only), and L2 (schema-narrowed)
    inference, preserving raw token ids and mask traces as test artifacts
    (issue #58 bullet 3). L1/L2 use a fresh `Session` per fixture, each built
    with `mode`-appropriate schema selection (bullet 2's "schema selection"),
    reusing the exact committed schema fixtures the Rust L2 suite verifies.
    """
    config = _load_fixtures()
    budgets = config["max_new_tokens"]
    tokenizer = model_bundle["tokenizer"]
    model = model_bundle["model"]
    device = model_bundle["device"]
    vocab_bytes = model_bundle["vocab_bytes"]
    grammar = model_bundle["grammar"]

    results = []
    for fixture in config["fixtures"]:
        prompt_ids = build_prompt_ids(tokenizer, fixture["question"], fixture["class_path"])
        schema_json = _load_schema_json(fixture["db_id"])

        results.append(
            generate(
                model,
                tokenizer,
                device,
                vocab_bytes,
                prompt_ids,
                budgets["unconstrained"],
                fixture["fixture_id"],
                fixture["db_id"],
                fixture["gold_pure_text"],
                "unconstrained",
                session=None,
            )
        )
        results.append(
            generate(
                model,
                tokenizer,
                device,
                vocab_bytes,
                prompt_ids,
                budgets["constrained"],
                fixture["fixture_id"],
                fixture["db_id"],
                fixture["gold_pure_text"],
                "l1",
                session=purecard.Session(grammar),
            )
        )
        results.append(
            generate(
                model,
                tokenizer,
                device,
                vocab_bytes,
                prompt_ids,
                budgets["constrained"],
                fixture["fixture_id"],
                fixture["db_id"],
                fixture["gold_pure_text"],
                "l2",
                session=purecard.Session(grammar, schema_json),
            )
        )

    write_traces_jsonl(_ARTIFACT_DIR / "traces.jsonl", results)
    write_generated_queries_jsonl(_ARTIFACT_DIR / "generated_queries.jsonl", results)

    constrained = [r for r in results if r.mode in ("l1", "l2")]
    truncated = [r for r in constrained if not r.completed]
    if truncated:
        ids = ", ".join(f"{r.fixture_id}/{r.mode}" for r in truncated)
        pytest.fail(
            f"{len(truncated)}/{len(constrained)} constrained generations did not "
            f"reach EOS within their token budget: {ids}. Raise "
            "`max_new_tokens.constrained` in real_model_prompts.json rather than "
            "weakening this assertion."
        )
