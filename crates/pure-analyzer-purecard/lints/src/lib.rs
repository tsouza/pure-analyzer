#![feature(rustc_private)]
#![warn(unused_extern_crates)]
//! Custom dylint library for this workspace.
//!
//! Ships a single lint, [`NO_UNWRAP_IN_LIB`], enforcing the project rule that
//! `.unwrap()` and `.expect(..)` must not appear in non-test library code.
//! Tests (`#[cfg(test)]` / `#[test]`) are exempt.
//!
//! Built as a `cdylib` and loaded by `cargo-dylint`. Run with:
//! `cargo dylint --lib lints -- --workspace`.

// `rustc` private crates, provided by the nightly toolchain when
// `rustc_private` is enabled.
extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_session;
extern crate rustc_span;

use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::is_in_test;
use rustc_hir::{Expr, ExprKind};
use rustc_lint::{LateContext, LateLintPass};
use rustc_session::{declare_lint, declare_lint_pass};

declare_lint! {
    /// ### What it does
    /// Forbids `.unwrap()` and `.expect(..)` in non-test code.
    ///
    /// ### Why is this bad?
    /// These methods panic on `None`/`Err`, turning recoverable conditions into
    /// process aborts. In library code they erase the type-level error
    /// contract this project relies on. Prefer `?`, pattern matching, or a
    /// typed error.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let value = maybe_value.unwrap(); // panics on None
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// let value = maybe_value.ok_or(MyError::Missing)?;
    /// ```
    pub NO_UNWRAP_IN_LIB,
    Deny,
    "`.unwrap()`/`.expect()` are forbidden outside of test code"
}

declare_lint_pass!(NoUnwrapInLib => [NO_UNWRAP_IN_LIB]);

impl<'tcx> LateLintPass<'tcx> for NoUnwrapInLib {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'tcx>) {
        // Only method calls of the shape `receiver.method(..)`.
        let ExprKind::MethodCall(path, _receiver, _args, _span) = expr.kind else {
            return;
        };

        let method = path.ident.name.as_str();
        if method != "unwrap" && method != "expect" {
            return;
        }

        // Exempt test code: unit tests routinely `.unwrap()` on setup.
        if is_in_test(cx.tcx, expr.hir_id) {
            return;
        }

        span_lint_and_help(
            cx,
            NO_UNWRAP_IN_LIB,
            expr.span,
            format!("use of `.{method}()` in non-test code"),
            None,
            "propagate the error with `?` or handle it explicitly instead of panicking",
        );
    }
}

// Emit the FFI symbols (`dylint_version`, the registration entry point, etc.)
// that the `cargo-dylint` driver looks for. This is the standard dylint
// library template macro.
dylint_linting::dylint_library!();

/// dylint entry point: register this library's lint pass with the driver.
///
/// The `#[no_mangle]` symbol is discovered by `cargo-dylint`; `register_lints`
/// is the conventional name the driver invokes to install the lint.
#[unsafe(no_mangle)]
pub fn register_lints(sess: &rustc_session::Session, lint_store: &mut rustc_lint::LintStore) {
    dylint_linting::init_config(sess);
    lint_store.register_lints(&[NO_UNWRAP_IN_LIB]);
    lint_store.register_late_pass(|_| Box::new(NoUnwrapInLib));
}
