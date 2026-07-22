//! Thin, typed wrapper around `std::process::Command` for running toolchain
//! commands and propagating their exit codes.

use std::process::Command;

use anyhow::{Context, Result, bail};

/// Run `program` with `args`, inheriting stdio, and fail if it exits non-zero.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or exits with a non-zero
/// status (or is killed by a signal).
pub fn run(program: &str, args: &[&str]) -> Result<()> {
    eprintln!("+ {program} {}", args.join(" "));

    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{program}` (is it installed and on PATH?)"))?;

    if !status.success() {
        match status.code() {
            Some(code) => bail!("`{program}` exited with status {code}"),
            None => bail!("`{program}` was terminated by a signal"),
        }
    }
    Ok(())
}

/// Run `program` with `args` and return its captured stdout as a `String`,
/// failing if the process cannot be spawned, exits non-zero, or emits non-UTF-8.
///
/// # Errors
///
/// Returns an error on spawn failure, non-zero exit / signal, or invalid UTF-8.
pub fn run_stdout(program: &str, args: &[&str]) -> Result<String> {
    eprintln!("+ {program} {}", args.join(" "));

    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn `{program}` (is it installed and on PATH?)"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match output.status.code() {
            Some(code) => bail!("`{program}` exited with status {code}:\n{stderr}"),
            None => bail!("`{program}` was terminated by a signal:\n{stderr}"),
        }
    }
    String::from_utf8(output.stdout)
        .with_context(|| format!("`{program}` produced non-UTF-8 output"))
}

/// Run a sequence of `cargo` subcommands in order, stopping at the first
/// failure (fail-fast).
///
/// # Errors
///
/// Propagates the first failing step's error.
pub fn run_cargo_steps(steps: &[&[&str]]) -> Result<()> {
    for step in steps {
        run("cargo", step)?;
    }
    Ok(())
}
