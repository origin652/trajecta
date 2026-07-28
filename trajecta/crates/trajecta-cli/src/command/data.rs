//! # Contract: data inventory commands
//!
//! Data commands inspect already-present files or create immutable lock
//! descriptions. Remote acquisition is outside the v0 command contract.

use std::path::PathBuf;

/// Dataset lock and source-inspection command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataCommand {
    /// Build an immutable lock from a local dataset and explicit profile.
    Lock {
        /// Local dataset root.
        root: PathBuf,
        /// Exact profile name.
        profile: String,
        /// Case from which coverage and capability requirements are resolved.
        case: PathBuf,
        /// Destination lockfile path.
        output: PathBuf,
        /// Whether an existing destination may be atomically replaced.
        replace: bool,
    },
    /// Inspect one local source file without decoding a full simulation.
    Inspect {
        /// Local GRIB or NetCDF file.
        file: PathBuf,
    },
}
