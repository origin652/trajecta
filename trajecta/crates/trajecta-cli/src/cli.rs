//! # Contract: top-level command parsing
//!
//! Parsing converts operating-system arguments into typed commands without
//! opening meteorology files or executing scientific work. Machine-readable
//! and human output modes share the same command result envelope.

use std::ffi::OsString;

use crate::command::Command;

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
        _arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, CliParseError> {
        Err(CliParseError::NotImplemented)
    }
}

/// Top-level argument parsing failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliParseError {
    /// Argument parser has not been implemented yet.
    NotImplemented,
    /// A required subcommand or option is absent.
    MissingArgument(String),
    /// An option value is invalid.
    InvalidArgument(String),
}
