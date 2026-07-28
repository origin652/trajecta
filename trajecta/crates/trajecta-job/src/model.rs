//! # Contract: job identities, states, events, and resource requests
//!
//! Types in this module are control-plane records. They describe what should
//! run and what has happened, but cannot execute or alter a simulation.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId, RunLifecycleStatus};

/// Frozen job-record machine schema identity.
pub const JOB_RECORD_SCHEMA_ID: &str = "trajecta.job-record/v1";
/// Frozen job-event machine schema identity.
pub const JOB_EVENT_SCHEMA_ID: &str = "trajecta.job-event/v1";

/// Derived readiness state of a project directory.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    /// Required Case or RunProfile content is still incomplete.
    Draft,
    /// Logical configuration is complete but data or locks may be pending.
    Configured,
    /// Data, locks, paths, and resolved documents are ready to run.
    Finalized,
}

/// Persistent scheduler-visible state of one job attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Accepted into the durable queue.
    Queued,
    /// Resources were reserved and a worker is being started.
    Starting,
    /// Worker execution is active.
    Running,
    /// Safe cancellation was requested and awaits a macro-step boundary.
    Cancelling,
    /// Simulation completed without abnormal particle termination.
    Complete,
    /// Simulation finalized but contained abnormal particle termination.
    CompletedWithParticleErrors,
    /// A controlled fatal run-level error was terminalized.
    Failed,
    /// A safe user cancellation finalized partial outputs.
    Cancelled,
    /// The worker disappeared or was force-stopped before safe finalization.
    Interrupted,
}

impl JobState {
    /// Returns whether the scheduler must never dispatch this attempt again.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Complete
                | Self::CompletedWithParticleErrors
                | Self::Failed
                | Self::Cancelled
                | Self::Interrupted
        )
    }

    /// Returns whether the simulation itself succeeded.
    #[must_use]
    pub const fn run_success(self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Returns the frozen foreground-run and `job wait` terminal exit code.
    #[must_use]
    pub const fn wait_exit_code(self) -> Option<i32> {
        match self {
            Self::Complete => Some(0),
            Self::CompletedWithParticleErrors
            | Self::Failed
            | Self::Cancelled
            | Self::Interrupted => Some(1),
            Self::Queued | Self::Starting | Self::Running | Self::Cancelling => None,
        }
    }

    /// Returns whether the frozen job state machine permits `next`.
    #[must_use]
    pub const fn allows_transition_to(self, next: Self) -> bool {
        match self {
            Self::Queued => matches!(next, Self::Starting | Self::Cancelled),
            Self::Starting => matches!(
                next,
                Self::Running | Self::Cancelling | Self::Failed | Self::Interrupted
            ),
            Self::Running => matches!(
                next,
                Self::Cancelling
                    | Self::Complete
                    | Self::CompletedWithParticleErrors
                    | Self::Failed
                    | Self::Interrupted
            ),
            Self::Cancelling => {
                matches!(next, Self::Cancelled | Self::Failed | Self::Interrupted)
            }
            Self::Complete
            | Self::CompletedWithParticleErrors
            | Self::Failed
            | Self::Cancelled
            | Self::Interrupted => false,
        }
    }
}

impl From<RunLifecycleStatus> for JobState {
    fn from(status: RunLifecycleStatus) -> Self {
        match status {
            RunLifecycleStatus::Running => Self::Running,
            RunLifecycleStatus::Complete => Self::Complete,
            RunLifecycleStatus::CompletedWithParticleErrors => Self::CompletedWithParticleErrors,
            RunLifecycleStatus::Failed => Self::Failed,
            RunLifecycleStatus::Cancelled => Self::Cancelled,
            RunLifecycleStatus::Interrupted => Self::Interrupted,
        }
    }
}

/// Requested cancellation strength.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelMode {
    /// Poll at a macro-step boundary and legally finalize partial outputs.
    Safe,
    /// Stop the worker immediately and mark the attempt interrupted.
    Force,
}

/// Explicit scheduler resource request for one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequest {
    /// Scheduler CPU slots reserved for the worker.
    pub cpu_slots: u32,
    /// Scheduler memory reservation in mebibytes.
    pub memory_mib: u64,
    /// Simulation worker-thread count.
    pub worker_threads: u32,
}

impl ResourceRequest {
    /// Validates positive resources and prevents worker oversubscription.
    pub fn validate(self) -> Result<(), JobModelError> {
        if self.cpu_slots == 0
            || self.memory_mib == 0
            || self.worker_threads == 0
            || self.worker_threads > self.cpu_slots
        {
            return Err(JobModelError::InvalidResources);
        }
        Ok(())
    }
}

/// Resolved source documents submitted to a backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunInput {
    /// Project-directory mode selecting one concrete named RunProfile.
    Project {
        /// Absolute normalized project root.
        project_root: PathBuf,
        /// Non-empty profile name from the project index.
        profile_name: String,
    },
    /// Direct mode using explicit Case and RunProfile documents.
    Direct {
        /// Absolute normalized Case path.
        case_path: PathBuf,
        /// Absolute normalized RunProfile path.
        run_profile_path: PathBuf,
    },
}

impl RunInput {
    fn validate(&self) -> Result<(), JobModelError> {
        match self {
            Self::Project {
                project_root,
                profile_name,
            } => {
                if !is_normal_absolute_path(project_root) || profile_name.trim().is_empty() {
                    return Err(JobModelError::InvalidInput);
                }
            }
            Self::Direct {
                case_path,
                run_profile_path,
            } => {
                if !is_normal_absolute_path(case_path) || !is_normal_absolute_path(run_profile_path)
                {
                    return Err(JobModelError::InvalidInput);
                }
            }
        }
        Ok(())
    }
}

/// Immutable request accepted by a job backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    /// Project or direct document selection.
    pub input: RunInput,
    /// Explicit resource reservation.
    pub resources: ResourceRequest,
}

impl SubmitRequest {
    /// Validates normalized inputs and explicit resources.
    pub fn validate(&self) -> Result<(), JobModelError> {
        self.input.validate()?;
        self.resources.validate()
    }
}

/// Identity returned after durable queue acceptance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobReceipt {
    /// Logical series identity.
    pub job_series_id: JobSeriesId,
    /// Unique attempt run identity.
    pub run_id: RunId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Initial durable state, normally queued.
    pub state: JobState,
}

impl JobReceipt {
    /// Validates durable queue-acceptance identity and initial state.
    pub fn validate(&self) -> Result<(), JobModelError> {
        if !is_uuid_v7(&self.job_series_id.0) || !is_uuid_v7(&self.run_id.0) || self.attempt == 0 {
            return Err(JobModelError::InvalidIdentity);
        }
        if self.state != JobState::Queued {
            return Err(JobModelError::InvalidLifecycle);
        }
        Ok(())
    }
}

/// One durable job-attempt snapshot returned by status and list commands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSnapshot {
    /// Must equal [`JOB_RECORD_SCHEMA_ID`].
    pub schema_version: String,
    /// Logical series identity.
    pub job_series_id: JobSeriesId,
    /// Unique attempt run identity.
    pub run_id: RunId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Current durable state.
    pub state: JobState,
    /// Original normalized input selection.
    pub input: RunInput,
    /// Reserved resources.
    pub resources: ResourceRequest,
    /// Queue acceptance time.
    pub created_at: Timestamp,
    /// Worker start time when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Terminal observation time when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// Current FIFO position for queued jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<u64>,
    /// Absolute normalized attempt output directory when allocated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_directory: Option<PathBuf>,
}

impl JobSnapshot {
    /// Validates identity, time, lifecycle, input, and resource coherence.
    pub fn validate(&self) -> Result<(), JobModelError> {
        if self.schema_version != JOB_RECORD_SCHEMA_ID
            || !is_uuid_v7(&self.job_series_id.0)
            || !is_uuid_v7(&self.run_id.0)
            || self.attempt == 0
        {
            return Err(JobModelError::InvalidIdentity);
        }
        self.input.validate()?;
        self.resources.validate()?;
        if self
            .started_at
            .is_some_and(|started| started < self.created_at)
            || self
                .finished_at
                .is_some_and(|finished| finished < self.started_at.unwrap_or(self.created_at))
            || (self.state.is_terminal() != self.finished_at.is_some())
            || (!matches!(self.state, JobState::Queued) && self.queue_position.is_some())
            || self
                .output_directory
                .as_deref()
                .is_some_and(|path| !is_normal_absolute_path(path))
        {
            return Err(JobModelError::InvalidLifecycle);
        }
        Ok(())
    }
}

/// Frozen event classification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEventKind {
    /// Durable state changed.
    StateTransition,
    /// Macro-step progress counters changed.
    Progress,
    /// CPU, memory, or elapsed-time observation.
    Resource,
    /// Non-fatal condition requiring attention.
    Warning,
    /// Fatal or terminal error diagnostic.
    Error,
    /// Auditable output artifact was created or finalized.
    Artifact,
}

/// Task-level progress counters emitted at bounded cadence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobProgress {
    /// Zero-based completed macro-step count.
    pub completed_macro_steps: u64,
    /// Physical simulation time when execution has started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulation_time: Option<Timestamp>,
    /// Currently active particle count.
    pub active_particles: u64,
    /// Cumulative normal termination count.
    pub normal_terminations: u64,
    /// Cumulative abnormal termination count.
    pub abnormal_terminations: u64,
}

/// Task-level resource observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceObservation {
    /// Elapsed wall time in milliseconds.
    pub wall_time_ms: u64,
    /// Accumulated process CPU time in milliseconds when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_time_ms: Option<u64>,
    /// Current resident working set in bytes when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
}

/// One persistent fan-out event; reading an event never consumes it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobEvent {
    /// Must equal [`JOB_EVENT_SCHEMA_ID`].
    pub schema_version: String,
    /// Global monotonically increasing cursor, starting at one.
    pub sequence: u64,
    /// Event persistence time.
    pub emitted_at: Timestamp,
    /// Logical series identity.
    pub job_series_id: JobSeriesId,
    /// Unique attempt run identity.
    pub run_id: RunId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Event classification.
    pub kind: JobEventKind,
    /// State observed after this event.
    pub state: JobState,
    /// Optional progress counters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    /// Optional resource observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceObservation>,
    /// Optional stable diagnostic code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Optional human-readable message without secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional absolute normalized artifact path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<PathBuf>,
}

impl JobEvent {
    /// Validates schema, cursor, identity, diagnostic, and path fields.
    pub fn validate(&self) -> Result<(), JobModelError> {
        if self.schema_version != JOB_EVENT_SCHEMA_ID
            || self.sequence == 0
            || !is_uuid_v7(&self.job_series_id.0)
            || !is_uuid_v7(&self.run_id.0)
            || self.attempt == 0
        {
            return Err(JobModelError::InvalidIdentity);
        }
        if self
            .code
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
            || self
                .message
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || self
                .artifact_path
                .as_deref()
                .is_some_and(|path| !is_normal_absolute_path(path))
        {
            return Err(JobModelError::InvalidEvent);
        }
        Ok(())
    }
}

/// Bounded query used by `job list`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobListQuery {
    /// Optional exact state filter.
    pub states: Vec<JobState>,
    /// Maximum returned rows, in the range 1..=1000.
    pub limit: usize,
}

impl JobListQuery {
    /// Validates the bounded result limit.
    pub fn validate(&self) -> Result<(), JobModelError> {
        if self.limit == 0 || self.limit > 1000 {
            return Err(JobModelError::InvalidQuery);
        }
        Ok(())
    }
}

/// Cursor query used by one-shot and follow event readers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventQuery {
    /// Optional logical-series filter; absent means all jobs.
    pub job_series_id: Option<JobSeriesId>,
    /// Return records strictly after this global sequence.
    pub after_sequence: Option<u64>,
    /// Maximum returned rows, in the range 1..=10000.
    pub limit: usize,
}

impl EventQuery {
    /// Validates identity and bounded result limit.
    pub fn validate(&self) -> Result<(), JobModelError> {
        if self.limit == 0
            || self.limit > 10_000
            || self
                .job_series_id
                .as_ref()
                .is_some_and(|value| !is_uuid_v7(&value.0))
        {
            return Err(JobModelError::InvalidQuery);
        }
        Ok(())
    }
}

/// Job-model validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobModelError {
    /// Identity or attempt is malformed.
    InvalidIdentity,
    /// Input paths or names are malformed.
    InvalidInput,
    /// Resource request is zero or oversubscribed.
    InvalidResources,
    /// State and timestamps conflict.
    InvalidLifecycle,
    /// Event diagnostics or artifact path are malformed.
    InvalidEvent,
    /// Query limit or identity filter is invalid.
    InvalidQuery,
}

impl JobModelError {
    /// Stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidIdentity => "job.invalid_identity",
            Self::InvalidInput => "job.invalid_input",
            Self::InvalidResources => "job.invalid_resources",
            Self::InvalidLifecycle => "job.invalid_lifecycle",
            Self::InvalidEvent => "job.invalid_event",
            Self::InvalidQuery => "job.invalid_query",
        }
    }
}

fn is_normal_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

fn is_uuid_v7(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
        && bytes[14] == b'7'
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}
