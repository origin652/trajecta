//! # Contract: command-line crate
//!
//! This crate is a thin adapter: argument parsing, local document loading,
//! public library calls, rendering, and exit-code selection. It contains no
//! meteorological decoding, interpolation, cache, migration, or integration
//! algorithms.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::ffi::OsString;

pub mod cli;
pub mod command;
pub mod envelope;
pub mod render;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 0;

/// Parses and dispatches a command, returning a process exit code.
#[must_use]
pub fn main_entry(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    match cli::Cli::parse_from(arguments) {
        Ok(_cli) => 2,
        Err(_error) => 2,
    }
}
