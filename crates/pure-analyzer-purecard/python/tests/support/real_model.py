"""Real-model inference harness for PureCARD (issue #58).

This is the host-side integration `docs/spec/architecture.md` §9.3 describes as
"out of scope to build" for the Rust crate: it loads a real causal LM and its
tokenizer, builds the byte-token vocabulary `compile_grammar` needs (via
:mod:`support.byte_decode`, the exact same GPT-2 byte table the Rust Qwen
oracle uses), and drives the documented per-step loop — mask, sample, accept,
map the model's real stop ids onto PureCARD's reserved EOS bit — against the
shipped `purecard` wheel.

Greedy decoding only: deterministic by construction (no RNG to seed), which is
what "deterministic prompt/query fixtures" (issue #58 bullet 3) needs. A future
top-p lane would need an explicit seeded RNG threaded through here.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

import numpy as np
import torch

import purecard

from .byte_decode import gpt2_byte_decoder, true_bytes

# Qwen's in-vocab stop ids (mirrors `tests/qwen_soundness.rs`'s `QWEN_ENDOFTEXT`
# / `QWEN_IM_END`): the model's real EOS ids live *inside* the vocabulary,
# distinct from PureCARD's reserved EOS bit at `vocab_len`. Verified live
# against the pinned tokenizer/config in this harness's own tests.
QWEN_ENDOFTEXT = 151643
QWEN_IM_END = 151645

Mode = Literal["unconstrained", "l1", "l2"]


def build_qwen_vocab(tokenizer) -> tuple[list[bytes], int]:
    """Build the ``vocab_bytes``/``eos_id`` pair ``compile_grammar`` needs from
    a real tokenizer: token id -> true emitted bytes, decoded through the
    shared GPT-2 byte table. Mirrors `tests/qwen_soundness.rs::build_qwen_vocab`
    (constitution §4, DRY) — same dense-id-space validation, same convention
    that EOS is the reserved bit at ``vocab_len``, not any in-vocab id.
    """
    decoder = gpt2_byte_decoder()
    vocab_map = tokenizer.get_vocab()
    size = len(vocab_map)
    tokens: list[bytes | None] = [None] * size
    for token_str, token_id in vocab_map.items():
        if not 0 <= token_id < size:
            raise ValueError(f"tokenizer id {token_id} >= vocab size {size}: non-dense id space")
        if tokens[token_id] is not None:
            raise ValueError(f"duplicate tokenizer id {token_id}")
        tokens[token_id] = true_bytes(token_str, decoder)
    filled: list[bytes] = []
    for index, entry in enumerate(tokens):
        if entry is None:
            raise ValueError(f"tokenizer id {index} unfilled: holey id space")
        filled.append(entry)
    return filled, size


# Fixed textual "reasoning" that ends every prompt, never generated and never
# masked — the mode switch point. `docs/spec/architecture.md` §9.3: "activate
# constraint only over the final-query span... not over tool calls or
# reasoning text." Keeping this fixed (not model-generated) keeps the fixture
# fully deterministic: the query-span continuation always starts from the
# exact same prompt token ids across every mode and every run.
_REASONING_PREAMBLE = "I will answer using {class_path}.\nFinal Pure query:\n"

_SYSTEM_PROMPT = (
    "You translate questions into Legend Pure queries. A Pure query that "
    "returns every instance of a class is written `|<full::class::Path>.all()` "
    "— nothing else. For example, for the class `demo::model::Widget` and the "
    "question 'List every widget.', the final query is "
    "`|demo::model::Widget.all()`.\n"
    "Answer with a short plan, then the exact query after the line "
    "'Final Pure query:'."
)


def build_prompt_ids(tokenizer, question: str, class_path: str) -> list[int]:
    """The fixed prompt token ids for one fixture: a chat-template system/user
    turn plus the fixed reasoning preamble, ending exactly at the final-query
    span's first token. Identical across every mode for a given fixture.
    """
    messages = [
        {"role": "system", "content": _SYSTEM_PROMPT},
        {"role": "user", "content": f"{question} (class: {class_path})"},
    ]
    prefix = tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    prefix += _REASONING_PREAMBLE.format(class_path=class_path)
    return tokenizer.encode(prefix, add_special_tokens=False)


def load_model_and_tokenizer(model_dir: str):
    """Load the pinned model + tokenizer from a local snapshot directory
    (populated by ``just qwen-infer-model-fetch``), on the best available
    device. Evaluation mode, `bfloat16` per the pinned config's own declared
    dtype (`config.json`'s `torch_dtype`) so results match the shipped weights.
    """
    from transformers import AutoModelForCausalLM, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(model_dir)
    device = "mps" if torch.backends.mps.is_available() else "cpu"
    model = AutoModelForCausalLM.from_pretrained(
        model_dir, dtype=torch.bfloat16 if device == "mps" else torch.float32
    )
    model.to(device)
    model.eval()
    return tokenizer, model, device


@dataclass
class MaskStep:
    """One decode step's mask signature: cheap enough to keep every step
    (`allowed_count` for a quick eyeball) while `mask_sha256` still lets a
    trace be compared byte-for-byte across runs without storing the full
    ~19 KiB packed mask per step per fixture.
    """

    step: int
    allowed_count: int
    mask_sha256: str


@dataclass
class GenerationResult:
    """One fixture's one-mode generation, preserved as a test artifact
    (issue #58 bullet 3: "preserving raw token IDs and mask traces")."""

    fixture_id: str
    db_id: str | None
    gold_pure_text: str | None
    mode: Mode
    prompt_ids: list[int]
    generated_ids: list[int]
    text: str
    completed: bool
    steps: int
    mask_trace: list[MaskStep] = field(default_factory=list)

    def to_json(self) -> dict:
        return {
            "fixture_id": self.fixture_id,
            "db_id": self.db_id,
            "gold_pure_text": self.gold_pure_text,
            "mode": self.mode,
            "prompt_ids": self.prompt_ids,
            "generated_ids": self.generated_ids,
            "text": self.text,
            "completed": self.completed,
            "steps": self.steps,
            "mask_trace": [
                {"step": m.step, "allowed_count": m.allowed_count, "mask_sha256": m.mask_sha256}
                for m in self.mask_trace
            ],
        }


def _unpack_mask(mask_bytes: bytes, bit_len: int) -> np.ndarray:
    """Unpack a little-endian packed mask to a bool array of length `bit_len`,
    per `ffi.rs::allowed_mask`'s documented convention."""
    return np.unpackbits(np.frombuffer(mask_bytes, dtype=np.uint8), bitorder="little")[
        :bit_len
    ].astype(bool)


def generate(
    model,
    tokenizer,
    device: str,
    vocab_bytes: list[bytes],
    eos_id: int,
    prompt_ids: list[int],
    max_new_tokens: int,
    fixture_id: str,
    db_id: str | None,
    gold_pure_text: str | None,
    mode: Mode,
    session=None,
) -> GenerationResult:
    """Greedy-decode `max_new_tokens` continuing `prompt_ids`.

    `session is None` is the unconstrained lane: plain greedy argmax over the
    model's real vocabulary (ids `[0, vocab_len)`), stopping on either real
    stop id. Otherwise this is the documented per-step loop
    (`docs/spec/architecture.md` §9.3): mask the logits, sample (here: greedy
    argmax over the masked distribution, extended with a synthetic EOS logit
    at index `vocab_len`), `accept_token`, stop once the synthetic EOS wins.

    A token sampled from `allowed_mask` that `accept_token` then rejects is a
    hard error (`RuntimeError`), never silently skipped — that combination
    would mean the mask and the recognizer disagree, the exact soundness bug
    this whole harness exists to catch live against a real model.
    """
    vocab_len = len(vocab_bytes)
    input_ids = torch.tensor([prompt_ids], device=device)
    past = None
    generated: list[int] = []
    mask_trace: list[MaskStep] = []
    completed = False

    for step in range(max_new_tokens):
        with torch.no_grad():
            out = model(input_ids=input_ids, past_key_values=past, use_cache=True)
        logits = out.logits[0, -1].to(torch.float32).cpu().numpy()
        past = out.past_key_values

        if session is None:
            candidate = int(np.argmax(logits[:vocab_len]))
            generated.append(candidate)
            if candidate in (QWEN_ENDOFTEXT, QWEN_IM_END):
                completed = True
                break
            next_id = candidate
        else:
            mask_bits = _unpack_mask(session.allowed_mask(), vocab_len + 1)
            mask_trace.append(
                MaskStep(
                    step=step,
                    allowed_count=int(mask_bits.sum()),
                    mask_sha256=hashlib.sha256(mask_bits.tobytes()).hexdigest(),
                )
            )
            extended = np.full(vocab_len + 1, -np.inf, dtype=np.float64)
            extended[:vocab_len] = np.where(mask_bits[:vocab_len], logits[:vocab_len], -np.inf)
            if mask_bits[vocab_len]:
                extended[vocab_len] = max(logits[QWEN_ENDOFTEXT], logits[QWEN_IM_END])
            candidate = int(np.argmax(extended))
            if candidate == vocab_len:
                session.accept_token(eos_id)
                completed = True
                break
            try:
                session.accept_token(candidate)
            except purecard.PureCARDError as err:
                raise RuntimeError(
                    f"{fixture_id}/{mode} step {step}: mask admitted id {candidate} "
                    f"but accept_token rejected it: {err}"
                ) from err
            generated.append(candidate)
            next_id = candidate

        input_ids = torch.tensor([[next_id]], device=device)

    text = b"".join(vocab_bytes[i] for i in generated).decode("utf-8", errors="replace")
    return GenerationResult(
        fixture_id=fixture_id,
        db_id=db_id,
        gold_pure_text=gold_pure_text,
        mode=mode,
        prompt_ids=list(prompt_ids),
        generated_ids=generated,
        text=text,
        completed=completed,
        steps=len(generated),
        mask_trace=mask_trace,
    )


def write_traces_jsonl(path: Path, results: list[GenerationResult]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for result in results:
            handle.write(json.dumps(result.to_json()) + "\n")


def write_generated_queries_jsonl(path: Path, results: list[GenerationResult]) -> None:
    """The subset of a trace the Rust Legend compile-check consumes: only
    completed constrained (L1/L2) generations are meaningful compile
    candidates — a truncated fragment or the unconstrained baseline's free-form
    text would only fail for uninteresting reasons, so both are excluded here
    (still fully preserved in the raw trace file above, never silently
    dropped from the evidence, just from the compile-rate denominator).
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as handle:
        for result in results:
            if result.mode == "unconstrained" or not result.completed:
                continue
            handle.write(
                json.dumps(
                    {
                        "fixture_id": result.fixture_id,
                        "db_id": result.db_id,
                        "gold_pure_text": result.gold_pure_text,
                        "mode": result.mode,
                        "query_text": result.text,
                    }
                )
                + "\n"
            )
