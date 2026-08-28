"""Byte-level-BPE token-string -> raw-byte decoding for the real-model harness.

Ports ``tests/support/byte_decode.rs``'s exact GPT-2 ``bytes_to_unicode``
inverse (constitution §4, DRY): a byte-level BPE tokenizer (Qwen2.5-Coder's is
one) emits token *strings* in GPT-2's byte-to-unicode alphabet, and this
recovers the raw bytes the model actually emits, deliberately kept in lockstep
with the Rust oracle rather than reinvented on the Python side.
"""

from __future__ import annotations

# GPT-2's `bytes_to_unicode` keeps three byte ranges as their own code point and
# remaps the rest; these are the exact range bounds from the reference
# implementation (mirrors byte_decode.rs's constants exactly).
LATIN1_PRINTABLE_LO = 0xA1
LATIN1_PRINTABLE_HI = 0xAC
LATIN1_SYMBOLS_LO = 0xAE
LATIN1_SYMBOLS_HI = 0xFF
# Bytes not kept as themselves are assigned code points starting just past the
# 256-value byte range, so a remapped byte never collides with a kept one.
REMAP_BASE = 256


def gpt2_byte_decoder() -> dict[str, int]:
    """The inverse of GPT-2's ``bytes_to_unicode``: map each byte-level-BPE
    token-string char back to the raw byte the model actually emits (this also
    undoes the ``Ġ``->space and other whitespace conventions, since they live
    inside the byte table). Every byte-level-BPE token string is composed only
    of chars in this table.
    """
    bs = (
        list(range(ord("!"), ord("~") + 1))
        + list(range(LATIN1_PRINTABLE_LO, LATIN1_PRINTABLE_HI + 1))
        + list(range(LATIN1_SYMBOLS_LO, LATIN1_SYMBOLS_HI + 1))
    )
    cs = list(bs)
    kept = set(bs)
    n = 0
    for byte in range(256):
        if byte not in kept:
            bs.append(byte)
            cs.append(REMAP_BASE + n)
            n += 1
    return {chr(code_point): byte for byte, code_point in zip(bs, cs)}


def true_bytes(tok: str, dec: dict[str, int]) -> bytes:
    """The true emitted bytes of one byte-level-BPE token string, decoded
    through :func:`gpt2_byte_decoder`. A special token (``<|im_end|>``, FIM) is
    stored as a literal ASCII string whose chars map to themselves, so its
    "bytes" are the literal ``<|...|>`` text — never valid Pure, so the byte-PDA
    rejects it and it is inadmissible mid-query, exactly as required.
    """
    try:
        return bytes(dec[char] for char in tok)
    except KeyError as err:
        raise ValueError(
            f"token char {err.args[0]!r} is outside the byte-level table; "
            "cannot recover its true bytes"
        ) from err
