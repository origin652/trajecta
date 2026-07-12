//! # Contract: minimal particle-run command
//!
//! The run command resolves a Case and RunProfile, delegates construction to
//! `RunnerBuilder`, executes the runner, and maps typed errors to diagnostics
//! and exit status.

use std::path::PathBuf;

/// Minimal particle simulation command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunCommand {
    /// Local Case path.
    pub case: PathBuf,
    /// Local RunProfile path.
    pub run_profile: PathBuf,
}
