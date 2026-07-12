//! Executable checks for CLI envelope semantics.

use trajecta_case::diagnostic::Diagnostic;
use trajecta_cli::envelope::CommandEnvelope;

#[test]
fn error_diagnostics_determine_exit_status() {
    let envelope: CommandEnvelope<()> = CommandEnvelope {
        data: None,
        diagnostics: vec![Diagnostic::error("test.error", "expected test error")],
    };
    assert!(!envelope.is_success());
    assert_eq!(envelope.exit_code(), 1);
}
