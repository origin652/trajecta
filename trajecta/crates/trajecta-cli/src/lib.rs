//! # Contract: command-line crate
//!
//! This crate is a thin adapter: argument parsing, local document loading,
//! public library calls, rendering, and exit-code selection. It contains no
//! meteorological decoding, interpolation, cache, migration, or integration
//! algorithms.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::{self, Write};

mod app;
mod case_data;
mod command_result;
mod configuration;
mod data_lock;
mod project;
mod result_products;
mod runtime;

pub mod cli;
pub mod command;
pub mod envelope;
pub mod render;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 0;

/// Parses and dispatches a command, returning a process exit code.
#[must_use]
pub fn main_entry(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    if let Some(exit_code) = runtime::internal_entry(&arguments) {
        return exit_code;
    }
    match cli::Cli::parse_from(arguments.clone()) {
        Ok(cli) => {
            match &cli.command {
                command::Command::StagedRun(command) => {
                    return runtime::execute_run(command, cli.config_path.as_deref(), cli.output);
                }
                command::Command::Job(command) => {
                    return runtime::execute_job(command, cli.config_path.as_deref(), cli.output);
                }
                command::Command::Result(command) => {
                    return runtime::execute_result(
                        command,
                        cli.config_path.as_deref(),
                        cli.output,
                    );
                }
                _ => {}
            }
            if let command::Command::Met(met) = &cli.command {
                if cli.output == cli::OutputMode::Human {
                    return match command::met::execute(met) {
                        Ok(code) => code,
                        Err(error) => {
                            let _ = writeln!(io::stderr(), "trajecta: {error}");
                            error.exit_code()
                        }
                    };
                }
                if met_writes_stdout(met) {
                    let outcome = app::AppOutcome::error(
                        app::command_name(&cli.command),
                        "met.machine_stdout_requires_file",
                        "machine envelope mode requires met records to be directed to --output FILE",
                        "use --output FILE or --format human for native JSONL records",
                    );
                    return write_outcome(&outcome, cli.output);
                }
                let outcome = match command::met::execute(met) {
                    Ok(0) => app::AppOutcome::ok(
                        app::command_name(&cli.command),
                        serde_json::json!({"stream": "met output completed"}),
                    ),
                    Ok(code) => app::AppOutcome::error(
                        app::command_name(&cli.command),
                        "met.runtime",
                        format!("met command exited with {code}"),
                        "inspect meteorology input and diagnostics",
                    ),
                    Err(error) => app::AppOutcome::error(
                        app::command_name(&cli.command),
                        error.code(),
                        error.to_string(),
                        "inspect meteorology input and diagnostics",
                    ),
                };
                return write_outcome(&outcome, cli.output);
            }
            let outcome = app::execute(
                &cli.command,
                cli.config_path.as_deref(),
                cli.project_path.as_deref(),
            );
            write_outcome(&outcome, cli.output)
        }
        Err(error) => {
            let output = cli::requested_output(&arguments);
            if matches!(error, cli::CliParseError::Help) {
                if output == cli::OutputMode::Human {
                    let _ = writeln!(io::stdout(), "{error}");
                    return 0;
                }
                let outcome =
                    app::AppOutcome::ok("cli", serde_json::json!({"help": error.to_string()}));
                return write_outcome(&outcome, output);
            }
            if output == cli::OutputMode::Human {
                let _ = writeln!(io::stderr(), "{error}");
                return error.exit_code();
            }
            let outcome = app::AppOutcome::error(
                cli::parse_error_command(&arguments),
                "cli.invalid_arguments",
                error.to_string(),
                "inspect command usage and global options",
            );
            let _ = write_outcome(&outcome, output);
            error.exit_code()
        }
    }
}

fn met_writes_stdout(command: &command::met::MetCommand) -> bool {
    match command {
        command::met::MetCommand::Probe { output, .. } => {
            output.is_none() || output.as_deref() == Some(std::path::Path::new("-"))
        }
        command::met::MetCommand::Replay { output, .. } => output == std::path::Path::new("-"),
    }
}

fn write_outcome(outcome: &app::AppOutcome, output: cli::OutputMode) -> i32 {
    match app::render(outcome, output) {
        Ok(text) => {
            let _ = writeln!(io::stdout(), "{text}");
            if outcome.is_ok() {
                outcome.exit_code
            } else {
                1
            }
        }
        Err(error) => {
            let _ = writeln!(io::stderr(), "trajecta: render failure: {error}");
            1
        }
    }
}
