//! # Contract: daemon scheduling and worker process boundary
//!
//! This module combines the durable catalog with the pure scheduler. Process
//! creation, liveness, and force termination remain explicit platform traits;
//! no scientific document or numerical rule is duplicated here.

use std::path::Path;
use std::time::{Duration, Instant};

use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId};
use trajecta_core::runner::{RunnerControl, RunnerControlDecision, RunnerProgress};

use crate::backend::{JobBackend, JobBackendError};
use crate::catalog::{
    LocalJobCatalog, TransitionDiagnostic, WorkerControl, WorkerLease, system_timestamp,
};
use crate::history::{JobAttemptHistory, JobHistoryBackend, PrunePlan};
use crate::model::{
    CancelMode, EventQuery, JobEvent, JobListQuery, JobProgress, JobReceipt, JobSnapshot, JobState,
    ResourceObservation, SubmitRequest,
};
use crate::scheduler::{DispatchPlan, SchedulerCapacity, SchedulerError, plan_dispatch};

/// Identity returned after the daemon starts an independent worker process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnedWorker {
    /// Platform process identifier.
    pub worker_pid: u32,
    /// Process-start token used to reject PID reuse during recovery.
    pub worker_start_token: String,
}

/// Platform process launch and emergency termination operations.
pub trait WorkerProcessManager {
    /// Starts the same-release binary in hidden worker mode.
    fn launch(&mut self, snapshot: &JobSnapshot) -> Result<SpawnedWorker, String>;

    /// Immediately terminates a process whose PID and start token both match.
    fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String>;
}

/// Platform force-stop operation used by the IPC control backend.
pub trait WorkerTerminator {
    /// Immediately terminates a process whose PID and start token both match.
    fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String>;

    /// Finalizes or audits the attempt artifacts after the process is stopped
    /// and returns the terminal state that the catalog must persist.
    ///
    /// Platform-only implementations may keep the default `interrupted`
    /// result. Product integrations can atomically mark a valid running
    /// manifest interrupted, or return an already-persisted terminal manifest
    /// state when the worker exited in the narrow manifest/catalog gap.
    fn terminal_state_after_force_stop(
        &mut self,
        _snapshot: &JobSnapshot,
        _at: Timestamp,
    ) -> Result<JobState, String> {
        Ok(JobState::Interrupted)
    }
}

/// Outcome of one non-blocking scheduling pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DispatchCycleReport {
    /// Pure scheduling decision applied to the catalog.
    pub plan: DispatchPlan,
    /// Workers successfully launched and leased.
    pub launched_run_ids: Vec<RunId>,
    /// Rows terminalized after a controlled launch failure.
    pub failed_run_ids: Vec<RunId>,
    /// Rows marked interrupted because a spawned process could not be controlled.
    pub interrupted_run_ids: Vec<RunId>,
}

/// Runs one resource-accounted FIFO/safe-backfill dispatch cycle.
pub fn dispatch_once(
    catalog: &mut LocalJobCatalog,
    capacity: SchedulerCapacity,
    daemon_instance_id: &str,
    external_memory_pressure: bool,
    at: Timestamp,
    processes: &mut dyn WorkerProcessManager,
) -> Result<DispatchCycleReport, JobBackendError> {
    if daemon_instance_id.trim().is_empty() {
        return Err(JobBackendError::InvalidRequest(
            "daemon instance identity is empty".into(),
        ));
    }
    let queue = catalog.queued_attempts()?;
    let active = catalog.active_resource_usage()?;
    let plan = plan_dispatch(capacity, active, external_memory_pressure, &queue)
        .map_err(scheduler_error)?;
    let selected = catalog.apply_dispatch_plan(&plan, at)?;
    let mut report = DispatchCycleReport {
        plan,
        ..DispatchCycleReport::default()
    };
    for snapshot in selected {
        match processes.launch(&snapshot) {
            Ok(spawned) => {
                let lease = WorkerLease {
                    run_id: snapshot.run_id.clone(),
                    daemon_instance_id: daemon_instance_id.into(),
                    worker_pid: spawned.worker_pid,
                    worker_start_token: spawned.worker_start_token,
                    heartbeat_at: at,
                    cancel_mode: None,
                };
                if let Err(attach_error) = catalog.attach_worker(&lease) {
                    let stop = processes.force_stop(&lease);
                    let current = catalog.snapshot_for_run(&snapshot.run_id)?;
                    if current.state.is_terminal() {
                        if let Err(stop_error) = stop {
                            return Err(JobBackendError::Unavailable(format!(
                                "worker became terminal before lease attachment, but the spawned process could not be stopped: {attach_error}; {}",
                                stop_error
                            )));
                        }
                        report.interrupted_run_ids.push(snapshot.run_id);
                        continue;
                    }
                    let (terminal, code, message) = match stop {
                        Ok(()) => (
                            JobState::Failed,
                            "worker.lease_attach_failed",
                            format!(
                                "worker lease attach failed and spawned process was stopped: {attach_error}"
                            ),
                        ),
                        Err(stop_error) => (
                            JobState::Interrupted,
                            "worker.uncontrolled_after_attach_failure",
                            format!(
                                "worker lease attach failed: {attach_error}; force stop also failed: {stop_error}"
                            ),
                        ),
                    };
                    catalog.transition(
                        &snapshot.run_id,
                        terminal,
                        at,
                        TransitionDiagnostic {
                            code: Some(code.into()),
                            message: Some(message),
                        },
                    )?;
                    if terminal == JobState::Failed {
                        report.failed_run_ids.push(snapshot.run_id);
                    } else {
                        report.interrupted_run_ids.push(snapshot.run_id);
                    }
                } else {
                    report.launched_run_ids.push(snapshot.run_id);
                }
            }
            Err(error) => {
                catalog.transition(
                    &snapshot.run_id,
                    JobState::Failed,
                    at,
                    TransitionDiagnostic {
                        code: Some("worker.launch_failed".into()),
                        message: Some(error),
                    },
                )?;
                report.failed_run_ids.push(snapshot.run_id);
            }
        }
    }
    Ok(report)
}

/// Capacity-validating IPC backend with ordered force-stop semantics.
pub struct DaemonControlBackend<T> {
    catalog: LocalJobCatalog,
    capacity: SchedulerCapacity,
    terminator: T,
}

impl<T> DaemonControlBackend<T> {
    /// Creates a daemon control backend from an owned catalog connection.
    pub fn new(
        catalog: LocalJobCatalog,
        capacity: SchedulerCapacity,
        terminator: T,
    ) -> Result<Self, JobBackendError> {
        capacity.validate().map_err(scheduler_error)?;
        Ok(Self {
            catalog,
            capacity,
            terminator,
        })
    }

    /// Returns the owned catalog for daemon-local scheduling operations.
    #[must_use]
    pub const fn catalog(&self) -> &LocalJobCatalog {
        &self.catalog
    }

    /// Returns mutable catalog access for daemon-local scheduling operations.
    pub fn catalog_mut(&mut self) -> &mut LocalJobCatalog {
        &mut self.catalog
    }

    /// Consumes the wrapper and returns its catalog and platform terminator.
    pub fn into_parts(self) -> (LocalJobCatalog, T) {
        (self.catalog, self.terminator)
    }
}

impl<T: WorkerTerminator> JobBackend for DaemonControlBackend<T> {
    fn submit(&mut self, request: SubmitRequest) -> Result<JobReceipt, JobBackendError> {
        request
            .validate()
            .map_err(|error| JobBackendError::InvalidRequest(error.code().into()))?;
        if request.resources.cpu_slots > self.capacity.cpu_slots
            || request.resources.memory_mib > self.capacity.memory_mib
        {
            return Err(JobBackendError::InvalidRequest(
                "job resource request exceeds local daemon capacity".into(),
            ));
        }
        self.catalog.submit(request)
    }

    fn list(&self, query: &JobListQuery) -> Result<Vec<JobSnapshot>, JobBackendError> {
        self.catalog.list(query)
    }

    fn status(&self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError> {
        self.catalog.status(job_series_id)
    }

    fn cancel(
        &mut self,
        job_series_id: &JobSeriesId,
        mode: CancelMode,
    ) -> Result<JobSnapshot, JobBackendError> {
        if mode == CancelMode::Force {
            let current = self.catalog.status(job_series_id)?;
            if matches!(
                current.state,
                JobState::Starting | JobState::Running | JobState::Cancelling
            ) && let Some(lease) = self.catalog.worker_lease(&current.run_id)?
            {
                self.terminator
                    .force_stop(&lease)
                    .map_err(JobBackendError::Unavailable)?;
                let at = system_timestamp()?;
                let terminal = self
                    .terminator
                    .terminal_state_after_force_stop(&current, at)
                    .map_err(JobBackendError::Unavailable)?;
                if !terminal.is_terminal() {
                    return Err(JobBackendError::InvalidRequest(
                        "force-stop artifact finalization returned a non-terminal state".into(),
                    ));
                }
                let (code, message) = if terminal == JobState::Interrupted {
                    (
                        "job.force_stopped",
                        "worker was force-stopped; forensic files were preserved",
                    )
                } else {
                    (
                        "worker.terminal_reconciled",
                        "catalog adopted a validated terminal manifest after force-stop",
                    )
                };
                return self.catalog.finish_worker(
                    &current.run_id,
                    &lease.worker_start_token,
                    terminal,
                    at,
                    TransitionDiagnostic {
                        code: Some(code.into()),
                        message: Some(message.into()),
                    },
                );
            }
        }
        self.catalog.cancel(job_series_id, mode)
    }

    fn events(&self, query: &EventQuery) -> Result<Vec<JobEvent>, JobBackendError> {
        self.catalog.events(query)
    }
}

impl<T: WorkerTerminator> JobHistoryBackend for DaemonControlBackend<T> {
    fn rerun(&mut self, job_series_id: &JobSeriesId) -> Result<JobReceipt, JobBackendError> {
        let current = self.catalog.status(job_series_id)?;
        if current.resources.cpu_slots > self.capacity.cpu_slots
            || current.resources.memory_mib > self.capacity.memory_mib
        {
            return Err(JobBackendError::InvalidRequest(
                "rerun resource request exceeds current daemon capacity".into(),
            ));
        }
        self.catalog.rerun(job_series_id)
    }

    fn forget(&mut self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError> {
        self.catalog.forget(job_series_id)
    }

    fn attempt_history(
        &self,
        job_series_id: &JobSeriesId,
    ) -> Result<Vec<JobAttemptHistory>, JobBackendError> {
        self.catalog.attempt_history(job_series_id)
    }

    fn record_full_verification(
        &mut self,
        run_id: &RunId,
        canonical_output_sha256: &str,
    ) -> Result<JobAttemptHistory, JobBackendError> {
        self.catalog
            .record_full_verification(run_id, canonical_output_sha256)
    }

    fn attempt(&self, run_id: &RunId) -> Result<JobAttemptHistory, JobBackendError> {
        self.catalog.attempt(run_id)
    }

    fn prune_plan(&self) -> Result<PrunePlan, JobBackendError> {
        self.catalog.prune_plan()
    }
}

/// Runner hook backed directly by the durable worker lease and event queue.
pub struct CatalogRunnerControl {
    catalog: LocalJobCatalog,
    run_id: RunId,
    worker_start_token: String,
    sample_interval: Duration,
    started: Instant,
    last_sample: Option<Instant>,
}

impl CatalogRunnerControl {
    /// Opens a worker-side catalog connection for macro-step polling.
    pub fn open(
        catalog_path: &Path,
        run_id: RunId,
        worker_start_token: String,
        sample_interval: Duration,
    ) -> Result<Self, JobBackendError> {
        if worker_start_token.trim().is_empty() || sample_interval.is_zero() {
            return Err(JobBackendError::InvalidRequest(
                "worker control token or sample interval is invalid".into(),
            ));
        }
        let catalog = LocalJobCatalog::open(catalog_path)?;
        catalog.configure_worker_telemetry()?;
        Ok(Self {
            catalog,
            run_id,
            worker_start_token,
            sample_interval,
            started: Instant::now(),
            last_sample: None,
        })
    }
}

impl RunnerControl for CatalogRunnerControl {
    fn macro_step_boundary(
        &mut self,
        progress: RunnerProgress,
    ) -> Result<RunnerControlDecision, String> {
        let now = Instant::now();
        if self
            .last_sample
            .is_some_and(|last| now.duration_since(last) < self.sample_interval)
        {
            return Ok(RunnerControlDecision::Continue);
        }
        let control = self
            .catalog
            .worker_control(&self.run_id, &self.worker_start_token)
            .map_err(|error| error.to_string())?;
        let at = system_timestamp().map_err(|error| error.to_string())?;
        self.catalog
            .heartbeat(
                &self.run_id,
                &self.worker_start_token,
                at,
                Some(JobProgress {
                    completed_macro_steps: progress.completed_macro_steps,
                    simulation_time: Some(progress.simulation_time),
                    active_particles: progress.active_particles,
                    normal_terminations: progress.normal_terminations,
                    abnormal_terminations: progress.abnormal_terminations,
                }),
                Some(ResourceObservation {
                    wall_time_ms: u64::try_from(self.started.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                    cpu_time_ms: None,
                    rss_bytes: None,
                }),
            )
            .map_err(|error| error.to_string())?;
        self.last_sample = Some(now);
        match control {
            WorkerControl::Continue => Ok(RunnerControlDecision::Continue),
            WorkerControl::SafeCancel => Ok(RunnerControlDecision::Cancel),
            WorkerControl::ForceStop => Err(
                "force-stop reached cooperative runner control before process termination".into(),
            ),
        }
    }
}

fn scheduler_error(error: SchedulerError) -> JobBackendError {
    match error {
        SchedulerError::InvalidCapacity => {
            JobBackendError::InvalidRequest("invalid daemon scheduling capacity".into())
        }
        SchedulerError::InvalidActiveUsage => {
            JobBackendError::Storage("active resource reservations exceed capacity".into())
        }
        SchedulerError::InvalidRequest(run_id) => {
            JobBackendError::Storage(format!("invalid queued resource request for {}", run_id.0))
        }
        SchedulerError::RequestExceedsCapacity(run_id) => JobBackendError::Storage(format!(
            "queued request unexpectedly exceeds capacity for {}",
            run_id.0
        )),
        SchedulerError::InvalidBypassCount(run_id) => {
            JobBackendError::Storage(format!("invalid queued bypass count for {}", run_id.0))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::model::{ResourceRequest, RunInput};

    #[derive(Default)]
    struct FakeProcesses {
        next_pid: u32,
        launch_fail_after: Option<usize>,
        launch_count: usize,
        stopped: Vec<RunId>,
    }

    impl WorkerProcessManager for FakeProcesses {
        fn launch(&mut self, snapshot: &JobSnapshot) -> Result<SpawnedWorker, String> {
            self.launch_count += 1;
            if self.launch_fail_after == Some(self.launch_count) {
                return Err("forced launch failure".into());
            }
            self.next_pid += 1;
            Ok(SpawnedWorker {
                worker_pid: self.next_pid,
                worker_start_token: format!("start-{}", snapshot.run_id.0),
            })
        }

        fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String> {
            self.stopped.push(lease.run_id.clone());
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct RecordingTerminator {
        stopped: Arc<Mutex<Vec<RunId>>>,
        fail: bool,
        terminal: Option<JobState>,
    }

    impl WorkerTerminator for RecordingTerminator {
        fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String> {
            if self.fail {
                return Err("forced stop failure".into());
            }
            self.stopped.lock().unwrap().push(lease.run_id.clone());
            Ok(())
        }

        fn terminal_state_after_force_stop(
            &mut self,
            _snapshot: &JobSnapshot,
            _at: Timestamp,
        ) -> Result<JobState, String> {
            Ok(self.terminal.unwrap_or(JobState::Interrupted))
        }
    }

    fn request(root: PathBuf, cpu_slots: u32, memory_mib: u64) -> SubmitRequest {
        SubmitRequest {
            input: RunInput::Project {
                project_root: root,
                profile_name: "default".into(),
            },
            resources: ResourceRequest {
                cpu_slots,
                memory_mib,
                worker_threads: cpu_slots,
            },
        }
    }

    fn capacity() -> SchedulerCapacity {
        SchedulerCapacity {
            cpu_slots: 4,
            memory_mib: 4_096,
        }
    }

    #[test]
    fn daemon_rejects_impossible_requests_before_durable_queue_acceptance() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut backend = DaemonControlBackend::new(
            LocalJobCatalog::open_in_memory().unwrap(),
            capacity(),
            RecordingTerminator::default(),
        )
        .unwrap();
        assert!(backend.submit(request(root, 5, 256)).is_err());
        assert!(
            backend
                .list(&JobListQuery {
                    states: Vec::new(),
                    limit: 10,
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn dispatch_launches_independent_workers_and_terminalizes_launch_failure() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut backend = DaemonControlBackend::new(
            LocalJobCatalog::open_in_memory().unwrap(),
            capacity(),
            RecordingTerminator::default(),
        )
        .unwrap();
        let first = backend.submit(request(root.clone(), 1, 256)).unwrap();
        let second = backend.submit(request(root, 1, 256)).unwrap();
        let mut processes = FakeProcesses {
            next_pid: 100,
            launch_fail_after: Some(2),
            ..FakeProcesses::default()
        };
        let report = dispatch_once(
            backend.catalog_mut(),
            capacity(),
            "daemon-a",
            false,
            system_timestamp().unwrap(),
            &mut processes,
        )
        .unwrap();
        assert_eq!(report.launched_run_ids, vec![first.run_id.clone()]);
        assert_eq!(report.failed_run_ids, vec![second.run_id.clone()]);
        assert!(
            backend
                .catalog()
                .worker_lease(&first.run_id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            backend.status(&second.job_series_id).unwrap().state,
            JobState::Failed
        );
    }

    #[test]
    fn force_cancel_stops_matching_worker_before_marking_interrupted() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let stopped = Arc::new(Mutex::new(Vec::new()));
        let mut backend = DaemonControlBackend::new(
            LocalJobCatalog::open_in_memory().unwrap(),
            capacity(),
            RecordingTerminator {
                stopped: stopped.clone(),
                fail: false,
                terminal: None,
            },
        )
        .unwrap();
        let receipt = backend.submit(request(root, 1, 256)).unwrap();
        dispatch_once(
            backend.catalog_mut(),
            capacity(),
            "daemon-a",
            false,
            Timestamp::UNIX_EPOCH,
            &mut FakeProcesses {
                next_pid: 100,
                ..FakeProcesses::default()
            },
        )
        .unwrap();
        let snapshot = backend
            .cancel(&receipt.job_series_id, CancelMode::Force)
            .unwrap();
        assert_eq!(snapshot.state, JobState::Interrupted);
        assert_eq!(*stopped.lock().unwrap(), vec![receipt.run_id]);
    }

    #[test]
    fn force_cancel_reconciles_an_already_terminal_manifest_state() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let stopped = Arc::new(Mutex::new(Vec::new()));
        let mut backend = DaemonControlBackend::new(
            LocalJobCatalog::open_in_memory().unwrap(),
            capacity(),
            RecordingTerminator {
                stopped: stopped.clone(),
                fail: false,
                terminal: Some(JobState::Complete),
            },
        )
        .unwrap();
        let receipt = backend.submit(request(root, 1, 256)).unwrap();
        dispatch_once(
            backend.catalog_mut(),
            capacity(),
            "daemon-a",
            false,
            Timestamp::UNIX_EPOCH,
            &mut FakeProcesses {
                next_pid: 100,
                ..FakeProcesses::default()
            },
        )
        .unwrap();
        let token = backend
            .catalog()
            .worker_lease(&receipt.run_id)
            .unwrap()
            .unwrap()
            .worker_start_token;
        backend
            .catalog_mut()
            .mark_worker_running(&receipt.run_id, &token, system_timestamp().unwrap())
            .unwrap();

        let snapshot = backend
            .cancel(&receipt.job_series_id, CancelMode::Force)
            .unwrap();
        assert_eq!(snapshot.state, JobState::Complete);
        assert_eq!(*stopped.lock().unwrap(), vec![receipt.run_id]);
    }

    #[test]
    fn force_stop_failure_keeps_active_state_for_forensic_retry() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut backend = DaemonControlBackend::new(
            LocalJobCatalog::open_in_memory().unwrap(),
            capacity(),
            RecordingTerminator {
                fail: true,
                ..RecordingTerminator::default()
            },
        )
        .unwrap();
        let receipt = backend.submit(request(root, 1, 256)).unwrap();
        dispatch_once(
            backend.catalog_mut(),
            capacity(),
            "daemon-a",
            false,
            Timestamp::UNIX_EPOCH,
            &mut FakeProcesses {
                next_pid: 100,
                ..FakeProcesses::default()
            },
        )
        .unwrap();
        assert!(
            backend
                .cancel(&receipt.job_series_id, CancelMode::Force)
                .is_err()
        );
        assert_eq!(
            backend.status(&receipt.job_series_id).unwrap().state,
            JobState::Starting
        );
    }

    #[test]
    fn runner_control_observes_safe_cancel_at_the_configured_sample_interval() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let catalog_path = temp.path().join("jobs.sqlite");
        let mut catalog = LocalJobCatalog::open(&catalog_path).unwrap();
        let receipt = catalog.submit(request(root, 1, 256)).unwrap();
        dispatch_once(
            &mut catalog,
            capacity(),
            "daemon-a",
            false,
            Timestamp::UNIX_EPOCH,
            &mut FakeProcesses {
                next_pid: 100,
                ..FakeProcesses::default()
            },
        )
        .unwrap();
        let token = catalog
            .worker_lease(&receipt.run_id)
            .unwrap()
            .unwrap()
            .worker_start_token;
        catalog
            .mark_worker_running(&receipt.run_id, &token, system_timestamp().unwrap())
            .unwrap();

        let sample_interval = Duration::from_secs(60);
        let mut control = CatalogRunnerControl::open(
            &catalog_path,
            receipt.run_id.clone(),
            token,
            sample_interval,
        )
        .unwrap();
        let progress = RunnerProgress {
            completed_macro_steps: 1,
            simulation_time: Timestamp::UNIX_EPOCH,
            active_particles: 1,
            normal_terminations: 0,
            abnormal_terminations: 0,
        };

        assert_eq!(
            control.macro_step_boundary(progress).unwrap(),
            RunnerControlDecision::Continue
        );
        catalog
            .cancel(&receipt.job_series_id, CancelMode::Safe)
            .unwrap();
        assert_eq!(
            control.macro_step_boundary(progress).unwrap(),
            RunnerControlDecision::Continue
        );

        control.last_sample = Some(Instant::now() - sample_interval);
        assert_eq!(
            control.macro_step_boundary(progress).unwrap(),
            RunnerControlDecision::Cancel
        );
    }
}
