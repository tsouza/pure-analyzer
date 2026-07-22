"""Hermetic tests for the PureCARD PyO3 boundary (M4).

No model and no Legend engine: a hand-crafted byte-token vocabulary is enough to
prove the binding faithfully marshals the Rust core — a valid Pure query streams
token by token with every gold token set in ``allowed_mask()`` and accepted, an
illegal token is masked and rejected, and ``is_complete``/``reset`` behave. The
grammar/masking semantics themselves are the Rust suite's job; this asserts the
*boundary* is a faithful, thin pass-through.
"""

import pytest

import purecard

# A whole-token vocabulary mirroring the Rust `token_vocab` fixture: a complete
# source expression, a step opener, a digit, a closer, and the empty token. Token
# id == list index. The reserved EOS bit lives at index ``VOCAB_LEN`` (one past
# the last token), independent of this ``eos_id`` field.
VOCAB = [b"|X.all()", b"->take(", b"1", b")", b""]
EOS_ID = 4
VOCAB_LEN = len(VOCAB)
# "|X.all()->take(1)" as a token-id stream: source, step, digit, closer.
GOLD_QUERY = [0, 1, 2, 3]


def _bit_set(mask: bytes, idx: int) -> bool:
    """Whether bit ``idx`` is set in the little-endian packed mask."""
    return bool((int.from_bytes(mask, "little") >> idx) & 1)


@pytest.fixture
def grammar():
    return purecard.compile_grammar("", VOCAB, EOS_ID)


def test_compile_grammar_reports_vocab_len(grammar):
    session = purecard.Session(grammar)
    assert session.vocab_len == VOCAB_LEN


def test_mask_is_packed_to_the_expected_byte_length(grammar):
    session = purecard.Session(grammar)
    mask = session.allowed_mask()
    # ceil((VOCAB_LEN + 1) / 8) bytes cover ids 0..=VOCAB_LEN (EOS included).
    assert len(mask) == (VOCAB_LEN + 1 + 7) // 8


def test_a_valid_query_streams_with_every_gold_token_admissible(grammar):
    session = purecard.Session(grammar)
    assert not session.is_complete()
    for token_id in GOLD_QUERY:
        mask = session.allowed_mask()
        assert _bit_set(mask, token_id), f"gold token {token_id} must be admissible"
        session.accept_token(token_id)
    assert session.is_complete()
    # A completed stream sets the reserved EOS bit (index VOCAB_LEN)…
    assert _bit_set(session.allowed_mask(), VOCAB_LEN)
    # …and EOS is then acceptable.
    session.accept_token(VOCAB_LEN)


def test_an_illegal_token_is_masked_and_rejected(grammar):
    session = purecard.Session(grammar)
    # "|X.all()" alone is a complete query (empty stack) — a lone closer ")" (id 3)
    # cannot follow it.
    session.accept_token(0)
    assert session.is_complete()
    assert not _bit_set(session.allowed_mask(), 3)
    with pytest.raises(purecard.PureCARDError):
        session.accept_token(3)
    # The rejected token left the session untouched: still complete.
    assert session.is_complete()


def test_out_of_range_token_is_rejected(grammar):
    session = purecard.Session(grammar)
    with pytest.raises(purecard.PureCARDError):
        session.accept_token(999)


def test_premature_eos_is_rejected(grammar):
    session = purecard.Session(grammar)
    assert not session.is_complete()
    with pytest.raises(purecard.PureCARDError):
        session.accept_token(VOCAB_LEN)


def test_reset_restores_a_fresh_stream(grammar):
    session = purecard.Session(grammar)
    for token_id in GOLD_QUERY:
        session.accept_token(token_id)
    assert session.is_complete()
    session.reset()
    assert not session.is_complete()
    # After reset the mask matches a never-driven session's, bit for bit.
    fresh = purecard.Session(grammar)
    assert session.allowed_mask() == fresh.allowed_mask()
    # …and the stream can be driven again.
    session.accept_token(0)
    assert session.is_complete()


def test_schema_json_that_is_not_a_contract_raises(grammar):
    with pytest.raises(purecard.PureCARDError):
        purecard.Session(grammar, "{ not valid json")


# A whole-token vocabulary spelling a %latest milestoning query (gap report G2):
# a milestoned source open, the symbolic literal, the source close, a step, a
# digit, the closer, and the empty token. Token id == list index.
LATEST_VOCAB = [
    b"|finos::trade::Trade.all(",
    b"%latest",
    b")",
    b"->take(",
    b"1",
    b")",
    b"",
]
LATEST_EOS_ID = 6
LATEST_GOLD = [0, 1, 2, 3, 4, 5]


def test_a_latest_milestoning_query_streams_end_to_end():
    """A `%latest` milestoning query marshals through the PyO3 boundary end to
    end — the symbolic literal is admissible at each step and the stream completes
    (gap report G2)."""
    grammar = purecard.compile_grammar("", LATEST_VOCAB, LATEST_EOS_ID)
    session = purecard.Session(grammar)
    assert not session.is_complete()
    for token_id in LATEST_GOLD:
        mask = session.allowed_mask()
        assert _bit_set(mask, token_id), f"gold token {token_id} must be admissible"
        session.accept_token(token_id)
    assert session.is_complete()
    # The completed stream sets the reserved EOS bit — at index `len(vocab)` (one
    # past the last token id), exactly as the base fixture asserts, not at the
    # `eos_id` field — and EOS is then acceptable.
    assert _bit_set(session.allowed_mask(), len(LATEST_VOCAB))
    session.accept_token(len(LATEST_VOCAB))


# A whole-token vocabulary spelling an arm-R Relation/Function API query
# (`|X.all()->project(~[Col: x|$x.a])`, gap report G1): source, the `project(~[`
# relation column-set open, a column lambda, the close, and the empty token.
ARM_R_VOCAB = [
    b"|X.all()",
    b"->project(~[",
    b"Col: x|$x.a",
    b"])",
    b"",
]
ARM_R_EOS_ID = 4
ARM_R_GOLD = [0, 1, 2, 3]


def test_an_arm_r_relation_api_query_streams_end_to_end():
    """An arm-R `project(~[…])` relation query marshals through the PyO3 boundary
    end to end — the `~` column-set sigil and its column lambda are admissible at
    each step and the stream completes (gap report G1)."""
    grammar = purecard.compile_grammar("", ARM_R_VOCAB, ARM_R_EOS_ID)
    session = purecard.Session(grammar)
    assert not session.is_complete()
    for token_id in ARM_R_GOLD:
        mask = session.allowed_mask()
        assert _bit_set(mask, token_id), f"gold token {token_id} must be admissible"
        session.accept_token(token_id)
    assert session.is_complete()
    assert _bit_set(session.allowed_mask(), len(ARM_R_VOCAB))
    session.accept_token(len(ARM_R_VOCAB))
