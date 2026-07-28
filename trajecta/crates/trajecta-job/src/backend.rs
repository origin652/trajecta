//! # Contract: scheduler backend control-plane boundary
//!
//! Backends accept already-resolved control requests and expose durable state.
//! They do not resolve scientific documents or execute particle algorithms.

use std::error::Error;
use std::fmt;

use trajecta_core::manifest::JobSeriesId;

use crate::model::{
    CancelMode, EventQuery, JobEvent, JobListQuery, JobReceipt, JobSnapshot, SubmitRequest,
};

/// Control operations required from a scheduler backend.
pub trait JobBackend {
    /// Durably accepts a new attempt and returns its allocated identities.
    fn submit(&mut self, request: SubmitRequest) -> Result<JobReceipt, JobBackendError>;

    /// Returns bounded durable snapshots in backend-defined FIFO order.
    fn list(&self, query: &JobListQuery) -> Result<Vec<JobSnapshot>, JobBackendError>;

    /// Returns the latest snapshot for one logical job series.
    fn status(&self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError>;

    /// Requests safe or forced cancellation and returns the resulting snapshot.
    fn cancel(
        &mut self,
        job_series_id: &JobSeriesId,
        mode: CancelMode,
    ) -> Result<JobSnapshot, JobBackendError>;

    /// Returns persistent events strictly after the supplied cursor.
    fn events(&self, query: &EventQuery) -> Result<Vec<JobEvent>, JobBackendError>;
}

/// Scheduler backend control-plane failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobBackendError {
    /// The request violates a frozen control-plane contract.
    InvalidRequest(String),
    /// The requested logical job series does not exist.
    NotFound,
    /// Current durable state conflicts with the requested transition.
    Conflict(String),
    /// Backend IPC or worker control is temporarily unavailable.
    Unavailable(String),
    /// Durable catalog or event storage failed.
    Storage(String),
}

impl JobBackendError {
    /// Stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "job_backend.invalid_request",
            Self::NotFound => "job_backend.not_found",
            Self::Conflict(_) => "job_backend.conflict",
            Self::Unavailable(_) => "job_backend.unavailable",
            Self::Storage(_) => "job_backend.storage",
        }
    }
}

impl fmt::Display for JobBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message)
            | Self::Conflict(message)
            | Self::Unavailable(message)
            | Self::Storage(message) => formatter.write_str(message),
            Self::NotFound => formatter.write_str("job series not found"),
        }
    }
}

impl Error for JobBackendError {}
