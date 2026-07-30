//! # Contract: local daemon request transport
//!
//! One request and one response are exchanged per local stream connection.
//! Frames are bounded, length-prefixed JSON. Transport selection is fixed by
//! the platform: Windows named pipe or Unix domain socket, never TCP.

use std::io::{self, Read, Write};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use trajecta_core::manifest::{JobSeriesId, RunId};
use trajecta_local_ipc::{LocalListener, LocalStream};

use crate::backend::{JobBackend, JobBackendError};
use crate::history::{JobAttemptHistory, JobHistoryBackend, PrunePlan};
use crate::model::{
    CancelMode, EventQuery, JobEvent, JobListQuery, JobReceipt, JobSnapshot, SubmitRequest,
};

/// Maximum accepted encoded request or response frame.
pub const MAXIMUM_IPC_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Same-release daemon operations, including A3 attempt history controls.
pub trait DaemonBackend: JobBackend + JobHistoryBackend {}

impl<T: JobBackend + JobHistoryBackend> DaemonBackend for T {}

/// Typed request accepted by the same-release local daemon.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum DaemonRequest {
    /// Checks endpoint reachability without reading or changing the catalog.
    Ping,
    /// Durably accepts one new logical job series.
    Submit {
        /// Validated project or direct run request.
        request: SubmitRequest,
    },
    /// Returns bounded job snapshots.
    List {
        /// List filter and bound.
        query: JobListQuery,
    },
    /// Returns the latest attempt for one series.
    Status {
        /// Logical job-series identity.
        job_series_id: JobSeriesId,
    },
    /// Requests safe or forced cancellation.
    Cancel {
        /// Logical job-series identity.
        job_series_id: JobSeriesId,
        /// Requested cancellation strength.
        mode: CancelMode,
    },
    /// Returns persistent events after a cursor.
    Events {
        /// Fan-out event query.
        query: EventQuery,
    },
    /// Creates a new attempt in an existing terminal series.
    Rerun {
        /// Logical job-series identity.
        job_series_id: JobSeriesId,
    },
    /// Hides a terminal series from routine list output.
    Forget {
        /// Logical job-series identity.
        job_series_id: JobSeriesId,
    },
    /// Returns all attempts and verification relationships for one series.
    AttemptHistory {
        /// Logical job-series identity.
        job_series_id: JobSeriesId,
    },
    /// Returns one exact attempt by run identity.
    Attempt {
        /// Unique attempt identity.
        run_id: RunId,
    },
    /// Records successful full verification for one Complete attempt.
    RecordFullVerification {
        /// Unique attempt identity.
        run_id: RunId,
        /// Independently recomputed canonical output digest.
        canonical_output_sha256: String,
    },
    /// Returns the global dry-run pruning plan.
    PrunePlan,
    /// Internal authenticated request used only for graceful idle shutdown.
    Shutdown {
        /// Per-process token never exposed by public CLI output.
        token: String,
    },
}

/// Typed successful daemon response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "result",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DaemonSuccess {
    /// Endpoint is alive.
    Pong,
    /// Durable submission identity.
    Receipt(JobReceipt),
    /// Bounded list result.
    Snapshots(Vec<JobSnapshot>),
    /// One latest job snapshot.
    Snapshot(Box<JobSnapshot>),
    /// Persistent event rows.
    Events(Vec<JobEvent>),
    /// Durable attempt history rows.
    AttemptHistory(Vec<JobAttemptHistory>),
    /// One updated attempt-history row.
    Attempt(Box<JobAttemptHistory>),
    /// Deterministic dry-run pruning plan.
    PrunePlan(Box<PrunePlan>),
    /// Daemon accepted its own graceful shutdown request.
    ShuttingDown,
}

/// Stable backend-error category transported across local IPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonErrorKind {
    /// Request violated a control-plane contract.
    InvalidRequest,
    /// Logical series was not found.
    NotFound,
    /// Current lifecycle or ownership conflicts with the operation.
    Conflict,
    /// Daemon or worker control is temporarily unavailable.
    Unavailable,
    /// Durable storage failed or was corrupt.
    Storage,
}

/// Structured error returned by the daemon.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonError {
    /// Stable backend-error category.
    pub kind: DaemonErrorKind,
    /// Stable diagnostic code.
    pub code: String,
    /// Human-readable message without secrets.
    pub message: String,
}

impl From<JobBackendError> for DaemonError {
    fn from(error: JobBackendError) -> Self {
        let kind = match error {
            JobBackendError::InvalidRequest(_) => DaemonErrorKind::InvalidRequest,
            JobBackendError::NotFound => DaemonErrorKind::NotFound,
            JobBackendError::Conflict(_) => DaemonErrorKind::Conflict,
            JobBackendError::Unavailable(_) => DaemonErrorKind::Unavailable,
            JobBackendError::Storage(_) => DaemonErrorKind::Storage,
        };
        Self {
            kind,
            code: error.code().into(),
            message: error.to_string(),
        }
    }
}

impl From<DaemonError> for JobBackendError {
    fn from(error: DaemonError) -> Self {
        match error.kind {
            DaemonErrorKind::InvalidRequest => Self::InvalidRequest(error.message),
            DaemonErrorKind::NotFound => Self::NotFound,
            DaemonErrorKind::Conflict => Self::Conflict(error.message),
            DaemonErrorKind::Unavailable => Self::Unavailable(error.message),
            DaemonErrorKind::Storage => Self::Storage(error.message),
        }
    }
}

/// One complete daemon response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DaemonResponse {
    /// Request completed successfully.
    Ok(DaemonSuccess),
    /// Request reached the daemon but failed.
    Error(DaemonError),
}

/// Blocking local IPC client implementing the shared backend interface.
#[derive(Clone, Debug)]
pub struct LocalJobClient {
    endpoint: String,
    connect_timeout: Duration,
}

impl LocalJobClient {
    /// Creates a client for a platform-local endpoint.
    #[must_use]
    pub fn new(endpoint: impl Into<String>, connect_timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into(),
            connect_timeout,
        }
    }

    /// Checks that a compatible daemon accepts framed requests.
    pub fn ping(&self) -> Result<(), JobBackendError> {
        match self.request(&DaemonRequest::Ping)? {
            DaemonSuccess::Pong => Ok(()),
            _ => Err(JobBackendError::Unavailable(
                "daemon returned the wrong ping response".into(),
            )),
        }
    }

    /// Requests graceful shutdown using the daemon-private token.
    pub fn shutdown(&self, token: String) -> Result<(), JobBackendError> {
        match self.request(&DaemonRequest::Shutdown { token })? {
            DaemonSuccess::ShuttingDown => Ok(()),
            _ => Err(JobBackendError::Unavailable(
                "daemon returned the wrong shutdown response".into(),
            )),
        }
    }

    fn request(&self, request: &DaemonRequest) -> Result<DaemonSuccess, JobBackendError> {
        let mut stream = LocalStream::connect(&self.endpoint, self.connect_timeout)
            .map_err(|error| JobBackendError::Unavailable(error.to_string()))?;
        write_frame(&mut stream, request)
            .map_err(|error| JobBackendError::Unavailable(error.to_string()))?;
        match read_frame::<_, DaemonResponse>(&mut stream)
            .map_err(|error| JobBackendError::Unavailable(error.to_string()))?
        {
            DaemonResponse::Ok(success) => Ok(success),
            DaemonResponse::Error(error) => Err(error.into()),
        }
    }
}

impl JobBackend for LocalJobClient {
    fn submit(&mut self, request: SubmitRequest) -> Result<JobReceipt, JobBackendError> {
        match self.request(&DaemonRequest::Submit { request })? {
            DaemonSuccess::Receipt(receipt) => Ok(receipt),
            _ => Err(wrong_response("submit")),
        }
    }

    fn list(&self, query: &JobListQuery) -> Result<Vec<JobSnapshot>, JobBackendError> {
        match self.request(&DaemonRequest::List {
            query: query.clone(),
        })? {
            DaemonSuccess::Snapshots(snapshots) => Ok(snapshots),
            _ => Err(wrong_response("list")),
        }
    }

    fn status(&self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError> {
        match self.request(&DaemonRequest::Status {
            job_series_id: job_series_id.clone(),
        })? {
            DaemonSuccess::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err(wrong_response("status")),
        }
    }

    fn cancel(
        &mut self,
        job_series_id: &JobSeriesId,
        mode: CancelMode,
    ) -> Result<JobSnapshot, JobBackendError> {
        match self.request(&DaemonRequest::Cancel {
            job_series_id: job_series_id.clone(),
            mode,
        })? {
            DaemonSuccess::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err(wrong_response("cancel")),
        }
    }

    fn events(&self, query: &EventQuery) -> Result<Vec<JobEvent>, JobBackendError> {
        match self.request(&DaemonRequest::Events {
            query: query.clone(),
        })? {
            DaemonSuccess::Events(events) => Ok(events),
            _ => Err(wrong_response("events")),
        }
    }
}

impl JobHistoryBackend for LocalJobClient {
    fn rerun(&mut self, job_series_id: &JobSeriesId) -> Result<JobReceipt, JobBackendError> {
        match self.request(&DaemonRequest::Rerun {
            job_series_id: job_series_id.clone(),
        })? {
            DaemonSuccess::Receipt(receipt) => Ok(receipt),
            _ => Err(wrong_response("rerun")),
        }
    }

    fn forget(&mut self, job_series_id: &JobSeriesId) -> Result<JobSnapshot, JobBackendError> {
        match self.request(&DaemonRequest::Forget {
            job_series_id: job_series_id.clone(),
        })? {
            DaemonSuccess::Snapshot(snapshot) => Ok(*snapshot),
            _ => Err(wrong_response("forget")),
        }
    }

    fn attempt_history(
        &self,
        job_series_id: &JobSeriesId,
    ) -> Result<Vec<JobAttemptHistory>, JobBackendError> {
        match self.request(&DaemonRequest::AttemptHistory {
            job_series_id: job_series_id.clone(),
        })? {
            DaemonSuccess::AttemptHistory(history) => Ok(history),
            _ => Err(wrong_response("attempt history")),
        }
    }

    fn record_full_verification(
        &mut self,
        run_id: &RunId,
        canonical_output_sha256: &str,
    ) -> Result<JobAttemptHistory, JobBackendError> {
        match self.request(&DaemonRequest::RecordFullVerification {
            run_id: run_id.clone(),
            canonical_output_sha256: canonical_output_sha256.into(),
        })? {
            DaemonSuccess::Attempt(history) => Ok(*history),
            _ => Err(wrong_response("record full verification")),
        }
    }

    fn attempt(&self, run_id: &RunId) -> Result<JobAttemptHistory, JobBackendError> {
        match self.request(&DaemonRequest::Attempt {
            run_id: run_id.clone(),
        })? {
            DaemonSuccess::Attempt(history) => Ok(*history),
            _ => Err(wrong_response("attempt")),
        }
    }

    fn prune_plan(&self) -> Result<PrunePlan, JobBackendError> {
        match self.request(&DaemonRequest::PrunePlan)? {
            DaemonSuccess::PrunePlan(plan) => Ok(*plan),
            _ => Err(wrong_response("prune plan")),
        }
    }
}

/// Blocking local daemon listener serving one request per accepted connection.
pub struct LocalJobServer {
    listener: LocalListener,
}

impl LocalJobServer {
    /// Binds the platform-local endpoint; an existing listener is an error.
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        LocalListener::bind(endpoint).map(|listener| Self { listener })
    }

    /// Accepts, executes, and responds to one request.
    pub fn serve_one(&mut self, backend: &mut dyn DaemonBackend) -> io::Result<()> {
        self.serve_one_controlled(backend, None).map(|_| ())
    }

    /// Serves one request and reports whether the listener should continue.
    ///
    /// Public clients never receive the shutdown token. The daemon uses this
    /// path only after it has independently established that the queue and
    /// worker set are idle.
    pub fn serve_one_controlled(
        &mut self,
        backend: &mut dyn DaemonBackend,
        shutdown_token: Option<&str>,
    ) -> io::Result<bool> {
        let mut stream = self.listener.accept()?;
        let (response, keep_serving) = match read_frame::<_, DaemonRequest>(&mut stream) {
            Ok(DaemonRequest::Shutdown { token }) => {
                if shutdown_token.is_some_and(|expected| expected == token) {
                    (DaemonResponse::Ok(DaemonSuccess::ShuttingDown), false)
                } else {
                    (
                        DaemonResponse::Error(DaemonError {
                            kind: DaemonErrorKind::Conflict,
                            code: "job_backend.conflict".into(),
                            message: "daemon shutdown token does not match".into(),
                        }),
                        true,
                    )
                }
            }
            Ok(request) => (execute_request(backend, request), true),
            Err(error) => (
                DaemonResponse::Error(DaemonError {
                    kind: DaemonErrorKind::InvalidRequest,
                    code: "job_backend.invalid_request".into(),
                    message: error.to_string(),
                }),
                true,
            ),
        };
        write_frame(&mut stream, &response)?;
        Ok(keep_serving)
    }
}

fn execute_request(backend: &mut dyn DaemonBackend, request: DaemonRequest) -> DaemonResponse {
    let result = match request {
        DaemonRequest::Ping => Ok(DaemonSuccess::Pong),
        DaemonRequest::Submit { request } => backend.submit(request).map(DaemonSuccess::Receipt),
        DaemonRequest::List { query } => backend.list(&query).map(DaemonSuccess::Snapshots),
        DaemonRequest::Status { job_series_id } => backend
            .status(&job_series_id)
            .map(Box::new)
            .map(DaemonSuccess::Snapshot),
        DaemonRequest::Cancel {
            job_series_id,
            mode,
        } => backend
            .cancel(&job_series_id, mode)
            .map(Box::new)
            .map(DaemonSuccess::Snapshot),
        DaemonRequest::Events { query } => backend.events(&query).map(DaemonSuccess::Events),
        DaemonRequest::Rerun { job_series_id } => {
            backend.rerun(&job_series_id).map(DaemonSuccess::Receipt)
        }
        DaemonRequest::Forget { job_series_id } => backend
            .forget(&job_series_id)
            .map(Box::new)
            .map(DaemonSuccess::Snapshot),
        DaemonRequest::AttemptHistory { job_series_id } => backend
            .attempt_history(&job_series_id)
            .map(DaemonSuccess::AttemptHistory),
        DaemonRequest::Attempt { run_id } => backend
            .attempt(&run_id)
            .map(Box::new)
            .map(DaemonSuccess::Attempt),
        DaemonRequest::RecordFullVerification {
            run_id,
            canonical_output_sha256,
        } => backend
            .record_full_verification(&run_id, &canonical_output_sha256)
            .map(Box::new)
            .map(DaemonSuccess::Attempt),
        DaemonRequest::PrunePlan => backend
            .prune_plan()
            .map(Box::new)
            .map(DaemonSuccess::PrunePlan),
        DaemonRequest::Shutdown { .. } => Err(JobBackendError::Conflict(
            "daemon shutdown is unavailable on this listener".into(),
        )),
    };
    match result {
        Ok(success) => DaemonResponse::Ok(success),
        Err(error) => DaemonResponse::Error(error.into()),
    }
}

fn write_frame<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.is_empty() || payload.len() > MAXIMUM_IPC_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local IPC frame length is outside the frozen bound",
        ));
    }
    let length = u32::try_from(payload.len()).map_err(io::Error::other)?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

fn read_frame<R, T>(reader: &mut R) -> io::Result<T>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut encoded_length = [0_u8; 4];
    reader.read_exact(&mut encoded_length)?;
    let length = u32::from_be_bytes(encoded_length) as usize;
    if length == 0 || length > MAXIMUM_IPC_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local IPC frame length is outside the frozen bound",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).map_err(io::Error::other)
}

fn wrong_response(command: &str) -> JobBackendError {
    JobBackendError::Unavailable(format!("daemon returned the wrong response for {command}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::catalog::{LocalJobCatalog, TransitionDiagnostic, system_timestamp};
    use crate::model::{JobState, ResourceRequest, RunInput};

    fn endpoint(root: &Path) -> String {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        #[cfg(windows)]
        {
            let _ = root;
            format!(
                r"\\.\pipe\trajecta-job-ipc-test-{}-{nonce}",
                std::process::id()
            )
        }
        #[cfg(unix)]
        {
            root.join(format!("job-ipc-{nonce}.sock"))
                .to_string_lossy()
                .into_owned()
        }
    }

    fn request(root: PathBuf) -> SubmitRequest {
        SubmitRequest {
            input: RunInput::Project {
                project_root: root,
                profile_name: "default".into(),
            },
            resources: ResourceRequest {
                cpu_slots: 1,
                memory_mib: 256,
                worker_threads: 1,
            },
        }
    }

    #[test]
    fn local_backend_roundtrip_is_typed_and_event_reads_are_fanout() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let endpoint = endpoint(&root);
        let server_endpoint = endpoint.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let server = thread::spawn(move || {
            let mut server = LocalJobServer::bind(&server_endpoint).unwrap();
            let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
            ready_tx.send(()).unwrap();
            for _ in 0..7 {
                server.serve_one(&mut catalog).unwrap();
            }
        });
        ready_rx.recv().unwrap();

        let mut client = LocalJobClient::new(endpoint, Duration::from_secs(2));
        client.ping().unwrap();
        let receipt = client.submit(request(root)).unwrap();
        let snapshot = client.status(&receipt.job_series_id).unwrap();
        assert_eq!(snapshot.state, JobState::Queued);
        assert_eq!(
            client
                .list(&JobListQuery {
                    states: vec![JobState::Queued],
                    limit: 10,
                })
                .unwrap()
                .len(),
            1
        );
        let events_query = EventQuery {
            job_series_id: Some(receipt.job_series_id.clone()),
            after_sequence: None,
            limit: 10,
        };
        let first = client.events(&events_query).unwrap();
        let second = client.events(&events_query).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            client
                .cancel(&receipt.job_series_id, CancelMode::Safe)
                .unwrap()
                .state,
            JobState::Cancelled
        );
        server.join().unwrap();
    }

    #[test]
    fn history_roundtrip_preserves_attempts_and_returns_only_dry_run_pruning() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let first_output = root.join("attempt-1");
        let second_output = root.join("attempt-2");
        std::fs::create_dir(&first_output).unwrap();
        std::fs::create_dir(&second_output).unwrap();
        std::fs::write(first_output.join("old.bin"), b"old").unwrap();
        std::fs::write(second_output.join("new.bin"), b"new").unwrap();
        let endpoint = endpoint(&root);
        let server_endpoint = endpoint.clone();
        let server_root = root.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let server = thread::spawn(move || {
            let mut server = LocalJobServer::bind(&server_endpoint).unwrap();
            let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
            let first = catalog.submit(request(server_root)).unwrap();
            catalog
                .transition(
                    &first.run_id,
                    JobState::Starting,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            catalog
                .set_output_directory(&first.run_id, &first_output)
                .unwrap();
            catalog
                .transition(
                    &first.run_id,
                    JobState::Running,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            catalog
                .transition(
                    &first.run_id,
                    JobState::Complete,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            let second = catalog.rerun(&first.job_series_id).unwrap();
            catalog
                .transition(
                    &second.run_id,
                    JobState::Starting,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            catalog
                .set_output_directory(&second.run_id, &second_output)
                .unwrap();
            catalog
                .transition(
                    &second.run_id,
                    JobState::Running,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            catalog
                .transition(
                    &second.run_id,
                    JobState::Complete,
                    system_timestamp().unwrap(),
                    TransitionDiagnostic::default(),
                )
                .unwrap();
            ready_tx.send((first.job_series_id, second.run_id)).unwrap();
            for _ in 0..7 {
                server.serve_one(&mut catalog).unwrap();
            }
        });
        let (job_series_id, latest_run_id) = ready_rx.recv().unwrap();

        let mut client = LocalJobClient::new(endpoint, Duration::from_secs(2));
        client
            .record_full_verification(&latest_run_id, &"ab".repeat(32))
            .unwrap();
        let history = client.attempt_history(&job_series_id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].superseded_by, Some(latest_run_id.clone()));
        let plan = client.prune_plan().unwrap();
        assert!(!plan.delete_enabled);
        assert_eq!(plan.candidates.len(), 1);
        client.forget(&job_series_id).unwrap();
        assert!(
            client
                .list(&JobListQuery {
                    states: Vec::new(),
                    limit: 10,
                })
                .unwrap()
                .is_empty()
        );
        let rerun = client.rerun(&job_series_id).unwrap();
        assert_eq!(rerun.attempt, 3);
        assert_eq!(client.status(&job_series_id).unwrap().run_id, rerun.run_id);
        server.join().unwrap();
    }

    #[test]
    fn bounded_frames_reject_zero_and_oversized_lengths_before_allocation() {
        let mut zero = Cursor::new(0_u32.to_be_bytes());
        assert_eq!(
            read_frame::<_, DaemonRequest>(&mut zero)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let oversized = u32::try_from(MAXIMUM_IPC_FRAME_BYTES + 1).unwrap();
        let mut oversized = Cursor::new(oversized.to_be_bytes());
        assert_eq!(
            read_frame::<_, DaemonRequest>(&mut oversized)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn graceful_shutdown_requires_the_daemon_private_token() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let endpoint = endpoint(&root);
        let server_endpoint = endpoint.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let server = thread::spawn(move || {
            let mut server = LocalJobServer::bind(&server_endpoint).unwrap();
            let mut catalog = LocalJobCatalog::open_in_memory().unwrap();
            ready_tx.send(()).unwrap();
            assert!(
                server
                    .serve_one_controlled(&mut catalog, Some("secret"))
                    .unwrap()
            );
            assert!(
                !server
                    .serve_one_controlled(&mut catalog, Some("secret"))
                    .unwrap()
            );
        });
        ready_rx.recv().unwrap();
        let client = LocalJobClient::new(endpoint, Duration::from_secs(2));
        assert!(client.shutdown("wrong".into()).is_err());
        client.shutdown("secret".into()).unwrap();
        server.join().unwrap();
    }
}
