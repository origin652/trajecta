//! # Contract: human and explanation rendering
//!
//! Renderers transform typed envelopes and provenance into text. They do not
//! alter exit status, execute commands, or infer scientific meaning that is
//! absent from diagnostics and provenance records.

use crate::envelope::CommandEnvelope;

/// Text renderer for a typed command envelope.
pub trait Renderer<T> {
    /// Produces complete output text without changing the envelope.
    fn render(&self, envelope: &CommandEnvelope<T>) -> Result<String, RenderError>;
}

/// Concise human-readable renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct HumanRenderer;

impl<T: std::fmt::Debug> Renderer<T> for HumanRenderer {
    fn render(&self, envelope: &CommandEnvelope<T>) -> Result<String, RenderError> {
        Ok(format!(
            "data: {:?}\ndiagnostics: {:?}",
            envelope.data, envelope.diagnostics
        ))
    }
}

/// Detailed provenance and interpolation explanation renderer.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExplainRenderer;

impl<T: std::fmt::Debug> Renderer<T> for ExplainRenderer {
    fn render(&self, envelope: &CommandEnvelope<T>) -> Result<String, RenderError> {
        Ok(format!("explanation\n{envelope:#?}"))
    }
}

/// Output rendering failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderError {
    /// Typed payload cannot be represented by the requested renderer.
    UnsupportedPayload,
    /// Serialization failed.
    Serialization(String),
}
