//! # Contract: durable local job catalog
//!
//! The catalog owns atomic lifecycle transitions, persistent fan-out events,
//! resource reservations, and worker leases. Platform process probing and IPC
//! remain outside this module so recovery decisions can be tested without
//! changing simulation behavior.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId};
use uuid::Uuid;

use crate::backend::{JobBackend, JobBackendError};
use crate::model::{
    CancelMode, EventQuery, JobEvent, JobEventKind, JobListQuery, JobProgress, JobReceipt,
    JobSnapshot, JobState, ResourceObservation, ResourceRequest, SubmitRequest,
    is_normal_absolute_path,
};
use crate::scheduler::{DispatchPlan, MAXIMUM_HEAD_BYPASS, QueuedAttempt, ResourceUsage};

mod storage;

use storage::*;

const CATALOG_SCHEMA_VERSION: i64 = 1;

/// Optional diagnostic attached to one atomic state transition.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransitionDiagnostic {
    /// Stable machine-readable code.
    pub code: Option<String>,
    /// Human-readable message without secrets.
    pub message: Option<String>,
}

/// Durable identity and heartbeat of one independently running worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerLease {
    /// Attempt controlled by this worker.
    pub run_id: RunId,
    /// Daemon instance currently responsible for observation.
    pub daemon_instance_id: String,
    /// Platform process identifier.
    pub worker_pid: u32,
    /// Process-start identity used to reject PID reuse.
    pub worker_start_token: String,
    /// Last durable worker heartbeat.
    pub heartbeat_at: Timestamp,
    /// Pending worker control request.
    pub cancel_mode: Option<CancelMode>,
}

impl WorkerLease {
    fn validate(&self) -> Result<(), JobBackendError> {
        if self.daemon_instance_id.trim().is_empty()
            || self.worker_pid == 0
            || self.worker_start_token.trim().is_empty()
        {
            return Err(JobBackendError::InvalidRequest(
                "worker lease identity is incomplete".into(),
            ));
        }
        Ok(())
    }
}

/// Single-instance daemon ownership record guarded by process-start identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DaemonLease {
    /// Unique identity generated for this daemon process.
    pub daemon_instance_id: String,
    /// Platform process identifier.
    pub daemon_pid: u32,
    /// Process-start identity used to reject PID reuse.
    pub daemon_start_token: String,
    /// Last durable daemon heartbeat.
    pub heartbeat_at: Timestamp,
}

impl DaemonLease {
    fn validate(&self) -> Result<(), JobBackendError> {
        if self.daemon_instance_id.trim().is_empty()
            || self.daemon_pid == 0
            || self.daemon_start_token.trim().is_empty()
        {
            return Err(JobBackendError::InvalidRequest(
                "daemon lease identity is incomplete".into(),
            ));
        }
        Ok(())
    }
}

/// Control value polled by a worker at a safe boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerControl {
    /// Continue normal execution.
    Continue,
    /// Finalize a legal cancellation at the next macro-step boundary.
    SafeCancel,
    /// Stop immediately and preserve forensic files.
    ForceStop,
}

/// Result of reconciling durable active rows after daemon restart.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Workers confirmed alive and assigned to the new daemon instance.
    pub reattached_run_ids: Vec<RunId>,
    /// Attempts whose already-persisted terminal result was reconciled into the catalog.
    pub reconciled_terminal_run_ids: Vec<RunId>,
    /// Attempts made terminal because no live worker could be confirmed.
    pub interrupted_run_ids: Vec<RunId>,
}

/// Host-side decision for one active attempt during daemon recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerRecoveryDisposition {
    /// The worker process identity is still live and its lease should move to the new daemon.
    Reattach,
    /// A validated terminal manifest already exists and the catalog should adopt its state.
    ReconcileTerminal(JobState),
    /// No live worker or validated terminal result exists; preserve artifacts as interrupted.
    Interrupt,
}

/// SQLite-backed implementation of the frozen local job-control contract.
pub struct LocalJobCatalog {
    connection: Connection,
}

impl LocalJobCatalog {
    /// Opens or creates a durable catalog at `path`.
    pub fn open(path: &Path) -> Result<Self, JobBackendError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(storage_error)?;
        }
        let connection = Connection::open(path).map_err(storage_error)?;
        Self::initialize(connection)
    }

    /// Opens an isolated in-memory catalog, primarily for control-plane tests.
    pub fn open_in_memory() -> Result<Self, JobBackendError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> Result<Self, JobBackendError> {
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(storage_error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")
            .map_err(storage_error)?;
        let version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(storage_error)?;
        if version == 0 {
            // Journal mode is persistent database state. Setting it on every
            // connection requires a lock that can block observers and workers
            // while the daemon is active; establish it exactly once when the
            // catalog is created.
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(storage_error)?;
            connection
                .execute_batch(CATALOG_SCHEMA)
                .map_err(storage_error)?;
            connection
                .pragma_update(None, "user_version", CATALOG_SCHEMA_VERSION)
                .map_err(storage_error)?;
        } else if version != CATALOG_SCHEMA_VERSION {
            return Err(JobBackendError::Storage(format!(
                "unsupported job catalog schema {version}"
            )));
        }
        Ok(Self { connection })
    }

    /// Claims single-daemon ownership using an explicit compare-and-swap value.
    ///
    /// A new catalog accepts an empty `replace_instance_id`. Reclaim by the same
    /// process identity is idempotent. Replacing another owner succeeds only
    /// when the caller supplies that exact observed instance identity after a
    /// platform liveness check.
    pub fn claim_daemon(
        &mut self,
        lease: &DaemonLease,
        replace_instance_id: Option<&str>,
    ) -> Result<(), JobBackendError> {
        lease.validate()?;
        let transaction = self.connection.transaction().map_err(storage_error)?;
        let current = transaction
            .query_row(
                "SELECT daemon_instance_id, daemon_pid, daemon_start_token,
                        heartbeat_seconds, heartbeat_nanosecond
                 FROM daemon_lease WHERE singleton = 1",
                [],
                stored_daemon_lease_from_row,
            )
            .optional()
            .map_err(storage_error)?
            .map(daemon_lease_from_stored)
            .transpose()?;
        match current {
            None => {
                transaction
                    .execute(
                        "INSERT INTO daemon_lease (
                            singleton, daemon_instance_id, daemon_pid, daemon_start_token,
                            heartbeat_seconds, heartbeat_nanosecond
                         ) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
                        params![
                            lease.daemon_instance_id,
                            i64::from(lease.daemon_pid),
                            lease.daemon_start_token,
                            lease.heartbeat_at.seconds_since_unix_epoch(),
                            i64::from(lease.heartbeat_at.nanosecond()),
                        ],
                    )
                    .map_err(storage_error)?;
            }
            Some(current)
                if current.daemon_instance_id == lease.daemon_instance_id
                    && current.daemon_pid == lease.daemon_pid
                    && current.daemon_start_token == lease.daemon_start_token =>
            {
                transaction
                    .execute(
                        "UPDATE daemon_lease
                         SET heartbeat_seconds = ?1, heartbeat_nanosecond = ?2
                         WHERE singleton = 1",
                        params![
                            lease.heartbeat_at.seconds_since_unix_epoch(),
                            i64::from(lease.heartbeat_at.nanosecond()),
                        ],
                    )
                    .map_err(storage_error)?;
            }
            Some(current) if replace_instance_id == Some(current.daemon_instance_id.as_str()) => {
                transaction
                    .execute(
                        "UPDATE daemon_lease SET
                            daemon_instance_id = ?1,
                            daemon_pid = ?2,
                            daemon_start_token = ?3,
                            heartbeat_seconds = ?4,
                            heartbeat_nanosecond = ?5
                         WHERE singleton = 1 AND daemon_instance_id = ?6",
                        params![
                            lease.daemon_instance_id,
                            i64::from(lease.daemon_pid),
                            lease.daemon_start_token,
                            lease.heartbeat_at.seconds_since_unix_epoch(),
                            i64::from(lease.heartbeat_at.nanosecond()),
                            current.daemon_instance_id,
                        ],
                    )
                    .map_err(storage_error)?;
            }
            Some(_) => {
                return Err(JobBackendError::Conflict(
                    "another local daemon owns the catalog".into(),
                ));
            }
        }
        transaction.commit().map_err(storage_error)
    }

    /// Reads the current daemon owner without changing its heartbeat.
    pub fn daemon_lease(&self) -> Result<Option<DaemonLease>, JobBackendError> {
        self.connection
            .query_row(
                "SELECT daemon_instance_id, daemon_pid, daemon_start_token,
                        heartbeat_seconds, heartbeat_nanosecond
                 FROM daemon_lease WHERE singleton = 1",
                [],
                stored_daemon_lease_from_row,
            )
            .optional()
            .map_err(storage_error)?
            .map(daemon_lease_from_stored)
            .transpose()
    }

    /// Refreshes ownership only when both daemon instance and start token match.
    pub fn heartbeat_daemon(
        &mut self,
        daemon_instance_id: &str,
        daemon_start_token: &str,
        at: Timestamp,
    ) -> Result<(), JobBackendError> {
        let changed = self
            .connection
            .execute(
                "UPDATE daemon_lease
                 SET heartbeat_seconds = ?1, heartbeat_nanosecond = ?2
                 WHERE singleton = 1 AND daemon_instance_id = ?3 AND daemon_start_token = ?4",
                params![
                    at.seconds_since_unix_epoch(),
                    i64::from(at.nanosecond()),
                    daemon_instance_id,
                    daemon_start_token,
                ],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Err(JobBackendError::Conflict(
                "daemon lease identity does not match".into(),
            ));
        }
        Ok(())
    }

    /// Releases ownership without affecting jobs or worker leases.
    pub fn release_daemon(
        &mut self,
        daemon_instance_id: &str,
        daemon_start_token: &str,
    ) -> Result<(), JobBackendError> {
        let changed = self
            .connection
            .execute(
                "DELETE FROM daemon_lease
                 WHERE singleton = 1 AND daemon_instance_id = ?1 AND daemon_start_token = ?2",
                params![daemon_instance_id, daemon_start_token],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Err(JobBackendError::Conflict(
                "daemon lease identity does not match".into(),
            ));
        }
        Ok(())
    }

    /// Atomically transitions one attempt and emits its persistent state event.
    pub fn transition(
        &mut self,
        run_id: &RunId,
        next: JobState,
        at: Timestamp,
        diagnostic: TransitionDiagnostic,
    ) -> Result<JobSnapshot, JobBackendError> {
        let transaction = self.connection.transaction().map_err(storage_error)?;
        transition_in_transaction(&transaction, run_id, next, at, &diagnostic)?;
        transaction.commit().map_err(storage_error)?;
        self.status_run(run_id)
    }

    /// Stores an absolute attempt output directory before worker execution.
    pub fn set_output_directory(
        &mut self,
        run_id: &RunId,
        output_directory: &Path,
    ) -> Result<(), JobBackendError> {
        if !is_normal_absolute_path(output_directory) {
            return Err(JobBackendError::InvalidRequest(
                "output directory must be a normalized absolute path".into(),
            ));
        }
        let path = output_directory.to_str().ok_or_else(|| {
            JobBackendError::InvalidRequest("output directory must be UTF-8".into())
        })?;
        let state = self.state_for_run(run_id)?;
        if !matches!(
            state,
            JobState::Starting | JobState::Running | JobState::Cancelling
        ) {
            return Err(JobBackendError::Conflict(
                "output directory can be assigned only to an active attempt".into(),
            ));
        }
        let changed = self
            .connection
            .execute(
                "UPDATE jobs SET output_directory = ?1 WHERE run_id = ?2",
                params![path, run_id.0],
            )
            .map_err(storage_error)?;
        if changed != 1 {
            return Err(JobBackendError::NotFound);
        }
        Ok(())
    }

    /// Attaches a newly spawned worker to a starting attempt.
    ///
    /// A safe cancellation may race with process creation after resources were
    /// reserved but before the daemon could persist the worker lease. In that
    /// case the attempt is already `cancelling`; the lease is still attached
    /// with the pending safe-cancel request so the worker can terminate at its
    /// first safe boundary instead of becoming an uncontrolled child.
    pub fn attach_worker(&mut self, lease: &WorkerLease) -> Result<(), JobBackendError> {
        lease.validate()?;
        if lease.cancel_mode.is_some() {
            return Err(JobBackendError::InvalidRequest(
                "a new worker lease cannot begin with cancellation requested".into(),
            ));
        }
        let transaction = self.connection.transaction().map_err(storage_error)?;
        let state = state_for_run_transaction(&transaction, &lease.run_id)?;
        if !matches!(state, JobState::Starting | JobState::Cancelling) {
            return Err(JobBackendError::Conflict(
                "worker may attach only while an attempt is starting or safely cancelling".into(),
            ));
        }
        let cancel_mode = (state == JobState::Cancelling).then_some("safe");
        transaction
            .execute(
                "INSERT INTO worker_leases (
                    run_id, daemon_instance_id, worker_pid, worker_start_token,
                    heartbeat_seconds, heartbeat_nanosecond, cancel_mode
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(run_id) DO UPDATE SET
                    daemon_instance_id = excluded.daemon_instance_id,
                    worker_pid = excluded.worker_pid,
                    worker_start_token = excluded.worker_start_token,
                    heartbeat_seconds = excluded.heartbeat_seconds,
                    heartbeat_nanosecond = excluded.heartbeat_nanosecond,
                    cancel_mode = excluded.cancel_mode",
                params![
                    lease.run_id.0,
                    lease.daemon_instance_id,
                    i64::from(lease.worker_pid),
                    lease.worker_start_token,
                    lease.heartbeat_at.seconds_since_unix_epoch(),
                    i64::from(lease.heartbeat_at.nanosecond()),
                    cancel_mode,
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    /// Marks an attached starting worker as running.
    pub fn mark_worker_running(
        &mut self,
        run_id: &RunId,
        worker_start_token: &str,
        at: Timestamp,
    ) -> Result<JobSnapshot, JobBackendError> {
        self.require_worker_token(run_id, worker_start_token)?;
        self.transition(
            run_id,
            JobState::Running,
            at,
            TransitionDiagnostic {
                code: Some("worker.running".into()),
                message: Some("worker entered resolved run preparation and execution".into()),
            },
        )
    }

    /// Persists one worker heartbeat and optional bounded task-level telemetry.
    pub fn heartbeat(
        &mut self,
        run_id: &RunId,
        worker_start_token: &str,
        at: Timestamp,
        progress: Option<JobProgress>,
        resource: Option<ResourceObservation>,
    ) -> Result<(), JobBackendError> {
        // Acquire the single WAL writer slot before reading the lease. A
        // deferred read followed by a write can otherwise lose a race with a
        // concurrent cancellation commit and fail with SQLITE_BUSY_SNAPSHOT.
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        require_worker_token_transaction(&transaction, run_id, worker_start_token)?;
        let stored = load_job_by_run_transaction(&transaction, run_id)?;
        if !matches!(
            stored.state,
            JobState::Starting | JobState::Running | JobState::Cancelling
        ) {
            return Err(JobBackendError::Conflict(
                "terminal or queued attempts cannot heartbeat".into(),
            ));
        }
        transaction
            .execute(
                "UPDATE worker_leases
                 SET heartbeat_seconds = ?1, heartbeat_nanosecond = ?2
                 WHERE run_id = ?3",
                params![
                    at.seconds_since_unix_epoch(),
                    i64::from(at.nanosecond()),
                    run_id.0
                ],
            )
            .map_err(storage_error)?;
        if let Some(progress) = progress {
            append_event_in_transaction(
                &transaction,
                &stored,
                at,
                JobEventKind::Progress,
                Some(&progress),
                None,
                None,
                None,
                None,
            )?;
        }
        if let Some(resource) = resource {
            append_event_in_transaction(
                &transaction,
                &stored,
                at,
                JobEventKind::Resource,
                None,
                Some(resource),
                None,
                None,
                None,
            )?;
        }
        transaction.commit().map_err(storage_error)
    }

    /// Emits one durable external-memory-pressure warning for each queued
    /// attempt that has not already received it.
    ///
    /// The daemon may call this on every pressure sampling tick. The catalog
    /// deduplicates by attempt and diagnostic code so an extended pressure
    /// interval cannot flood the persistent event log, while jobs submitted
    /// later during the same interval still receive an explicit warning.
    pub fn warn_queued_external_memory_pressure(
        &mut self,
        at: Timestamp,
    ) -> Result<Vec<RunId>, JobBackendError> {
        const CODE: &str = "daemon.external_memory_pressure";
        const MESSAGE: &str = "external memory pressure paused new job dispatch";

        let transaction = self.connection.transaction().map_err(storage_error)?;
        let jobs = {
            let mut statement = transaction
                .prepare(&format!(
                    "{JOB_SELECT}
                     WHERE jobs.state = 'queued'
                       AND NOT EXISTS (
                           SELECT 1 FROM events
                           WHERE events.run_id = jobs.run_id AND events.code = ?1
                       )
                     ORDER BY jobs.queue_order ASC"
                ))
                .map_err(storage_error)?;
            let rows = statement
                .query_map(params![CODE], stored_job_from_row)
                .map_err(storage_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)?
        };
        let mut warned = Vec::with_capacity(jobs.len());
        for job in jobs {
            append_event_in_transaction(
                &transaction,
                &job,
                at,
                JobEventKind::Warning,
                None,
                None,
                Some(CODE),
                Some(MESSAGE),
                None,
            )?;
            warned.push(job.run_id);
        }
        transaction.commit().map_err(storage_error)?;
        Ok(warned)
    }

    /// Reads the current worker control request without consuming it.
    pub fn worker_control(
        &self,
        run_id: &RunId,
        worker_start_token: &str,
    ) -> Result<WorkerControl, JobBackendError> {
        self.require_worker_token(run_id, worker_start_token)?;
        let mode = self
            .connection
            .query_row(
                "SELECT cancel_mode FROM worker_leases WHERE run_id = ?1",
                params![run_id.0],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_error)?;
        match mode.as_deref() {
            None => Ok(WorkerControl::Continue),
            Some("safe") => Ok(WorkerControl::SafeCancel),
            Some("force") => Ok(WorkerControl::ForceStop),
            Some(value) => Err(JobBackendError::Storage(format!(
                "invalid persisted cancel mode {value}"
            ))),
        }
    }

    /// Finalizes one attached worker and removes its active lease atomically.
    pub fn finish_worker(
        &mut self,
        run_id: &RunId,
        worker_start_token: &str,
        terminal: JobState,
        at: Timestamp,
        diagnostic: TransitionDiagnostic,
    ) -> Result<JobSnapshot, JobBackendError> {
        if !terminal.is_terminal() {
            return Err(JobBackendError::InvalidRequest(
                "worker completion requires a terminal state".into(),
            ));
        }
        let transaction = self.connection.transaction().map_err(storage_error)?;
        require_worker_token_transaction(&transaction, run_id, worker_start_token)?;
        transition_in_transaction(&transaction, run_id, terminal, at, &diagnostic)?;
        transaction
            .execute(
                "DELETE FROM worker_leases WHERE run_id = ?1",
                params![run_id.0],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        self.status_run(run_id)
    }

    /// Returns the current lease for an attempt when present.
    pub fn worker_lease(&self, run_id: &RunId) -> Result<Option<WorkerLease>, JobBackendError> {
        self.connection
            .query_row(
                "SELECT daemon_instance_id, worker_pid, worker_start_token,
                        heartbeat_seconds, heartbeat_nanosecond, cancel_mode
                 FROM worker_leases WHERE run_id = ?1",
                params![run_id.0],
                |row| stored_lease_from_row(row, run_id.clone()),
            )
            .optional()
            .map_err(storage_error)?
            .map(worker_lease_from_stored)
            .transpose()
    }

    /// Returns durable queued candidates in FIFO order for the pure scheduler.
    pub fn queued_attempts(&self) -> Result<Vec<QueuedAttempt>, JobBackendError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT run_id, resources_json, head_bypass_count
                 FROM jobs WHERE state = 'queued' ORDER BY queue_order ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(storage_error)?;
        let mut attempts = Vec::new();
        for row in rows {
            let (run_id, resources_json, bypass) = row.map_err(storage_error)?;
            let resources = serde_json::from_str::<ResourceRequest>(&resources_json)
                .map_err(corrupt_json_error)?;
            let head_bypass_count = u8::try_from(bypass).map_err(|_| {
                JobBackendError::Storage("invalid persisted head bypass count".into())
            })?;
            attempts.push(QueuedAttempt {
                run_id: RunId(run_id),
                resources,
                head_bypass_count,
            });
        }
        Ok(attempts)
    }

    /// Returns every active attempt and its optional worker lease for
    /// host-side process/manifest recovery inspection.
    pub fn recovery_candidates(
        &self,
    ) -> Result<Vec<(JobSnapshot, Option<WorkerLease>)>, JobBackendError> {
        self.active_rows_with_leases()?
            .into_iter()
            .map(|(run_id, lease)| self.status_run(&run_id).map(|snapshot| (snapshot, lease)))
            .collect()
    }

    /// Returns one exact attempt snapshot by run identity.
    pub fn snapshot_for_run(&self, run_id: &RunId) -> Result<JobSnapshot, JobBackendError> {
        self.status_run(run_id)
    }

    /// Sums resources reserved by starting, running, or cancelling attempts.
    pub fn active_resource_usage(&self) -> Result<ResourceUsage, JobBackendError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT resources_json FROM jobs
                 WHERE state IN ('starting', 'running', 'cancelling')",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut usage = ResourceUsage::default();
        for row in rows {
            let resources = serde_json::from_str::<ResourceRequest>(&row.map_err(storage_error)?)
                .map_err(corrupt_json_error)?;
            usage.cpu_slots = usage
                .cpu_slots
                .checked_add(resources.cpu_slots)
                .ok_or_else(|| JobBackendError::Storage("active CPU usage overflow".into()))?;
            usage.memory_mib = usage
                .memory_mib
                .checked_add(resources.memory_mib)
                .ok_or_else(|| JobBackendError::Storage("active memory usage overflow".into()))?;
        }
        Ok(usage)
    }

    /// Atomically persists bypass updates and reserves selected rows as starting.
    pub fn apply_dispatch_plan(
        &mut self,
        plan: &DispatchPlan,
        at: Timestamp,
    ) -> Result<Vec<JobSnapshot>, JobBackendError> {
        let selected = plan
            .selected_run_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if selected.len() != plan.selected_run_ids.len() {
            return Err(JobBackendError::InvalidRequest(
                "dispatch plan contains duplicate selected rows".into(),
            ));
        }
        let transaction = self.connection.transaction().map_err(storage_error)?;
        for update in &plan.bypass_updates {
            if update.head_bypass_count > MAXIMUM_HEAD_BYPASS {
                return Err(JobBackendError::InvalidRequest(
                    "dispatch plan exceeds the head bypass cap".into(),
                ));
            }
            let (state, current) = transaction
                .query_row(
                    "SELECT state, head_bypass_count FROM jobs WHERE run_id = ?1",
                    params![update.run_id.0],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(storage_error)?
                .ok_or(JobBackendError::NotFound)?;
            if parse_state(&state)? != JobState::Queued
                || i64::from(update.head_bypass_count) < current
            {
                return Err(JobBackendError::Conflict(
                    "dispatch bypass update is stale".into(),
                ));
            }
            transaction
                .execute(
                    "UPDATE jobs SET head_bypass_count = ?1 WHERE run_id = ?2",
                    params![i64::from(update.head_bypass_count), update.run_id.0],
                )
                .map_err(storage_error)?;
        }
        for run_id in &plan.selected_run_ids {
            transition_in_transaction(
                &transaction,
                run_id,
                JobState::Starting,
                at,
                &TransitionDiagnostic {
                    code: Some("scheduler.dispatched".into()),
                    message: Some("resources reserved and worker launch requested".into()),
                },
            )?;
        }
        transaction.commit().map_err(storage_error)?;
        plan.selected_run_ids
            .iter()
            .map(|run_id| self.status_run(run_id))
            .collect()
    }

    /// Reconciles active rows using a platform-supplied PID/start-token probe.
    ///
    /// Queued and terminal rows are never redispatched. An active row without a
    /// confirmed live lease becomes `interrupted`; a live lease is reassigned to
    /// `new_daemon_instance_id` and continues its existing attempt.
    pub fn recover_workers<F>(
        &mut self,
        new_daemon_instance_id: &str,
        at: Timestamp,
        mut is_alive: F,
    ) -> Result<RecoveryReport, JobBackendError>
    where
        F: FnMut(&WorkerLease) -> bool,
    {
        self.recover_workers_with(new_daemon_instance_id, at, |_, lease| {
            if lease.is_some_and(&mut is_alive) {
                WorkerRecoveryDisposition::Reattach
            } else {
                WorkerRecoveryDisposition::Interrupt
            }
        })
    }

    /// Reconciles active attempts using a host decision that may adopt an
    /// already-persisted terminal manifest instead of misclassifying it as an
    /// interrupted worker.
    pub fn recover_workers_with<F>(
        &mut self,
        new_daemon_instance_id: &str,
        at: Timestamp,
        mut disposition: F,
    ) -> Result<RecoveryReport, JobBackendError>
    where
        F: FnMut(&RunId, Option<&WorkerLease>) -> WorkerRecoveryDisposition,
    {
        if new_daemon_instance_id.trim().is_empty() {
            return Err(JobBackendError::InvalidRequest(
                "daemon instance identity is empty".into(),
            ));
        }
        let active = self.active_rows_with_leases()?;
        let mut report = RecoveryReport::default();
        for (run_id, lease) in active {
            match disposition(&run_id, lease.as_ref()) {
                WorkerRecoveryDisposition::Reattach => {
                    let Some(mut lease) = lease else {
                        return Err(JobBackendError::InvalidRequest(
                            "cannot reattach an attempt without a worker lease".into(),
                        ));
                    };
                    if lease.daemon_instance_id == new_daemon_instance_id {
                        continue;
                    }
                    lease.daemon_instance_id = new_daemon_instance_id.into();
                    let transaction = self.connection.transaction().map_err(storage_error)?;
                    transaction
                        .execute(
                            "UPDATE worker_leases
                             SET daemon_instance_id = ?1
                             WHERE run_id = ?2 AND worker_start_token = ?3",
                            params![
                                lease.daemon_instance_id,
                                lease.run_id.0,
                                lease.worker_start_token
                            ],
                        )
                        .map_err(storage_error)?;
                    let stored = load_job_by_run_transaction(&transaction, &run_id)?;
                    append_event_in_transaction(
                        &transaction,
                        &stored,
                        at,
                        JobEventKind::Warning,
                        None,
                        None,
                        Some("worker.reattached"),
                        Some("live worker lease reattached after daemon restart"),
                        None,
                    )?;
                    transaction.commit().map_err(storage_error)?;
                    report.reattached_run_ids.push(run_id);
                }
                WorkerRecoveryDisposition::ReconcileTerminal(terminal) => {
                    if !terminal.is_terminal() {
                        return Err(JobBackendError::InvalidRequest(
                            "manifest reconciliation requires a terminal state".into(),
                        ));
                    }
                    let transaction = self.connection.transaction().map_err(storage_error)?;
                    transition_in_transaction(
                        &transaction,
                        &run_id,
                        terminal,
                        at,
                        &TransitionDiagnostic {
                            code: Some("worker.terminal_reconciled".into()),
                            message: Some(
                                "catalog adopted a validated terminal manifest after worker exit"
                                    .into(),
                            ),
                        },
                    )?;
                    transaction
                        .execute(
                            "DELETE FROM worker_leases WHERE run_id = ?1",
                            params![run_id.0],
                        )
                        .map_err(storage_error)?;
                    transaction.commit().map_err(storage_error)?;
                    report.reconciled_terminal_run_ids.push(run_id);
                }
                WorkerRecoveryDisposition::Interrupt => {
                    let transaction = self.connection.transaction().map_err(storage_error)?;
                    transition_in_transaction(
                        &transaction,
                        &run_id,
                        JobState::Interrupted,
                        at,
                        &TransitionDiagnostic {
                            code: Some("worker.lost".into()),
                            message: Some(
                                "worker lease was absent or could not be confirmed alive".into(),
                            ),
                        },
                    )?;
                    transaction
                        .execute(
                            "DELETE FROM worker_leases WHERE run_id = ?1",
                            params![run_id.0],
                        )
                        .map_err(storage_error)?;
                    transaction.commit().map_err(storage_error)?;
                    report.interrupted_run_ids.push(run_id);
                }
            }
        }
        self.connection
            .execute(
                "DELETE FROM worker_leases
                 WHERE run_id IN (SELECT run_id FROM jobs WHERE state IN (
                    'complete', 'completed_with_particle_errors', 'failed', 'cancelled', 'interrupted'
                 ))",
                [],
            )
            .map_err(storage_error)?;
        Ok(report)
    }

    fn active_rows_with_leases(
        &self,
    ) -> Result<Vec<(RunId, Option<WorkerLease>)>, JobBackendError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT j.run_id,
                        l.daemon_instance_id, l.worker_pid, l.worker_start_token,
                        l.heartbeat_seconds, l.heartbeat_nanosecond, l.cancel_mode
                 FROM jobs j
                 LEFT JOIN worker_leases l ON l.run_id = j.run_id
                 WHERE j.state IN ('starting', 'running', 'cancelling')
                 ORDER BY j.queue_order ASC",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                let run_id = RunId(row.get::<_, String>(0)?);
                let daemon = row.get::<_, Option<String>>(1)?;
                let lease = match daemon {
                    Some(daemon_instance_id) => Some(StoredLease {
                        run_id: run_id.clone(),
                        daemon_instance_id,
                        worker_pid: row.get(2)?,
                        worker_start_token: row.get(3)?,
                        heartbeat_seconds: row.get(4)?,
                        heartbeat_nanosecond: row.get(5)?,
                        cancel_mode: row.get(6)?,
                    }),
                    None => None,
                };
                Ok((run_id, lease))
            })
            .map_err(storage_error)?;
        let mut active = Vec::new();
        for row in rows {
            let (run_id, lease) = row.map_err(storage_error)?;
            active.push((run_id, lease.map(worker_lease_from_stored).transpose()?));
        }
        Ok(active)
    }

    fn require_worker_token(
        &self,
        run_id: &RunId,
        worker_start_token: &str,
    ) -> Result<(), JobBackendError> {
        let matched = self
            .connection
            .query_row(
                "SELECT 1 FROM worker_leases
                 WHERE run_id = ?1 AND worker_start_token = ?2",
                params![run_id.0, worker_start_token],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_error)?;
        matched.ok_or_else(|| JobBackendError::Conflict("worker lease token does not match".into()))
    }

    fn state_for_run(&self, run_id: &RunId) -> Result<JobState, JobBackendError> {
        self.connection
            .query_row(
                "SELECT state FROM jobs WHERE run_id = ?1",
                params![run_id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(JobBackendError::NotFound)
            .and_then(|value| parse_state(&value))
    }

    fn status_run(&self, run_id: &RunId) -> Result<JobSnapshot, JobBackendError> {
        let stored = self
            .connection
            .query_row(
                &format!("{} WHERE run_id = ?1", JOB_SELECT),
                params![run_id.0],
                stored_job_from_row,
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(JobBackendError::NotFound)?;
        let positions = self.queue_positions()?;
        stored.into_snapshot(positions.get(run_id).copied())
    }

    fn queue_positions(&self) -> Result<BTreeMap<RunId, u64>, JobBackendError> {
        let mut statement = self
            .connection
            .prepare("SELECT run_id FROM jobs WHERE state = 'queued' ORDER BY queue_order ASC")
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut positions = BTreeMap::new();
        for (position, row) in rows.enumerate() {
            let position = u64::try_from(position)
                .map_err(|_| JobBackendError::Storage("queue position exceeded u64".into()))?;
            positions.insert(RunId(row.map_err(storage_error)?), position);
        }
        Ok(positions)
    }

    fn submit_at(
        &mut self,
        request: SubmitRequest,
        job_series_id: JobSeriesId,
        run_id: RunId,
        attempt: u32,
        at: Timestamp,
    ) -> Result<JobReceipt, JobBackendError> {
        request
            .validate()
            .map_err(|error| JobBackendError::InvalidRequest(error.code().into()))?;
        let input_json = serde_json::to_string(&request.input).map_err(storage_error)?;
        let resources_json = serde_json::to_string(&request.resources).map_err(storage_error)?;
        let transaction = self.connection.transaction().map_err(storage_error)?;
        transaction
            .execute(
                "INSERT INTO jobs (
                    job_series_id, run_id, attempt, state, input_json, resources_json,
                    created_seconds, created_nanosecond, head_bypass_count
                 ) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, 0)",
                params![
                    job_series_id.0,
                    run_id.0,
                    i64::from(attempt),
                    input_json,
                    resources_json,
                    at.seconds_since_unix_epoch(),
                    i64::from(at.nanosecond()),
                ],
            )
            .map_err(storage_error)?;
        let stored = load_job_by_run_transaction(&transaction, &run_id)?;
        append_event_in_transaction(
            &transaction,
            &stored,
            at,
            JobEventKind::StateTransition,
            None,
            None,
            Some("scheduler.queued"),
            Some("accepted into durable FIFO queue"),
            None,
        )?;
        transaction.commit().map_err(storage_error)?;
        let receipt = JobReceipt {
            job_series_id,
            run_id,
            attempt,
            state: JobState::Queued,
        };
        receipt
            .validate()
            .map_err(|error| JobBackendError::Storage(error.code().into()))?;
        Ok(receipt)
    }
}

impl JobBackend for LocalJobCatalog {
    fn submit(&mut self, request: SubmitRequest) -> Result<JobReceipt, JobBackendError> {
        let job_series_id = JobSeriesId(Uuid::now_v7().to_string());
        let run_id = RunId(Uuid::now_v7().to_string());
        self.submit_at(request, job_series_id, run_id, 1, system_timestamp()?)
    }

    fn list(&self, query: &JobListQuery) -> Result<Vec<JobSnapshot>, JobBackendError> {
        query
            .validate()
            .map_err(|error| JobBackendError::InvalidRequest(error.code().into()))?;
        let positions = self.queue_positions()?;
        let mut statement = self
            .connection
            .prepare(&format!("{} ORDER BY queue_order ASC", JOB_SELECT))
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], stored_job_from_row)
            .map_err(storage_error)?;
        let mut snapshots = Vec::new();
        for row in rows {
            let stored = row.map_err(storage_error)?;
            if !query.states.is_empty() && !query.states.contains(&stored.state) {
                continue;
            }
            let queue_position = positions.get(&stored.run_id).copied();
            snapshots.push(stored.into_snapshot(queue_position)?);
            if snapshots.len() == query.limit {
                break;
            }
        }
        Ok(snapshots)
    }

    fn status(&self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError> {
        let stored = self
            .connection
            .query_row(
                &format!(
                    "{} WHERE job_series_id = ?1 ORDER BY attempt DESC LIMIT 1",
                    JOB_SELECT
                ),
                params![job_series_id.0],
                stored_job_from_row,
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(JobBackendError::NotFound)?;
        let positions = self.queue_positions()?;
        let queue_position = positions.get(&stored.run_id).copied();
        stored.into_snapshot(queue_position)
    }

    fn cancel(
        &mut self,
        job_series_id: &JobSeriesId,
        mode: CancelMode,
    ) -> Result<JobSnapshot, JobBackendError> {
        let current = self.status(job_series_id)?;
        if current.state.is_terminal() {
            return Err(JobBackendError::Conflict(
                "terminal attempts cannot be cancelled or redispatched".into(),
            ));
        }
        let at = system_timestamp()?;
        match (current.state, mode) {
            (JobState::Queued, _) => self.transition(
                &current.run_id,
                JobState::Cancelled,
                at,
                TransitionDiagnostic {
                    code: Some("job.cancelled_before_start".into()),
                    message: Some("queued attempt cancelled without run artifacts".into()),
                },
            ),
            (JobState::Starting | JobState::Running, CancelMode::Safe) => {
                let transaction = self.connection.transaction().map_err(storage_error)?;
                transition_in_transaction(
                    &transaction,
                    &current.run_id,
                    JobState::Cancelling,
                    at,
                    &TransitionDiagnostic {
                        code: Some("job.safe_cancel_requested".into()),
                        message: Some("worker will cancel at a macro-step boundary".into()),
                    },
                )?;
                transaction
                    .execute(
                        "UPDATE worker_leases SET cancel_mode = 'safe' WHERE run_id = ?1",
                        params![current.run_id.0],
                    )
                    .map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
                self.status_run(&current.run_id)
            }
            (JobState::Cancelling, CancelMode::Safe) => Ok(current),
            (JobState::Starting | JobState::Running | JobState::Cancelling, CancelMode::Force) => {
                let transaction = self.connection.transaction().map_err(storage_error)?;
                transition_in_transaction(
                    &transaction,
                    &current.run_id,
                    JobState::Interrupted,
                    at,
                    &TransitionDiagnostic {
                        code: Some("job.force_stopped".into()),
                        message: Some(
                            "worker force-stop requested; forensic files preserved".into(),
                        ),
                    },
                )?;
                transaction
                    .execute(
                        "DELETE FROM worker_leases WHERE run_id = ?1",
                        params![current.run_id.0],
                    )
                    .map_err(storage_error)?;
                transaction.commit().map_err(storage_error)?;
                self.status_run(&current.run_id)
            }
            _ => Err(JobBackendError::Conflict(
                "cancellation is invalid for the current state".into(),
            )),
        }
    }

    fn events(&self, query: &EventQuery) -> Result<Vec<JobEvent>, JobBackendError> {
        query
            .validate()
            .map_err(|error| JobBackendError::InvalidRequest(error.code().into()))?;
        let after = query.after_sequence.unwrap_or(0);
        let stored = if let Some(job_series_id) = &query.job_series_id {
            let mut statement = self
                .connection
                .prepare(&format!(
                    "{} WHERE sequence > ?1 AND job_series_id = ?2
                     ORDER BY sequence ASC LIMIT ?3",
                    EVENT_SELECT
                ))
                .map_err(storage_error)?;
            collect_events(
                statement
                    .query_map(
                        params![after, job_series_id.0, query.limit as u64],
                        stored_event_from_row,
                    )
                    .map_err(storage_error)?,
            )?
        } else {
            let mut statement = self
                .connection
                .prepare(&format!(
                    "{} WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2",
                    EVENT_SELECT
                ))
                .map_err(storage_error)?;
            collect_events(
                statement
                    .query_map(params![after, query.limit as u64], stored_event_from_row)
                    .map_err(storage_error)?,
            )?
        };
        stored.into_iter().map(StoredEvent::into_event).collect()
    }
}

fn transition_in_transaction(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    next: JobState,
    at: Timestamp,
    diagnostic: &TransitionDiagnostic,
) -> Result<(), JobBackendError> {
    if matches!(next, JobState::Failed | JobState::Interrupted)
        && (diagnostic.code.as_deref().is_none_or(str::is_empty)
            || diagnostic.message.as_deref().is_none_or(str::is_empty))
    {
        return Err(JobBackendError::InvalidRequest(
            "failed and interrupted transitions require a diagnostic".into(),
        ));
    }
    let stored = load_job_by_run_transaction(transaction, run_id)?;
    if !stored.state.allows_transition_to(next) {
        return Err(JobBackendError::Conflict(format!(
            "illegal lifecycle transition {} -> {}",
            state_name(stored.state),
            state_name(next)
        )));
    }
    let started = (next == JobState::Running && stored.started_seconds.is_none())
        .then_some((at.seconds_since_unix_epoch(), i64::from(at.nanosecond())));
    let finished = next
        .is_terminal()
        .then_some((at.seconds_since_unix_epoch(), i64::from(at.nanosecond())));
    transaction
        .execute(
            "UPDATE jobs SET
                state = ?1,
                started_seconds = COALESCE(started_seconds, ?2),
                started_nanosecond = COALESCE(started_nanosecond, ?3),
                finished_seconds = ?4,
                finished_nanosecond = ?5
             WHERE run_id = ?6",
            params![
                state_name(next),
                started.map(|value| value.0),
                started.map(|value| value.1),
                finished.map(|value| value.0),
                finished.map(|value| value.1),
                run_id.0,
            ],
        )
        .map_err(storage_error)?;
    let mut updated = stored;
    updated.state = next;
    if let Some((seconds, nanos)) = started {
        updated.started_seconds = Some(seconds);
        updated.started_nanosecond = Some(nanos);
    }
    if let Some((seconds, nanos)) = finished {
        updated.finished_seconds = Some(seconds);
        updated.finished_nanosecond = Some(nanos);
    }
    append_event_in_transaction(
        transaction,
        &updated,
        at,
        JobEventKind::StateTransition,
        None,
        None,
        diagnostic.code.as_deref(),
        diagnostic.message.as_deref(),
        None,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_event_in_transaction(
    transaction: &Transaction<'_>,
    job: &StoredJob,
    at: Timestamp,
    kind: JobEventKind,
    progress: Option<&JobProgress>,
    resource: Option<ResourceObservation>,
    code: Option<&str>,
    message: Option<&str>,
    artifact_path: Option<&Path>,
) -> Result<(), JobBackendError> {
    if code.is_some_and(str::is_empty) || message.is_some_and(str::is_empty) {
        return Err(JobBackendError::InvalidRequest(
            "event diagnostics cannot be empty".into(),
        ));
    }
    let progress_json = progress
        .map(serde_json::to_string)
        .transpose()
        .map_err(storage_error)?;
    let resource_json = resource
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(storage_error)?;
    let artifact_path = artifact_path
        .map(|path| {
            if !is_normal_absolute_path(path) {
                return Err(JobBackendError::InvalidRequest(
                    "event artifact path must be normalized and absolute".into(),
                ));
            }
            path.to_str().map(str::to_owned).ok_or_else(|| {
                JobBackendError::InvalidRequest("artifact path must be UTF-8".into())
            })
        })
        .transpose()?;
    transaction
        .execute(
            "INSERT INTO events (
                emitted_seconds, emitted_nanosecond, job_series_id, run_id, attempt,
                kind, state, progress_json, resource_json, code, message, artifact_path
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                at.seconds_since_unix_epoch(),
                i64::from(at.nanosecond()),
                job.job_series_id.0,
                job.run_id.0,
                i64::from(job.attempt),
                event_kind_name(kind),
                state_name(job.state),
                progress_json,
                resource_json,
                code,
                message,
                artifact_path,
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn load_job_by_run_transaction(
    transaction: &Transaction<'_>,
    run_id: &RunId,
) -> Result<StoredJob, JobBackendError> {
    transaction
        .query_row(
            &format!("{} WHERE run_id = ?1", JOB_SELECT),
            params![run_id.0],
            stored_job_from_row,
        )
        .optional()
        .map_err(storage_error)?
        .ok_or(JobBackendError::NotFound)
}

fn state_for_run_transaction(
    transaction: &Transaction<'_>,
    run_id: &RunId,
) -> Result<JobState, JobBackendError> {
    transaction
        .query_row(
            "SELECT state FROM jobs WHERE run_id = ?1",
            params![run_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_error)?
        .ok_or(JobBackendError::NotFound)
        .and_then(|value| parse_state(&value))
}

fn require_worker_token_transaction(
    transaction: &Transaction<'_>,
    run_id: &RunId,
    worker_start_token: &str,
) -> Result<(), JobBackendError> {
    transaction
        .query_row(
            "SELECT 1 FROM worker_leases
             WHERE run_id = ?1 AND worker_start_token = ?2",
            params![run_id.0, worker_start_token],
            |_| Ok(()),
        )
        .optional()
        .map_err(storage_error)?
        .ok_or_else(|| JobBackendError::Conflict("worker lease token does not match".into()))
}

pub(crate) fn system_timestamp() -> Result<Timestamp, JobBackendError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| JobBackendError::Storage(error.to_string()))?;
    let seconds = i64::try_from(duration.as_secs())
        .map_err(|error| JobBackendError::Storage(error.to_string()))?;
    Timestamp::new(seconds, duration.subsec_nanos())
        .map_err(|error| JobBackendError::Storage(error.to_string()))
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Starting => "starting",
        JobState::Running => "running",
        JobState::Cancelling => "cancelling",
        JobState::Complete => "complete",
        JobState::CompletedWithParticleErrors => "completed_with_particle_errors",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::Interrupted => "interrupted",
    }
}

fn parse_state(value: &str) -> Result<JobState, JobBackendError> {
    match value {
        "queued" => Ok(JobState::Queued),
        "starting" => Ok(JobState::Starting),
        "running" => Ok(JobState::Running),
        "cancelling" => Ok(JobState::Cancelling),
        "complete" => Ok(JobState::Complete),
        "completed_with_particle_errors" => Ok(JobState::CompletedWithParticleErrors),
        "failed" => Ok(JobState::Failed),
        "cancelled" => Ok(JobState::Cancelled),
        "interrupted" => Ok(JobState::Interrupted),
        _ => Err(JobBackendError::Storage(format!(
            "invalid persisted job state {value}"
        ))),
    }
}

fn parse_state_sql(column: usize, value: &str) -> rusqlite::Result<JobState> {
    parse_state(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn event_kind_name(kind: JobEventKind) -> &'static str {
    match kind {
        JobEventKind::StateTransition => "state_transition",
        JobEventKind::Progress => "progress",
        JobEventKind::Resource => "resource",
        JobEventKind::Warning => "warning",
        JobEventKind::Error => "error",
        JobEventKind::Artifact => "artifact",
    }
}

fn parse_event_kind_sql(column: usize, value: &str) -> rusqlite::Result<JobEventKind> {
    let kind = match value {
        "state_transition" => JobEventKind::StateTransition,
        "progress" => JobEventKind::Progress,
        "resource" => JobEventKind::Resource,
        "warning" => JobEventKind::Warning,
        "error" => JobEventKind::Error,
        "artifact" => JobEventKind::Artifact,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(JobBackendError::Storage(format!(
                    "invalid persisted event kind {value}"
                ))),
            ));
        }
    };
    Ok(kind)
}

fn storage_error(error: impl std::fmt::Display) -> JobBackendError {
    JobBackendError::Storage(error.to_string())
}

fn corrupt_json_error(error: impl std::fmt::Display) -> JobBackendError {
    JobBackendError::Storage(format!("corrupt job catalog JSON: {error}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use tempfile::TempDir;

    use super::*;
    use crate::model::RunInput;
    use crate::scheduler::{ResourceUsage, SchedulerCapacity, plan_dispatch};

    fn series(value: u8) -> JobSeriesId {
        JobSeriesId(format!("018f0000-0000-7000-8000-{value:012x}"))
    }

    fn run(value: u8) -> RunId {
        RunId(format!("018f0000-0000-7000-8000-{value:012x}"))
    }

    fn request(root: &Path, cpu_slots: u32, memory_mib: u64) -> SubmitRequest {
        SubmitRequest {
            input: RunInput::Project {
                project_root: root.to_path_buf(),
                profile_name: "default".into(),
            },
            resources: ResourceRequest {
                cpu_slots,
                memory_mib,
                worker_threads: cpu_slots,
            },
        }
    }

    fn list_all(catalog: &LocalJobCatalog) -> Vec<JobSnapshot> {
        catalog
            .list(&JobListQuery {
                states: Vec::new(),
                limit: 100,
            })
            .unwrap()
    }

    fn event_query() -> EventQuery {
        EventQuery {
            job_series_id: None,
            after_sequence: None,
            limit: 100,
        }
    }

    #[test]
    fn durable_queue_and_fanout_events_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let database = temp.path().join("jobs.sqlite");
        let root = temp.path().canonicalize().unwrap();
        let receipt = {
            let mut catalog = LocalJobCatalog::open(&database).unwrap();
            catalog
                .submit_at(
                    request(&root, 2, 1_024),
                    series(1),
                    run(1),
                    1,
                    Timestamp::UNIX_EPOCH,
                )
                .unwrap()
        };
        let catalog = LocalJobCatalog::open(&database).unwrap();
        let snapshot = catalog.status(&receipt.job_series_id).unwrap();
        assert_eq!(snapshot.state, JobState::Queued);
        assert_eq!(snapshot.queue_position, Some(0));
        let first = catalog.events(&event_query()).unwrap();
        let second = catalog.events(&event_query()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].sequence, 1);
    }

    #[test]
    fn external_memory_pressure_warns_each_queued_attempt_once() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 1, 256),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();

        assert_eq!(
            catalog
                .warn_queued_external_memory_pressure(Timestamp::new(1, 0).unwrap())
                .unwrap(),
            vec![run(1)]
        );
        assert!(
            catalog
                .warn_queued_external_memory_pressure(Timestamp::new(2, 0).unwrap())
                .unwrap()
                .is_empty()
        );

        catalog
            .submit_at(
                request(&root, 1, 256),
                series(2),
                run(2),
                1,
                Timestamp::new(3, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(
            catalog
                .warn_queued_external_memory_pressure(Timestamp::new(4, 0).unwrap())
                .unwrap(),
            vec![run(2)]
        );

        let warnings = catalog
            .events(&event_query())
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == JobEventKind::Warning)
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].run_id, run(1));
        assert_eq!(warnings[1].run_id, run(2));
        assert!(warnings.iter().all(|event| {
            event.code.as_deref() == Some("daemon.external_memory_pressure")
                && event.state == JobState::Queued
        }));
    }

    #[test]
    fn dispatch_transition_is_atomic_and_terminal_rows_never_leave_terminal() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 2, 1_024),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        let plan = plan_dispatch(
            SchedulerCapacity {
                cpu_slots: 4,
                memory_mib: 4_096,
            },
            ResourceUsage::default(),
            false,
            &catalog.queued_attempts().unwrap(),
        )
        .unwrap();
        let started = catalog
            .apply_dispatch_plan(&plan, Timestamp::new(1, 0).unwrap())
            .unwrap();
        assert_eq!(started[0].state, JobState::Starting);
        catalog
            .transition(
                &run(1),
                JobState::Failed,
                Timestamp::new(2, 0).unwrap(),
                TransitionDiagnostic {
                    code: Some("worker.start_failed".into()),
                    message: Some("worker did not start".into()),
                },
            )
            .unwrap();
        assert!(catalog.queued_attempts().unwrap().is_empty());
        assert!(
            catalog
                .transition(
                    &run(1),
                    JobState::Starting,
                    Timestamp::new(3, 0).unwrap(),
                    TransitionDiagnostic::default(),
                )
                .is_err()
        );
        assert_eq!(catalog.status(&series(1)).unwrap().state, JobState::Failed);
    }

    #[test]
    fn safe_backfill_counts_and_selected_rows_commit_together() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        for value in 1..=3 {
            let (cpu, memory) = if value == 1 { (4, 4_096) } else { (1, 256) };
            catalog
                .submit_at(
                    request(&root, cpu, memory),
                    series(value),
                    run(value),
                    1,
                    Timestamp::new(i64::from(value), 0).unwrap(),
                )
                .unwrap();
        }
        let plan = plan_dispatch(
            SchedulerCapacity {
                cpu_slots: 4,
                memory_mib: 4_096,
            },
            ResourceUsage {
                cpu_slots: 2,
                memory_mib: 0,
            },
            false,
            &catalog.queued_attempts().unwrap(),
        )
        .unwrap();
        assert_eq!(plan.selected_run_ids, vec![run(2), run(3)]);
        catalog
            .apply_dispatch_plan(&plan, Timestamp::new(10, 0).unwrap())
            .unwrap();
        let queued = catalog.queued_attempts().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].run_id, run(1));
        assert_eq!(queued[0].head_bypass_count, 2);
        assert_eq!(
            catalog.status(&series(2)).unwrap().state,
            JobState::Starting
        );
    }

    #[test]
    fn worker_token_safe_cancel_and_terminalization_are_durable() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 2, 1_024),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        let plan = plan_dispatch(
            SchedulerCapacity {
                cpu_slots: 2,
                memory_mib: 1_024,
            },
            ResourceUsage::default(),
            false,
            &catalog.queued_attempts().unwrap(),
        )
        .unwrap();
        catalog
            .apply_dispatch_plan(&plan, Timestamp::new(1, 0).unwrap())
            .unwrap();
        catalog
            .attach_worker(&WorkerLease {
                run_id: run(1),
                daemon_instance_id: "daemon-a".into(),
                worker_pid: 42,
                worker_start_token: "pid-42-start-a".into(),
                heartbeat_at: Timestamp::new(1, 0).unwrap(),
                cancel_mode: None,
            })
            .unwrap();
        assert!(
            catalog
                .mark_worker_running(&run(1), "wrong-token", Timestamp::new(2, 0).unwrap())
                .is_err()
        );
        catalog
            .mark_worker_running(&run(1), "pid-42-start-a", Timestamp::new(2, 0).unwrap())
            .unwrap();
        let cancelled = catalog.cancel(&series(1), CancelMode::Safe).unwrap();
        assert_eq!(cancelled.state, JobState::Cancelling);
        assert_eq!(
            catalog.worker_control(&run(1), "pid-42-start-a").unwrap(),
            WorkerControl::SafeCancel
        );
        let finished = catalog
            .finish_worker(
                &run(1),
                "pid-42-start-a",
                JobState::Cancelled,
                Timestamp::new(3, 0).unwrap(),
                TransitionDiagnostic {
                    code: Some("job.cancelled".into()),
                    message: Some("safe macro-step cancellation finalized".into()),
                },
            )
            .unwrap();
        assert_eq!(finished.state, JobState::Cancelled);
        assert!(catalog.worker_lease(&run(1)).unwrap().is_none());
    }

    #[test]
    fn safe_cancel_between_spawn_and_lease_attach_reaches_worker() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 1, 256),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        catalog
            .apply_dispatch_plan(
                &DispatchPlan {
                    selected_run_ids: vec![run(1)],
                    bypass_updates: Vec::new(),
                    stopped_by: None,
                },
                Timestamp::new(1, 0).unwrap(),
            )
            .unwrap();
        assert_eq!(
            catalog.cancel(&series(1), CancelMode::Safe).unwrap().state,
            JobState::Cancelling
        );
        catalog
            .attach_worker(&WorkerLease {
                run_id: run(1),
                daemon_instance_id: "daemon-a".into(),
                worker_pid: 42,
                worker_start_token: "pid-42-start-a".into(),
                heartbeat_at: Timestamp::new(2, 0).unwrap(),
                cancel_mode: None,
            })
            .unwrap();
        assert_eq!(
            catalog.worker_control(&run(1), "pid-42-start-a").unwrap(),
            WorkerControl::SafeCancel
        );
        assert_eq!(
            catalog
                .finish_worker(
                    &run(1),
                    "pid-42-start-a",
                    JobState::Cancelled,
                    Timestamp::new(3, 0).unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap()
                .state,
            JobState::Cancelled
        );
    }

    #[test]
    fn restart_reattaches_live_worker_and_interrupts_missing_worker_without_retry() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        for value in 1..=3 {
            catalog
                .submit_at(
                    request(&root, 1, 256),
                    series(value),
                    run(value),
                    1,
                    Timestamp::UNIX_EPOCH,
                )
                .unwrap();
        }
        let plan = DispatchPlan {
            selected_run_ids: vec![run(1), run(2)],
            bypass_updates: Vec::new(),
            stopped_by: None,
        };
        catalog
            .apply_dispatch_plan(&plan, Timestamp::new(1, 0).unwrap())
            .unwrap();
        catalog
            .attach_worker(&WorkerLease {
                run_id: run(1),
                daemon_instance_id: "old-daemon".into(),
                worker_pid: 41,
                worker_start_token: "alive-token".into(),
                heartbeat_at: Timestamp::new(2, 0).unwrap(),
                cancel_mode: None,
            })
            .unwrap();
        let report = catalog
            .recover_workers("new-daemon", Timestamp::new(3, 0).unwrap(), |lease| {
                lease.worker_start_token == "alive-token"
            })
            .unwrap();
        assert_eq!(report.reattached_run_ids, vec![run(1)]);
        assert!(report.reconciled_terminal_run_ids.is_empty());
        assert_eq!(report.interrupted_run_ids, vec![run(2)]);
        assert_eq!(
            catalog
                .worker_lease(&run(1))
                .unwrap()
                .unwrap()
                .daemon_instance_id,
            "new-daemon"
        );
        assert_eq!(
            catalog.status(&series(2)).unwrap().state,
            JobState::Interrupted
        );
        assert_eq!(catalog.status(&series(3)).unwrap().state, JobState::Queued);
        assert_eq!(catalog.queued_attempts().unwrap().len(), 1);
    }

    #[test]
    fn recovery_adopts_a_validated_terminal_result_instead_of_interrupting_it() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 1, 256),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        catalog
            .apply_dispatch_plan(
                &DispatchPlan {
                    selected_run_ids: vec![run(1)],
                    bypass_updates: Vec::new(),
                    stopped_by: None,
                },
                Timestamp::new(1, 0).unwrap(),
            )
            .unwrap();
        catalog
            .attach_worker(&WorkerLease {
                run_id: run(1),
                daemon_instance_id: "old-daemon".into(),
                worker_pid: 42,
                worker_start_token: "finished-token".into(),
                heartbeat_at: Timestamp::new(1, 0).unwrap(),
                cancel_mode: None,
            })
            .unwrap();
        catalog
            .mark_worker_running(&run(1), "finished-token", Timestamp::new(1, 0).unwrap())
            .unwrap();
        let candidates = catalog.recovery_candidates().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0.run_id, run(1));

        let report = catalog
            .recover_workers_with("new-daemon", Timestamp::new(2, 0).unwrap(), |run_id, _| {
                assert_eq!(run_id, &run(1));
                WorkerRecoveryDisposition::ReconcileTerminal(JobState::Complete)
            })
            .unwrap();
        assert_eq!(report.reconciled_terminal_run_ids, vec![run(1)]);
        assert!(report.interrupted_run_ids.is_empty());
        assert_eq!(
            catalog.status(&series(1)).unwrap().state,
            JobState::Complete
        );
        assert!(catalog.worker_lease(&run(1)).unwrap().is_none());
    }

    #[test]
    fn repeated_same_daemon_recovery_does_not_emit_reattach_events() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 1, 256),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        catalog
            .apply_dispatch_plan(
                &DispatchPlan {
                    selected_run_ids: vec![run(1)],
                    bypass_updates: Vec::new(),
                    stopped_by: None,
                },
                Timestamp::new(1, 0).unwrap(),
            )
            .unwrap();
        catalog
            .attach_worker(&WorkerLease {
                run_id: run(1),
                daemon_instance_id: "daemon-a".into(),
                worker_pid: 42,
                worker_start_token: "alive-token".into(),
                heartbeat_at: Timestamp::new(1, 0).unwrap(),
                cancel_mode: None,
            })
            .unwrap();
        let before = catalog.events(&event_query()).unwrap();
        let report = catalog
            .recover_workers("daemon-a", Timestamp::new(2, 0).unwrap(), |_| true)
            .unwrap();
        assert!(report.reattached_run_ids.is_empty());
        assert!(report.reconciled_terminal_run_ids.is_empty());
        assert!(report.interrupted_run_ids.is_empty());
        assert_eq!(catalog.events(&event_query()).unwrap(), before);
    }

    #[test]
    fn queued_cancel_is_catalog_only_and_has_no_artifact_path() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        catalog
            .submit_at(
                request(&root, 1, 256),
                series(1),
                run(1),
                1,
                Timestamp::UNIX_EPOCH,
            )
            .unwrap();
        let snapshot = catalog.cancel(&series(1), CancelMode::Safe).unwrap();
        assert_eq!(snapshot.state, JobState::Cancelled);
        assert_eq!(snapshot.started_at, None);
        assert_eq!(snapshot.output_directory, None);
        assert!(catalog.worker_lease(&run(1)).unwrap().is_none());
        assert!(catalog.queued_attempts().unwrap().is_empty());
        assert_eq!(list_all(&catalog).len(), 1);
    }

    #[test]
    fn daemon_ownership_uses_compare_and_swap_and_rejects_stale_release() {
        let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
        let first = DaemonLease {
            daemon_instance_id: "daemon-a".into(),
            daemon_pid: 101,
            daemon_start_token: "start-a".into(),
            heartbeat_at: Timestamp::UNIX_EPOCH,
        };
        catalog.claim_daemon(&first, None).unwrap();
        catalog
            .claim_daemon(
                &DaemonLease {
                    heartbeat_at: Timestamp::new(1, 0).unwrap(),
                    ..first.clone()
                },
                None,
            )
            .unwrap();

        let second = DaemonLease {
            daemon_instance_id: "daemon-b".into(),
            daemon_pid: 202,
            daemon_start_token: "start-b".into(),
            heartbeat_at: Timestamp::new(2, 0).unwrap(),
        };
        assert!(catalog.claim_daemon(&second, None).is_err());
        catalog.claim_daemon(&second, Some("daemon-a")).unwrap();
        assert_eq!(catalog.daemon_lease().unwrap(), Some(second.clone()));
        assert!(catalog.release_daemon("daemon-a", "start-a").is_err());
        assert_eq!(catalog.daemon_lease().unwrap(), Some(second.clone()));
        catalog
            .heartbeat_daemon("daemon-b", "start-b", Timestamp::new(3, 0).unwrap())
            .unwrap();
        catalog.release_daemon("daemon-b", "start-b").unwrap();
        assert_eq!(catalog.daemon_lease().unwrap(), None);
    }
}
