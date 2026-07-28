use serde_json::Value;
use trajecta_case::diagnostic::Diagnostic;

pub(crate) struct CommandOutcome {
    pub(crate) data: Value,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl CommandOutcome {
    pub(crate) fn ok(data: Value) -> Self {
        Self {
            data,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn with_diagnostics(data: Value, diagnostics: Vec<Diagnostic>) -> Self {
        Self { data, diagnostics }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CommandError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl CommandError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
