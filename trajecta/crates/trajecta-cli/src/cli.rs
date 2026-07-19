//! # Contract: top-level command parsing
//!
//! Parsing converts operating-system arguments into typed commands without
//! opening meteorology files or executing scientific work. Machine-readable
//! and human output modes share the same command result envelope.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use trajecta_case::document::MeteorologyReaderBackend;

use crate::command::case::CaseCommand;
use crate::command::{Command, DataCommand, MetCommand, RunCommand};

/// Requested output representation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutputMode {
    /// Human-readable terminal output.
    Human,
    /// Stable machine-readable JSON envelope.
    Json,
}

/// Fully parsed top-level command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cli {
    /// Requested rendering mode.
    pub output: OutputMode,
    /// Typed subcommand.
    pub command: Command,
}

impl Cli {
    /// Parses operating-system strings without assuming UTF-8 paths.
    pub fn parse_from(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, CliParseError> {
        let mut args = arguments.into_iter();
        // Skip program name when present.
        let _ = args.next();
        let mut output = OutputMode::Human;
        let mut positional = Vec::new();
        for arg in args {
            if arg == "--json" {
                output = OutputMode::Json;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Err(CliParseError::Help);
            }
            positional.push(arg);
        }
        if positional.is_empty() {
            return Err(CliParseError::Help);
        }
        let command = match positional[0].to_string_lossy().as_ref() {
            "met" => Command::Met(parse_met(&positional[1..])?),
            "case" => Command::Case(parse_case(&positional[1..])?),
            "data" => Command::Data(parse_data(&positional[1..])?),
            "run" => Command::Run(parse_run(&positional[1..])?),
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown command `{other}`"
                )));
            }
        };
        Ok(Self { output, command })
    }
}

fn parse_met(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    if args.is_empty() {
        return Err(CliParseError::MissingArgument(
            "met subcommand (probe|replay)".into(),
        ));
    }
    match args[0].to_string_lossy().as_ref() {
        "probe" => parse_met_probe(&args[1..]),
        "replay" => parse_met_replay(&args[1..]),
        other => Err(CliParseError::InvalidArgument(format!(
            "unknown met subcommand `{other}`"
        ))),
    }
}

fn parse_met_probe(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    let mut data_root = None;
    let mut profile = None;
    let mut time_unix = None;
    let mut points = None;
    let mut backend = MeteorologyReaderBackend::Rust;
    let mut coverage_start_unix = None;
    let mut coverage_end_unix = None;
    let mut allow_estimated = false;
    let mut summary = true;
    let mut explain = false;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--data-root" => {
                index += 1;
                data_root = Some(require_path(args, index, "--data-root")?);
            }
            "--profile" => {
                index += 1;
                profile = Some(require_utf8(args, index, "--profile")?);
            }
            "--time" => {
                index += 1;
                time_unix = Some(require_i64(args, index, "--time")?);
            }
            "--points" => {
                index += 1;
                points = Some(require_path(args, index, "--points")?);
            }
            "--backend" => {
                index += 1;
                backend = parse_backend(&require_utf8(args, index, "--backend")?)?;
            }
            "--coverage-start" => {
                index += 1;
                coverage_start_unix = Some(require_i64(args, index, "--coverage-start")?);
            }
            "--coverage-end" => {
                index += 1;
                coverage_end_unix = Some(require_i64(args, index, "--coverage-end")?);
            }
            "--allow-estimated" => allow_estimated = true,
            "--no-summary" => summary = false,
            "--summary" => summary = true,
            "--explain" => explain = true,
            "--output" => {
                index += 1;
                output = Some(require_path(args, index, "--output")?);
            }
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown met probe option `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(MetCommand::Probe {
        data_root: data_root.ok_or_else(|| CliParseError::MissingArgument("--data-root".into()))?,
        profile: profile.ok_or_else(|| CliParseError::MissingArgument("--profile".into()))?,
        time_unix: time_unix.ok_or_else(|| CliParseError::MissingArgument("--time".into()))?,
        points,
        backend,
        coverage_start_unix,
        coverage_end_unix,
        allow_estimated,
        summary,
        explain,
        output,
    })
}

fn parse_met_replay(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    let mut data_root = None;
    let mut profile = None;
    let mut input = None;
    let mut output = Some(PathBuf::from("-"));
    let mut backend = MeteorologyReaderBackend::Rust;
    let mut coverage_start_unix = None;
    let mut coverage_end_unix = None;
    let mut allow_estimated = false;
    let mut explain = false;
    let mut summary = true;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--data-root" => {
                index += 1;
                data_root = Some(require_path(args, index, "--data-root")?);
            }
            "--profile" => {
                index += 1;
                profile = Some(require_utf8(args, index, "--profile")?);
            }
            "--input" => {
                index += 1;
                input = Some(require_path(args, index, "--input")?);
            }
            "--output" => {
                index += 1;
                output = Some(require_path(args, index, "--output")?);
            }
            "--backend" => {
                index += 1;
                backend = parse_backend(&require_utf8(args, index, "--backend")?)?;
            }
            "--coverage-start" => {
                index += 1;
                coverage_start_unix = Some(require_i64(args, index, "--coverage-start")?);
            }
            "--coverage-end" => {
                index += 1;
                coverage_end_unix = Some(require_i64(args, index, "--coverage-end")?);
            }
            "--allow-estimated" => allow_estimated = true,
            "--explain" => explain = true,
            "--no-summary" => summary = false,
            "--summary" => summary = true,
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown met replay option `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(MetCommand::Replay {
        data_root: data_root.ok_or_else(|| CliParseError::MissingArgument("--data-root".into()))?,
        profile: profile.ok_or_else(|| CliParseError::MissingArgument("--profile".into()))?,
        input: input.ok_or_else(|| CliParseError::MissingArgument("--input".into()))?,
        output: output.unwrap_or_else(|| PathBuf::from("-")),
        backend,
        coverage_start_unix,
        coverage_end_unix,
        allow_estimated,
        explain,
        summary,
    })
}

fn parse_case(_args: &[OsString]) -> Result<CaseCommand, CliParseError> {
    Err(CliParseError::NotImplemented)
}

fn parse_data(_args: &[OsString]) -> Result<DataCommand, CliParseError> {
    Err(CliParseError::NotImplemented)
}

fn parse_run(_args: &[OsString]) -> Result<RunCommand, CliParseError> {
    Err(CliParseError::NotImplemented)
}

fn parse_backend(value: &str) -> Result<MeteorologyReaderBackend, CliParseError> {
    match value {
        "rust" => Ok(MeteorologyReaderBackend::Rust),
        "native" => Ok(MeteorologyReaderBackend::Native),
        other => Err(CliParseError::InvalidArgument(format!(
            "unknown backend `{other}` (rust|native)"
        ))),
    }
}

fn require_path(args: &[OsString], index: usize, flag: &str) -> Result<PathBuf, CliParseError> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| CliParseError::MissingArgument(flag.into()))
}

fn require_utf8(args: &[OsString], index: usize, flag: &str) -> Result<String, CliParseError> {
    let value = args
        .get(index)
        .ok_or_else(|| CliParseError::MissingArgument(flag.into()))?;
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliParseError::InvalidArgument(format!("{flag} is not valid UTF-8")))
}

fn require_i64(args: &[OsString], index: usize, flag: &str) -> Result<i64, CliParseError> {
    let text = require_utf8(args, index, flag)?;
    text.parse::<i64>()
        .map_err(|_| CliParseError::InvalidArgument(format!("{flag} must be an integer")))
}

/// Top-level argument parsing failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliParseError {
    /// Argument parser has not been implemented yet.
    NotImplemented,
    /// Help was requested or no command was provided.
    Help,
    /// A required subcommand or option is absent.
    MissingArgument(String),
    /// An option value is invalid.
    InvalidArgument(String),
}

impl CliParseError {
    /// Stable process exit code.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Help | Self::MissingArgument(_) | Self::InvalidArgument(_) => 2,
            Self::NotImplemented => 2,
        }
    }
}

impl std::fmt::Display for CliParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotImplemented => formatter.write_str("command not implemented"),
            Self::Help => formatter.write_str(HELP_TEXT),
            Self::MissingArgument(name) => write!(formatter, "missing argument: {name}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
        }
    }
}

const HELP_TEXT: &str = "\
trajecta — Trajecta command-line interface

Usage:
  trajecta met probe --data-root DIR --profile NAME --time UNIX [options]
  trajecta met replay --data-root DIR --profile NAME --input JSONL [options]

met probe options:
  --points PATH       JSONL points (`-` = stdin); default single Europe sample
  --backend rust|native
  --coverage-start UNIX --coverage-end UNIX
  --allow-estimated   --explain  --summary|--no-summary
  --output PATH       JSONL output (`-` = stdout)

met replay options:
  --input PATH        JSONL stream (`-` = stdin)
  --output PATH       JSONL stream (`-` = stdout)
  --backend rust|native
  --coverage-start UNIX --coverage-end UNIX
  --allow-estimated   --explain  --summary|--no-summary

Global:
  --json              machine envelope mode (reserved)
  -h, --help          show this help
";

#[allow(dead_code)]
fn os(value: &str) -> &OsStr {
    OsStr::new(value)
}
