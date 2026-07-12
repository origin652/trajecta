//! # Contract: Case commands
//!
//! Case commands load local documents, invoke schema/reference/intent APIs,
//! and render diagnostics or a resolved document. Legacy migration stays in
//! the separate GPL adapter.

use std::path::PathBuf;

use trajecta_case::intent::ValidationIntent;

/// Case and RunProfile command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaseCommand {
    /// Validate document shape, references, and selected intent.
    Validate {
        /// Local Case or RunProfile path.
        path: PathBuf,
        /// Operation-specific validation intent.
        intent: ValidationIntent,
    },
    /// Expand local component references and emit normalized content.
    Resolve {
        /// Local Case or RunProfile path.
        path: PathBuf,
    },
}
