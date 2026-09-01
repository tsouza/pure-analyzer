#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Stdio binary for the `pure-analyzer` Language Server Protocol front end.

use std::process::ExitCode;

use pure_analyzer_lsp::{ServerExit, serve_stdio};

fn main() -> ExitCode {
    match serve_stdio() {
        Ok(ServerExit::Clean) => ExitCode::SUCCESS,
        Ok(ServerExit::Unclean) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("pure-analyzer-lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
