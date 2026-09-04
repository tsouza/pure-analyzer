#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Stdio binary for the `pure-analyzer` Language Server Protocol front end.

use std::process::ExitCode;

use pure_analyzer_lsp::{ServerExit, serve_stdio};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    init_tracing();
    match serve_stdio() {
        Ok(ServerExit::Clean) => ExitCode::SUCCESS,
        Ok(ServerExit::Unclean) => ExitCode::FAILURE,
        Err(error) => {
            tracing::error!(%error, "pure-analyzer-lsp exited on an I/O failure");
            ExitCode::FAILURE
        }
    }
}

/// Initialize the `tracing` subscriber, respecting `RUST_LOG`.
///
/// The stdio transport reserves standard output for JSON-RPC frames, so the
/// subscriber writes to standard error instead.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,pure_analyzer_lsp=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
}
