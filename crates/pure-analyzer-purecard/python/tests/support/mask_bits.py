"""Little-endian packed-mask bit test, per ``Session.allowed_mask``'s
documented convention (``ffi.rs``): bit ``id`` lives at byte ``id // 8``,
position ``id % 8``.

Zero third-party dependencies deliberately: shared by both the hermetic
PyO3-boundary tests (``test_session.py``, which must stay free of the
real-model harness's heavy `torch`/`transformers` dependencies) and the
real-model harness itself.
"""

from __future__ import annotations


def bit_set(mask: bytes, index: int) -> bool:
    """Whether bit ``index`` is set in the little-endian packed ``mask``."""
    return bool((int.from_bytes(mask, "little") >> index) & 1)
