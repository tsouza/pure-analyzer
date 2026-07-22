//! The PyO3 boundary (M4): the thin, feature-gated Python surface over the pure
//! decoder core (`docs/spec/architecture.md` §9.2). Compiled only under the
//! `python` feature; the default build never sees pyo3.
//!
//! This module is **marshaling only** — no decode logic lives here. It exposes
//! three things to Python: [`compile_grammar`] (build a shareable grammar from a
//! byte-token vocabulary), [`Grammar`] (an `Rc`-shared compiled grammar), and
//! [`Session`] (a per-stream decode driver). Each `#[pymethods]` entry is a
//! one-line delegate into the core `DecoderSession`; the mask-packing and error
//! mapping it depends on are pure functions ([`BitMask::pack_le_bytes_into`] and
//! the `From` impls below) tested without a Python interpreter.
//!
//! ## The lifetime crux
//!
//! `DecoderSession<'g>` borrows its [`CompiledGrammar`] (`src/session.rs`), and a
//! Python object cannot carry a Rust lifetime. [`Session`] resolves this with
//! `self_cell`: it co-locates the `Rc<CompiledGrammar>` *owner* and the borrowing
//! `DecoderSession` *dependent* in one cell, so no borrow ever crosses the FFI
//! boundary and the `Rc` keeps the grammar alive independently of the Python
//! [`Grammar`] object (several sessions may share one grammar). self_cell's
//! `unsafe` is encapsulated inside the self_cell crate, so the crate-wide
//! `#![forbid(unsafe_code)]` still holds here. `Rc`, not `Arc`, because the
//! pyclasses are `unsendable` — driven from one thread under the GIL — so the
//! atomic refcount would buy a thread-safety the boundary never uses.

// This whole module is a private FFI boundary: its items (`Grammar`, `Session`,
// `PureCARDError`, …) are reachable only from Python, never from the Rust public
// API, so intra-doc links among them resolve to "private" items. That is correct
// here — the docs describe the boundary's own internals — so the private-link
// lint is scoped-off for this module alone (it exists to catch *public* docs
// leaking private links, which cannot happen from a private module).
#![allow(rustdoc::private_intra_doc_links)]

use std::rc::Rc;

use pyo3::create_exception;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::wrap_pyfunction;

use crate::error::DecodeError;
use crate::grammar::compiled::CompiledGrammar;
use crate::schema::{Schema, SchemaError};
use crate::session::DecoderSession;
use crate::vocab::Vocab;

create_exception!(purecard, PureCARDError, PyValueError);

/// A rejected token or malformed schema surfaces to Python as [`PureCARDError`],
/// carrying the core error's `Display` text. `PyErr` and `DecodeError`/`SchemaError`
/// map one-to-one, so `?` in a `#[pymethods]` body does the conversion.
impl From<DecodeError> for PyErr {
    fn from(err: DecodeError) -> Self {
        PureCARDError::new_err(err.to_string())
    }
}

impl From<SchemaError> for PyErr {
    fn from(err: SchemaError) -> Self {
        PureCARDError::new_err(err.to_string())
    }
}

/// A grammar compiled against a model vocabulary, shared across sessions.
///
/// Holds an `Rc<CompiledGrammar>`: cloning it into each [`Session`] is cheap and
/// the underlying grammar outlives every session that borrows it. `frozen` marks
/// the class immutable, so Python sees a plain shareable handle.
///
/// `unsendable` because `CompiledGrammar`'s lazy per-state mask cache is a
/// `std::cell::OnceCell` (interior mutability behind `&self`, `!Sync`). The
/// decoder is driven from Python under the GIL — one thread at a time — so
/// pyo3's runtime thread-affinity check is the right, minimal boundary
/// concession; it keeps the pure core's cache design untouched rather than
/// forcing a `Sync` cell (and an atomic on the per-step hot path) for a
/// concurrency the caller never exercises.
#[pyclass(frozen, unsendable)]
pub(crate) struct Grammar {
    inner: Rc<CompiledGrammar>,
}

/// Compile `spec` against the byte-token vocabulary `vocab_bytes` (token id =
/// list index) with reserved EOS id `eos_id`, returning a shareable [`Grammar`].
///
/// Infallible: [`Vocab::from_byte_tokens`] and [`CompiledGrammar::from_spec`] do
/// not fail (the latter currently ignores `spec` and compiles the fixed byte-PDA;
/// §5 EBNF compilation lands later).
#[pyfunction]
pub(crate) fn compile_grammar(spec: &str, vocab_bytes: Vec<Vec<u8>>, eos_id: u32) -> Grammar {
    let vocab = Vocab::from_byte_tokens(vocab_bytes, eos_id);
    Grammar {
        inner: Rc::new(CompiledGrammar::from_spec(spec, vocab)),
    }
}

type BorrowedSession<'g> = DecoderSession<'g>;

// self_cell's generated constructors call `.unwrap()` internally (on an `Option`
// the macro proves is `Some`); that expansion lands in our crate, so the repo-wide
// disallowed-methods gate would flag it. Confine the cell to its own module so the
// allow covers only self_cell's expansion — none of our hand-written code is
// exempted (an `#[allow]` on the macro invocation itself is ignored by the
// compiler, hence the module).
mod cell {
    #![allow(clippy::disallowed_methods)]

    use self_cell::self_cell;

    use super::{BorrowedSession, CompiledGrammar, Rc};

    self_cell!(
        pub(super) struct SessionCell {
            owner: Rc<CompiledGrammar>,
            #[covariant]
            dependent: BorrowedSession,
        }
    );
}

use cell::SessionCell;

/// A per-stream decode driver: the Python-facing mirror of
/// [`DecoderSession`](crate::DecoderSession) (§9.2).
///
/// Owns its grammar through the [`SessionCell`], so it is fully self-contained
/// and `'static` — nothing borrows across the FFI boundary. `unsendable` for the
/// same reason as [`Grammar`]: the shared `CompiledGrammar`'s `OnceCell` cache is
/// `!Sync` and the session is driven single-threaded under the GIL.
#[pyclass(unsendable)]
pub(crate) struct Session {
    cell: SessionCell,
    /// The reused little-endian mask buffer [`allowed_mask`](Session::allowed_mask)
    /// refills each step, so the per-step path allocates nothing.
    mask: Vec<u8>,
    vocab_len: usize,
}

#[pymethods]
impl Session {
    /// Open a session over `grammar`, optionally enforcing the L2 schema overlay
    /// parsed from `schema_json` (`None` is L1-only).
    ///
    /// Raises [`PureCARDError`] if `schema_json` is present but not a well-formed
    /// schema contract.
    #[new]
    #[pyo3(signature = (grammar, schema_json=None))]
    fn new(grammar: &Grammar, schema_json: Option<String>) -> PyResult<Self> {
        let vocab_len = grammar.inner.vocab().len();
        let schema = match schema_json {
            Some(json) => Some(Schema::from_json(&json)?),
            None => None,
        };
        let owner = Rc::clone(&grammar.inner);
        let cell = match schema {
            Some(schema) => SessionCell::new(owner, |g| DecoderSession::with_schema(g, schema)),
            None => SessionCell::new(owner, |g| DecoderSession::new(g)),
        };
        Ok(Self {
            cell,
            mask: Vec::new(),
            vocab_len,
        })
    }

    /// The allowed-token mask at the current position, packed little-endian: bit
    /// `id` at byte `id / 8`, position `id % 8`. The EOS bit is at index
    /// [`vocab_len`](Session::vocab_len). Unpack in Python with
    /// `np.unpackbits(mask, bitorder="little")[:vocab_len + 1]`.
    ///
    /// This mask is the **sole point that enforces the shipped schema (L2) rules**
    /// (`docs/spec/schema.md` §6.7): with a schema set, it clears tokens illegal
    /// under those rules (deferred rules pass through). `accept_token` checks only
    /// the grammar — the contract is to sample from this mask, then commit.
    fn allowed_mask<'py>(&mut self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let buf = &mut self.mask;
        self.cell.with_dependent_mut(|_, session| {
            session.allowed_mask().pack_le_bytes_into(buf);
        });
        PyBytes::new(py, &self.mask)
    }

    /// Advance by one whole token id, or raise [`PureCARDError`] if the token is
    /// inadmissible (its bytes dead-end the recognizer, it is out of range, or it
    /// is a premature EOS). A rejected token leaves the session untouched (§8.5).
    ///
    /// "Inadmissible" here means **grammar**-inadmissible (L1). This does not raise
    /// on a token that is grammar-legal but schema-masked: `accept_token` is not a
    /// schema backstop. `allowed_mask` is the sole point that enforces the shipped
    /// schema (L2) rules — sample from it, then commit with `accept_token`.
    fn accept_token(&mut self, id: u32) -> PyResult<()> {
        self.cell
            .with_dependent_mut(|_, session| session.accept_token(id))?;
        Ok(())
    }

    /// Whether the stream so far is a complete query (an accepting configuration).
    fn is_complete(&self) -> bool {
        self.cell.with_dependent(|_, session| session.is_complete())
    }

    /// Return to a fresh stream, keeping the mask buffer and automaton stack
    /// allocated for reuse.
    fn reset(&mut self) {
        self.cell.with_dependent_mut(|_, session| session.reset());
    }

    /// The vocabulary size `V`; the reserved EOS bit lives at index `V` in the
    /// mask, whose full length is `V + 1`.
    #[getter]
    fn vocab_len(&self) -> usize {
        self.vocab_len
    }
}

/// The `purecard` CPython extension module (the maturin `module-name`).
#[pymodule]
fn purecard(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(compile_grammar, module)?)?;
    module.add_class::<Grammar>()?;
    module.add_class::<Session>()?;
    module.add("PureCARDError", module.py().get_type::<PureCARDError>())?;
    Ok(())
}
