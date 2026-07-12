//! # Contract: stable command result envelope
//!
//! Human and JSON renderers receive the same typed success value and sorted
//! diagnostics. Exit status is derived from the envelope rather than renderer
//! text.

use trajecta_case::diagnostic::{Diagnostic, Severity};

/// Uniform command result passed to all renderers.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandEnvelope<T> {
    /// Optional success payload.
    pub data: Option<T>,
    /// Deterministically sorted diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

impl<T> CommandEnvelope<T> {
    /// Returns true when no error diagnostic exists.
    #[must_use]
    pub fn is_success(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity() == Severity::Error)
    }

    /// Returns the stable process exit code for this envelope.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.is_success() { 0 } else { 1 }
    }
}
