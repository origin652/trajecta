//! # Contract: local doctor command
//!
//! The A1 doctor inspects only local configuration, project and filesystem readiness.

/// Doctor command selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DoctorCommand {
    /// Include local lock and SQLite readiness checks.
    pub deep: bool,
}
