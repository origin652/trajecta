//! # Contract: project orchestration commands
//!
//! Typed selections for progressive local project editing and deterministic data planning.

use std::path::PathBuf;

/// Project command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectCommand {
    /// Create a project skeleton at an optional path.
    Init {
        /// Optional project root; defaults to the current directory.
        path: Option<PathBuf>,
        /// Required project display name.
        name: String,
    },
    /// Show the derived project state.
    Status,
    /// Render the complete project index and derived state.
    Show,
    /// Read a project selector.
    Get {
        /// `index`, `case`, or `profile` selector.
        selector: String,
    },
    /// Atomically update a project selector.
    Set {
        /// `index`, `case`, or `profile` selector.
        selector: String,
        /// JSON/YAML scalar or structured value text.
        value: String,
    },
    /// Atomically remove a project selector.
    Unset {
        /// `index`, `case`, or `profile` selector.
        selector: String,
    },
    /// Validate the index and all present documents.
    Validate,
    /// Produce a deterministic data-plan.
    DataPlan {
        /// Optional output path; `-` is standard output.
        output: Option<PathBuf>,
    },
    /// Parse the A-owned finalization command.
    Finalize,
}
