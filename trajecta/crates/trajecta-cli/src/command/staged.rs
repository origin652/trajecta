//! # Contract: later-stage command selections
//!
//! Commands owned by M5-A2/A3 parse completely in A1 but return a stable stage-unavailable diagnostic.

use std::path::PathBuf;

/// Parsed run input mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunInput {
    /// Project/profile selection.
    Project {
        /// Project root or index path.
        project: PathBuf,
        /// Named project profile.
        profile: String,
    },
    /// Explicit Case and RunProfile paths.
    Direct {
        /// Case document path.
        case: PathBuf,
        /// RunProfile document path.
        run_profile: PathBuf,
    },
}

/// Parsed daemon-backed run command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedRunCommand {
    /// Future run input mode.
    pub input: RunInput,
    /// Whether a future daemon should detach.
    pub detach: bool,
}

/// Parsed job command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobCommand {
    /// List jobs.
    List,
    /// Inspect a job id.
    Status {
        /// Logical job-series id.
        job_id: String,
    },
    /// Wait for a job id.
    Wait {
        /// Logical job-series id.
        job_id: String,
    },
    /// Stream job events.
    Events {
        /// Optional logical job-series filter.
        job_id: Option<String>,
        /// Return events strictly after this global cursor.
        since: Option<u64>,
        /// Continue polling until the selected job terminates, or indefinitely for all jobs.
        follow: bool,
    },
    /// Cancel a job id.
    Cancel {
        /// Logical job-series id.
        job_id: String,
        /// Immediately stop the worker and retain forensic artifacts.
        force: bool,
    },
    /// Rerun a job id.
    Rerun(String),
    /// Forget a job id.
    Forget(String),
    /// Produce a dry-run pruning plan.
    Prune,
}

/// Parsed future result command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultCommand {
    /// Inspect a result id/path.
    Inspect(String),
    /// Verify a result id/path.
    Verify(String),
    /// Stream trajectories for a result id/path.
    Trajectory(String),
}
