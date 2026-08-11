//! # Contract: later-stage command selections
//!
//! Commands owned by M5-A2/A3 parse completely in A1 but return a stable stage-unavailable diagnostic.

use std::path::PathBuf;

use trajecta_case::model::time::Timestamp;

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

/// Parsed result command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResultCommand {
    /// Inspect a result id/path.
    Inspect(String),
    /// Verify a result id/path.
    Verify {
        /// Job-series id, run id, or run-directory path.
        result: String,
        /// Run the lifecycle, quality, mass, and row audits.
        full: bool,
    },
    /// Stream trajectories for a result id/path.
    Trajectory {
        /// Job-series id, run id, or run-directory path.
        result: String,
        /// Explicit bounded selection or the complete particle set.
        selection: TrajectorySelection,
    },
    /// Query physical-process summaries and optional event rows.
    Processes {
        /// Job-series id, run id, or run-directory path.
        result: String,
        /// Stable filter and streaming selection.
        selection: ProcessSelection,
    },
    /// Write the deterministic `run-report.md` product for a result id/path.
    Report {
        /// Job-series id, run id, or run-directory path.
        result: String,
    },
}

/// Particle selection for trajectory streaming.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrajectorySelection {
    /// Sorted unique stable particle identities.
    ParticleIds(Vec<u64>),
    /// Every particle, streamed without whole-result buffering.
    All,
}

/// Filters for `result processes`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessSelection {
    /// Sorted unique particle identities.
    pub particle_ids: Vec<u64>,
    /// Sorted unique physical module identifiers.
    pub module_ids: Vec<String>,
    /// Sorted unique substance identifiers.
    pub substance_ids: Vec<String>,
    /// Inclusive UTC lower bound.
    pub start: Option<Timestamp>,
    /// Inclusive UTC upper bound.
    pub end: Option<Timestamp>,
    /// Include discrete process-event rows.
    pub events: bool,
    /// Optional maximum number of event rows to return.
    pub max_records: Option<u64>,
}
