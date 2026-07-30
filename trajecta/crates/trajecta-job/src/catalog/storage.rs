use std::path::PathBuf;

use rusqlite::Row;
use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId};

use crate::backend::JobBackendError;
use crate::model::{
    CancelMode, JOB_EVENT_SCHEMA_ID, JOB_RECORD_SCHEMA_ID, JobEvent, JobEventKind, JobProgress,
    JobSnapshot, JobState, ResourceObservation, ResourceRequest, RunInput,
};

use super::{
    DaemonLease, WorkerLease, corrupt_json_error, parse_event_kind_sql, parse_state_sql,
    storage_error,
};

pub(super) fn collect_events<I>(rows: I) -> Result<Vec<StoredEvent>, JobBackendError>
where
    I: IntoIterator<Item = rusqlite::Result<StoredEvent>>,
{
    let mut events = Vec::new();
    for row in rows {
        events.push(row.map_err(storage_error)?);
    }
    Ok(events)
}

#[derive(Debug)]
pub(super) struct StoredJob {
    pub(super) job_series_id: JobSeriesId,
    pub(super) run_id: RunId,
    pub(super) attempt: u32,
    pub(super) state: JobState,
    pub(super) input_json: String,
    pub(super) resources_json: String,
    pub(super) created_seconds: i64,
    pub(super) created_nanosecond: i64,
    pub(super) started_seconds: Option<i64>,
    pub(super) started_nanosecond: Option<i64>,
    pub(super) finished_seconds: Option<i64>,
    pub(super) finished_nanosecond: Option<i64>,
    pub(super) output_directory: Option<String>,
}

impl StoredJob {
    pub(super) fn into_snapshot(
        self,
        queue_position: Option<u64>,
    ) -> Result<JobSnapshot, JobBackendError> {
        let snapshot = JobSnapshot {
            schema_version: JOB_RECORD_SCHEMA_ID.into(),
            job_series_id: self.job_series_id,
            run_id: self.run_id,
            attempt: self.attempt,
            state: self.state,
            input: serde_json::from_str::<RunInput>(&self.input_json)
                .map_err(corrupt_json_error)?,
            resources: serde_json::from_str::<ResourceRequest>(&self.resources_json)
                .map_err(corrupt_json_error)?,
            created_at: timestamp_from_parts(self.created_seconds, self.created_nanosecond)?,
            started_at: optional_timestamp(self.started_seconds, self.started_nanosecond)?,
            finished_at: optional_timestamp(self.finished_seconds, self.finished_nanosecond)?,
            queue_position,
            output_directory: self.output_directory.map(PathBuf::from),
        };
        snapshot
            .validate()
            .map_err(|error| JobBackendError::Storage(error.code().into()))?;
        Ok(snapshot)
    }
}

pub(super) fn stored_job_from_row(row: &Row<'_>) -> rusqlite::Result<StoredJob> {
    let attempt = row.get::<_, i64>(2)?;
    let state = row.get::<_, String>(3)?;
    Ok(StoredJob {
        job_series_id: JobSeriesId(row.get(0)?),
        run_id: RunId(row.get(1)?),
        attempt: u32::try_from(attempt).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        state: parse_state_sql(3, &state)?,
        input_json: row.get(4)?,
        resources_json: row.get(5)?,
        created_seconds: row.get(6)?,
        created_nanosecond: row.get(7)?,
        started_seconds: row.get(8)?,
        started_nanosecond: row.get(9)?,
        finished_seconds: row.get(10)?,
        finished_nanosecond: row.get(11)?,
        output_directory: row.get(12)?,
    })
}

#[derive(Debug)]
pub(super) struct StoredEvent {
    sequence: u64,
    emitted_seconds: i64,
    emitted_nanosecond: i64,
    job_series_id: JobSeriesId,
    run_id: RunId,
    attempt: u32,
    kind: JobEventKind,
    state: JobState,
    progress_json: Option<String>,
    resource_json: Option<String>,
    code: Option<String>,
    message: Option<String>,
    artifact_path: Option<String>,
}

impl StoredEvent {
    pub(super) fn into_event(self) -> Result<JobEvent, JobBackendError> {
        let event = JobEvent {
            schema_version: JOB_EVENT_SCHEMA_ID.into(),
            sequence: self.sequence,
            emitted_at: timestamp_from_parts(self.emitted_seconds, self.emitted_nanosecond)?,
            job_series_id: self.job_series_id,
            run_id: self.run_id,
            attempt: self.attempt,
            kind: self.kind,
            state: self.state,
            progress: self
                .progress_json
                .map(|value| serde_json::from_str::<JobProgress>(&value))
                .transpose()
                .map_err(corrupt_json_error)?,
            resource: self
                .resource_json
                .map(|value| serde_json::from_str::<ResourceObservation>(&value))
                .transpose()
                .map_err(corrupt_json_error)?,
            code: self.code,
            message: self.message,
            artifact_path: self.artifact_path.map(PathBuf::from),
        };
        event
            .validate()
            .map_err(|error| JobBackendError::Storage(error.code().into()))?;
        Ok(event)
    }
}

pub(super) fn stored_event_from_row(row: &Row<'_>) -> rusqlite::Result<StoredEvent> {
    let sequence = row.get::<_, i64>(0)?;
    let attempt = row.get::<_, i64>(5)?;
    let kind = row.get::<_, String>(6)?;
    let state = row.get::<_, String>(7)?;
    Ok(StoredEvent {
        sequence: u64::try_from(sequence).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        emitted_seconds: row.get(1)?,
        emitted_nanosecond: row.get(2)?,
        job_series_id: JobSeriesId(row.get(3)?),
        run_id: RunId(row.get(4)?),
        attempt: u32::try_from(attempt).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        kind: parse_event_kind_sql(6, &kind)?,
        state: parse_state_sql(7, &state)?,
        progress_json: row.get(8)?,
        resource_json: row.get(9)?,
        code: row.get(10)?,
        message: row.get(11)?,
        artifact_path: row.get(12)?,
    })
}

#[derive(Debug)]
pub(super) struct StoredLease {
    pub(super) run_id: RunId,
    pub(super) daemon_instance_id: String,
    pub(super) worker_pid: i64,
    pub(super) worker_start_token: String,
    pub(super) heartbeat_seconds: i64,
    pub(super) heartbeat_nanosecond: i64,
    pub(super) cancel_mode: Option<String>,
}

#[derive(Debug)]
pub(super) struct StoredDaemonLease {
    daemon_instance_id: String,
    daemon_pid: i64,
    daemon_start_token: String,
    heartbeat_seconds: i64,
    heartbeat_nanosecond: i64,
}

pub(super) fn stored_daemon_lease_from_row(row: &Row<'_>) -> rusqlite::Result<StoredDaemonLease> {
    Ok(StoredDaemonLease {
        daemon_instance_id: row.get(0)?,
        daemon_pid: row.get(1)?,
        daemon_start_token: row.get(2)?,
        heartbeat_seconds: row.get(3)?,
        heartbeat_nanosecond: row.get(4)?,
    })
}

pub(super) fn daemon_lease_from_stored(
    stored: StoredDaemonLease,
) -> Result<DaemonLease, JobBackendError> {
    let daemon_pid = u32::try_from(stored.daemon_pid)
        .map_err(|_| JobBackendError::Storage("invalid daemon pid".into()))?;
    let lease = DaemonLease {
        daemon_instance_id: stored.daemon_instance_id,
        daemon_pid,
        daemon_start_token: stored.daemon_start_token,
        heartbeat_at: timestamp_from_parts(stored.heartbeat_seconds, stored.heartbeat_nanosecond)?,
    };
    lease.validate()?;
    Ok(lease)
}

pub(super) fn stored_lease_from_row(row: &Row<'_>, run_id: RunId) -> rusqlite::Result<StoredLease> {
    Ok(StoredLease {
        run_id,
        daemon_instance_id: row.get(0)?,
        worker_pid: row.get(1)?,
        worker_start_token: row.get(2)?,
        heartbeat_seconds: row.get(3)?,
        heartbeat_nanosecond: row.get(4)?,
        cancel_mode: row.get(5)?,
    })
}

pub(super) fn worker_lease_from_stored(
    stored: StoredLease,
) -> Result<WorkerLease, JobBackendError> {
    let worker_pid = u32::try_from(stored.worker_pid)
        .map_err(|_| JobBackendError::Storage("invalid worker pid".into()))?;
    let cancel_mode = match stored.cancel_mode.as_deref() {
        None => None,
        Some("safe") => Some(CancelMode::Safe),
        Some("force") => Some(CancelMode::Force),
        Some(value) => {
            return Err(JobBackendError::Storage(format!(
                "invalid persisted cancel mode {value}"
            )));
        }
    };
    let lease = WorkerLease {
        run_id: stored.run_id,
        daemon_instance_id: stored.daemon_instance_id,
        worker_pid,
        worker_start_token: stored.worker_start_token,
        heartbeat_at: timestamp_from_parts(stored.heartbeat_seconds, stored.heartbeat_nanosecond)?,
        cancel_mode,
    };
    lease.validate()?;
    Ok(lease)
}

pub(super) fn optional_timestamp(
    seconds: Option<i64>,
    nanosecond: Option<i64>,
) -> Result<Option<Timestamp>, JobBackendError> {
    match (seconds, nanosecond) {
        (None, None) => Ok(None),
        (Some(seconds), Some(nanosecond)) => timestamp_from_parts(seconds, nanosecond).map(Some),
        _ => Err(JobBackendError::Storage(
            "partial timestamp persisted in job catalog".into(),
        )),
    }
}

pub(super) fn timestamp_from_parts(
    seconds: i64,
    nanosecond: i64,
) -> Result<Timestamp, JobBackendError> {
    let nanosecond = u32::try_from(nanosecond)
        .map_err(|_| JobBackendError::Storage("invalid timestamp nanosecond".into()))?;
    Timestamp::new(seconds, nanosecond)
        .map_err(|_| JobBackendError::Storage("invalid timestamp nanosecond".into()))
}

pub(super) const JOB_SELECT: &str =
    "SELECT job_series_id, run_id, attempt, state, input_json, resources_json,
            created_seconds, created_nanosecond, started_seconds, started_nanosecond,
            finished_seconds, finished_nanosecond, output_directory
     FROM jobs";

pub(super) const EVENT_SELECT: &str =
    "SELECT sequence, emitted_seconds, emitted_nanosecond, job_series_id, run_id,
            attempt, kind, state, progress_json, resource_json, code, message, artifact_path
     FROM events";

pub(super) const CATALOG_SCHEMA: &str = r#"
CREATE TABLE jobs (
    queue_order INTEGER PRIMARY KEY AUTOINCREMENT,
    job_series_id TEXT NOT NULL,
    run_id TEXT NOT NULL UNIQUE,
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'starting', 'running', 'cancelling', 'complete',
        'completed_with_particle_errors', 'failed', 'cancelled', 'interrupted'
    )),
    input_json TEXT NOT NULL,
    resources_json TEXT NOT NULL,
    created_seconds INTEGER NOT NULL,
    created_nanosecond INTEGER NOT NULL CHECK (
        created_nanosecond >= 0 AND created_nanosecond < 1000000000
    ),
    started_seconds INTEGER,
    started_nanosecond INTEGER CHECK (
        started_nanosecond IS NULL OR
        (started_nanosecond >= 0 AND started_nanosecond < 1000000000)
    ),
    finished_seconds INTEGER,
    finished_nanosecond INTEGER CHECK (
        finished_nanosecond IS NULL OR
        (finished_nanosecond >= 0 AND finished_nanosecond < 1000000000)
    ),
    output_directory TEXT,
    head_bypass_count INTEGER NOT NULL DEFAULT 0 CHECK (
        head_bypass_count >= 0 AND head_bypass_count <= 3
    ),
    UNIQUE(job_series_id, attempt),
    CHECK ((started_seconds IS NULL) = (started_nanosecond IS NULL)),
    CHECK ((finished_seconds IS NULL) = (finished_nanosecond IS NULL))
);

CREATE INDEX jobs_series_attempt ON jobs(job_series_id, attempt DESC);
CREATE INDEX jobs_state_queue ON jobs(state, queue_order);

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    emitted_seconds INTEGER NOT NULL,
    emitted_nanosecond INTEGER NOT NULL CHECK (
        emitted_nanosecond >= 0 AND emitted_nanosecond < 1000000000
    ),
    job_series_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES jobs(run_id) ON DELETE RESTRICT,
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    kind TEXT NOT NULL CHECK (kind IN (
        'state_transition', 'progress', 'resource', 'warning', 'error', 'artifact'
    )),
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'starting', 'running', 'cancelling', 'complete',
        'completed_with_particle_errors', 'failed', 'cancelled', 'interrupted'
    )),
    progress_json TEXT,
    resource_json TEXT,
    code TEXT,
    message TEXT,
    artifact_path TEXT
);

CREATE INDEX events_series_sequence ON events(job_series_id, sequence);

CREATE TABLE worker_leases (
    run_id TEXT PRIMARY KEY REFERENCES jobs(run_id) ON DELETE RESTRICT,
    daemon_instance_id TEXT NOT NULL,
    worker_pid INTEGER NOT NULL CHECK (worker_pid >= 1),
    worker_start_token TEXT NOT NULL,
    heartbeat_seconds INTEGER NOT NULL,
    heartbeat_nanosecond INTEGER NOT NULL CHECK (
        heartbeat_nanosecond >= 0 AND heartbeat_nanosecond < 1000000000
    ),
    cancel_mode TEXT CHECK (cancel_mode IS NULL OR cancel_mode IN ('safe', 'force'))
);

CREATE TABLE daemon_lease (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    daemon_instance_id TEXT NOT NULL,
    daemon_pid INTEGER NOT NULL CHECK (daemon_pid >= 1),
    daemon_start_token TEXT NOT NULL,
    heartbeat_seconds INTEGER NOT NULL,
    heartbeat_nanosecond INTEGER NOT NULL CHECK (
        heartbeat_nanosecond >= 0 AND heartbeat_nanosecond < 1000000000
    )
);
"#;

pub(super) const HISTORY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS forgotten_series (
    job_series_id TEXT PRIMARY KEY,
    forgotten_seconds INTEGER NOT NULL,
    forgotten_nanosecond INTEGER NOT NULL CHECK (
        forgotten_nanosecond >= 0 AND forgotten_nanosecond < 1000000000
    )
);

CREATE TABLE IF NOT EXISTS full_verifications (
    run_id TEXT PRIMARY KEY REFERENCES jobs(run_id) ON DELETE RESTRICT,
    verified_seconds INTEGER NOT NULL,
    verified_nanosecond INTEGER NOT NULL CHECK (
        verified_nanosecond >= 0 AND verified_nanosecond < 1000000000
    ),
    canonical_output_sha256 TEXT NOT NULL CHECK (length(canonical_output_sha256) = 64)
);

CREATE TABLE IF NOT EXISTS attempt_supersession (
    run_id TEXT PRIMARY KEY REFERENCES jobs(run_id) ON DELETE RESTRICT,
    superseded_by TEXT NOT NULL REFERENCES jobs(run_id) ON DELETE RESTRICT,
    CHECK (run_id <> superseded_by)
);

CREATE INDEX IF NOT EXISTS supersession_target ON attempt_supersession(superseded_by);
"#;
