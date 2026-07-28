use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::RunId;
use trajecta_job::catalog::{WorkerLease, WorkerRecoveryDisposition};
use trajecta_job::daemon::{SpawnedWorker, WorkerProcessManager, WorkerTerminator};
use trajecta_job::model::{JobSnapshot, JobState};
use trajecta_local_ipc::{ProcessIdentity, force_terminate_process, process_identity};

use super::recovery_manifest_disposition;

pub(super) struct HostWorkerProcesses {
    executable: PathBuf,
    config_path: PathBuf,
    log_root: PathBuf,
    children: BTreeMap<RunId, Child>,
}

impl HostWorkerProcesses {
    pub(super) fn new(executable: PathBuf, config_path: PathBuf, log_root: PathBuf) -> Self {
        Self {
            executable,
            config_path,
            log_root,
            children: BTreeMap::new(),
        }
    }

    pub(super) fn reap_finished(&mut self) {
        let finished = self
            .children
            .iter_mut()
            .filter_map(|(run_id, child)| match child.try_wait() {
                Ok(Some(_)) => Some(run_id.clone()),
                Ok(None) | Err(_) => None,
            })
            .collect::<Vec<_>>();
        for run_id in finished {
            self.children.remove(&run_id);
        }
    }
}

impl WorkerProcessManager for HostWorkerProcesses {
    fn launch(&mut self, snapshot: &JobSnapshot) -> Result<SpawnedWorker, String> {
        fs::create_dir_all(&self.log_root).map_err(|error| error.to_string())?;
        let log_path = self.log_root.join(format!("{}.log", snapshot.run_id.0));
        let stdout = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&log_path)
            .map_err(|error| format!("create worker log {}: {error}", log_path.display()))?;
        let stderr = stdout.try_clone().map_err(|error| error.to_string())?;
        let mut command = ProcessCommand::new(&self.executable);
        command
            .arg("__worker")
            .arg("--config")
            .arg(&self.config_path)
            .arg("--run-id")
            .arg(&snapshot.run_id.0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        configure_worker_process(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("spawn worker: {error}"))?;
        let pid = child.id();
        let started = Instant::now();
        let identity = loop {
            match process_identity(pid) {
                Ok(identity) => break identity,
                Err(error) if started.elapsed() < Duration::from_secs(2) => {
                    if child.try_wait().map_err(|wait| wait.to_string())?.is_some() {
                        return Err(format!("worker exited before identity probe: {error}"));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("probe worker identity: {error}"));
                }
            }
        };
        self.children.insert(snapshot.run_id.clone(), child);
        Ok(SpawnedWorker {
            worker_pid: identity.pid,
            worker_start_token: identity.start_token,
        })
    }

    fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String> {
        force_worker(lease)?;
        if let Some(mut child) = self.children.remove(&lease.run_id) {
            child.wait().map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

#[cfg(windows)]
fn configure_worker_process(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn configure_worker_process(_command: &mut ProcessCommand) {}

#[derive(Clone, Copy)]
pub(super) struct HostTerminator;

impl WorkerTerminator for HostTerminator {
    fn force_stop(&mut self, lease: &WorkerLease) -> Result<(), String> {
        force_worker(lease)
    }

    fn terminal_state_after_force_stop(
        &mut self,
        snapshot: &JobSnapshot,
        at: Timestamp,
    ) -> Result<JobState, String> {
        match recovery_manifest_disposition(snapshot, at).map_err(|error| error.message)? {
            WorkerRecoveryDisposition::ReconcileTerminal(state) => Ok(state),
            WorkerRecoveryDisposition::Interrupt => Ok(JobState::Interrupted),
            WorkerRecoveryDisposition::Reattach => {
                Err("force-stopped worker cannot be reattached".into())
            }
        }
    }
}

fn force_worker(lease: &WorkerLease) -> Result<(), String> {
    force_terminate_process(&ProcessIdentity {
        pid: lease.worker_pid,
        start_token: lease.worker_start_token.clone(),
    })
    .map_err(|error| error.to_string())
}
