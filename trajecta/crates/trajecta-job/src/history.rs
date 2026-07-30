//! # Contract: attempt history, rerun verification, and dry-run pruning
//!
//! History operations preserve every attempt. Forget changes list visibility,
//! while pruning only describes files that a future product could remove.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId};

use crate::backend::JobBackendError;
use crate::model::{JobReceipt, JobSnapshot, is_normal_absolute_path, is_uuid_v7};

/// Frozen dry-run pruning schema identity.
pub const PRUNE_PLAN_SCHEMA_ID: &str = "trajecta.prune-plan/v1";

/// Full-verification evidence recorded for one successful attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullVerificationRecord {
    /// Time at which the full verifier accepted the on-disk result.
    pub verified_at: Timestamp,
    /// Canonical output digest independently recomputed by the verifier.
    pub canonical_output_sha256: String,
}

/// Durable history metadata for one attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobAttemptHistory {
    /// Scheduler and artifact identity for the attempt.
    pub snapshot: JobSnapshot,
    /// Whether the series is visible in routine `job list` output.
    pub visible_in_routine_list: bool,
    /// Full-verification evidence, when the attempt passed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_verification: Option<FullVerificationRecord>,
    /// First later fully verified Complete attempt that superseded this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<RunId>,
}

impl JobAttemptHistory {
    /// Validates the history metadata against the frozen lifecycle contract.
    pub fn validate(&self) -> Result<(), JobBackendError> {
        self.snapshot
            .validate()
            .map_err(|error| JobBackendError::Storage(error.code().into()))?;
        if let Some(verification) = &self.full_verification {
            if !is_sha256(&verification.canonical_output_sha256)
                || verification.verified_at
                    < self
                        .snapshot
                        .finished_at
                        .unwrap_or(self.snapshot.created_at)
                || !self.snapshot.state.run_success()
            {
                return Err(JobBackendError::Storage(
                    "invalid persisted full-verification record".into(),
                ));
            }
        }
        if self.superseded_by.as_ref().is_some_and(|run_id| {
            !is_uuid_v7(&run_id.0)
                || run_id == &self.snapshot.run_id
                || !self.snapshot.state.is_terminal()
        }) {
            return Err(JobBackendError::Storage(
                "invalid persisted supersession identity".into(),
            ));
        }
        Ok(())
    }
}

/// One path considered by a dry-run pruning audit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PruneCandidate {
    /// Logical series identity.
    pub job_series_id: JobSeriesId,
    /// Attempt run identity.
    pub run_id: RunId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Absolute artifact path from the durable catalog.
    pub path: PathBuf,
    /// Current recursively observed byte size without following links.
    pub size_bytes: u64,
    /// Stable explanation of eligibility or protection.
    pub reason: String,
    /// Whether the path is forbidden from future removal.
    pub protected: bool,
    /// Fully verified Complete attempt that superseded this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<RunId>,
}

/// Deterministic plan that never deletes files.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrunePlan {
    /// Must equal [`PRUNE_PLAN_SCHEMA_ID`].
    pub schema_version: String,
    /// Always `dry_run` in M5.
    pub mode: String,
    /// Always false in M5.
    pub delete_enabled: bool,
    /// Attempt paths ordered by series, attempt, and run identity.
    pub candidates: Vec<PruneCandidate>,
}

impl PrunePlan {
    /// Validates the no-delete contract and deterministic candidate shape.
    pub fn validate(&self) -> Result<(), JobBackendError> {
        if self.schema_version != PRUNE_PLAN_SCHEMA_ID
            || self.mode != "dry_run"
            || self.delete_enabled
        {
            return Err(JobBackendError::Storage(
                "invalid persisted prune-plan contract".into(),
            ));
        }
        let mut previous = None;
        for candidate in &self.candidates {
            if !is_uuid_v7(&candidate.job_series_id.0)
                || !is_uuid_v7(&candidate.run_id.0)
                || candidate.attempt == 0
                || !is_normal_absolute_path(&candidate.path)
                || candidate.reason.trim().is_empty()
                || candidate
                    .superseded_by
                    .as_ref()
                    .is_some_and(|run_id| !is_uuid_v7(&run_id.0) || run_id == &candidate.run_id)
                || candidate.superseded_by.is_none()
                || (!candidate.protected
                    && candidate.reason != "superseded_by_verified_complete_attempt")
                || (candidate.protected && candidate.reason != "artifact_missing")
            {
                return Err(JobBackendError::Storage(
                    "invalid prune-plan candidate".into(),
                ));
            }
            let key = (
                candidate.job_series_id.0.as_str(),
                candidate.attempt,
                candidate.run_id.0.as_str(),
            );
            if previous.is_some_and(|previous| previous >= key) {
                return Err(JobBackendError::Storage(
                    "prune-plan candidates are not uniquely sorted".into(),
                ));
            }
            previous = Some(key);
        }
        Ok(())
    }
}

/// Higher-level history operations layered above the frozen scheduler backend.
pub trait JobHistoryBackend {
    /// Creates a queued attempt in an existing terminal series.
    fn rerun(&mut self, job_series_id: &JobSeriesId) -> Result<JobReceipt, JobBackendError>;

    /// Hides a terminal series from routine list output without deleting it.
    fn forget(&mut self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError>;

    /// Returns every durable attempt in a series in ascending attempt order.
    fn attempt_history(
        &self,
        job_series_id: &JobSeriesId,
    ) -> Result<Vec<JobAttemptHistory>, JobBackendError>;

    /// Returns one exact attempt by run identity.
    fn attempt(&self, run_id: &RunId) -> Result<JobAttemptHistory, JobBackendError>;

    /// Records successful full verification and supersedes older attempts.
    fn record_full_verification(
        &mut self,
        run_id: &RunId,
        canonical_output_sha256: &str,
    ) -> Result<JobAttemptHistory, JobBackendError>;

    /// Produces a deterministic dry-run plan without changing the filesystem.
    fn prune_plan(&self) -> Result<PrunePlan, JobBackendError>;
}

pub(crate) fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
