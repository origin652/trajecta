//! # Contract: meteorology probe and replay commands
//!
//! Commands construct a meteorology engine from resolved documents, prepare
//! explicit query batches, and render output. They never implement reader or
//! interpolation details themselves.

use std::path::PathBuf;

/// Meteorology command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetCommand {
    /// Validate availability and report source/grid/vertical metadata.
    Probe {
        /// Local Case path.
        case: PathBuf,
        /// Local RunProfile path.
        run_profile: PathBuf,
    },
    /// Execute a deterministic forward or backward meteorology replay.
    Replay {
        /// Local Case path.
        case: PathBuf,
        /// Local RunProfile path.
        run_profile: PathBuf,
        /// Local query-batch document.
        query: PathBuf,
        /// Whether to include detailed provenance and interpolation explanation.
        explain: bool,
    },
}
