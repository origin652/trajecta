//! Local M5 daemon, worker, and public job-control command wiring.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use trajecta_case::diagnostic::Diagnostic;
use trajecta_case::document::{ExecutionSpec, ResolvedCase, ResolvedRunProfile};
use trajecta_case::expand::{expand_case_file, expand_run_profile_file};
use trajecta_case::intent::{IntentValidator, ValidationIntent};
use trajecta_case::model::time::Timestamp;
use trajecta_case::resolver::LocalRefResolver;
use trajecta_core::manifest::{JobSeriesId, RunFailure, RunId, RunLifecycleStatus, RunManifest};
use trajecta_core::manifest_store::AtomicRunManifestStore;
use trajecta_core::runner::{
    RunManifestStore, RunOutcome, RunnerAttemptIdentity, build_runner_for_attempt,
    run_directory_path,
};
use trajecta_job::backend::{JobBackend, JobBackendError};
use trajecta_job::catalog::{
    DaemonLease, LocalJobCatalog, TransitionDiagnostic, WorkerLease, WorkerRecoveryDisposition,
};
use trajecta_job::daemon::{
    CatalogRunnerControl, DaemonControlBackend, SpawnedWorker, WorkerProcessManager,
    WorkerTerminator, dispatch_once,
};
use trajecta_job::ipc::{LocalJobClient, LocalJobServer};
use trajecta_job::model::{
    CancelMode, EventQuery, JobEvent, JobListQuery, JobReceipt, JobSnapshot, JobState,
    ResourceRequest, RunInput as JobRunInput, SubmitRequest,
};
use trajecta_job::scheduler::SchedulerCapacity;
use trajecta_local_ipc::{
    ProcessIdentity, available_memory_bytes, current_process_identity, force_terminate_process,
    process_identity, process_identity_matches, suppress_standard_handle_inheritance,
};
use uuid::Uuid;

use crate::app::AppOutcome;
use crate::cli::OutputMode;
use crate::command::staged::{JobCommand, RunInput, StagedRunCommand};
use crate::configuration::RuntimeSettings;

const DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(10);
const WORKER_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);
const MIB: u64 = 1024 * 1024;

pub(crate) fn internal_entry(arguments: &[OsString]) -> Option<i32> {
    match arguments.get(1).and_then(|value| value.to_str()) {
        Some("__daemon") => Some(run_internal(
            parse_daemon_arguments(arguments)
                .and_then(|invocation| run_daemon(&invocation.config_path)),
        )),
        Some("__worker") => Some(run_internal(
            parse_worker_arguments(arguments)
                .and_then(|invocation| run_worker(&invocation.config_path, invocation.run_id)),
        )),
        _ => None,
    }
}

fn run_internal(result: Result<(), RuntimeError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "trajecta internal: {}: {}",
                error.code,
                error.message
            );
            1
        }
    }
}

struct DaemonInvocation {
    config_path: PathBuf,
}

struct WorkerInvocation {
    config_path: PathBuf,
    run_id: RunId,
}

fn parse_daemon_arguments(arguments: &[OsString]) -> Result<DaemonInvocation, RuntimeError> {
    if arguments.len() != 4 || arguments[2].to_str() != Some("--config") {
        return Err(RuntimeError::internal(
            "daemon.arguments",
            "hidden daemon mode requires exactly --config PATH",
        ));
    }
    Ok(DaemonInvocation {
        config_path: PathBuf::from(&arguments[3]),
    })
}

fn parse_worker_arguments(arguments: &[OsString]) -> Result<WorkerInvocation, RuntimeError> {
    if arguments.len() != 6
        || arguments[2].to_str() != Some("--config")
        || arguments[4].to_str() != Some("--run-id")
    {
        return Err(RuntimeError::internal(
            "worker.arguments",
            "hidden worker mode requires exactly --config PATH --run-id UUID",
        ));
    }
    let run_id = arguments[5]
        .to_str()
        .ok_or_else(|| RuntimeError::internal("worker.arguments", "run id is not UTF-8"))?;
    validate_uuid_v7(run_id, "worker.invalid_run_id")?;
    Ok(WorkerInvocation {
        config_path: PathBuf::from(&arguments[3]),
        run_id: RunId(run_id.into()),
    })
}

pub(crate) fn execute_run(
    command: &StagedRunCommand,
    config_path: Option<&Path>,
    output: OutputMode,
) -> i32 {
    let result = (|| {
        let settings = runtime_settings(config_path)?;
        let submission = resolve_submission(command)?;
        let mut client = daemon_client(&settings)?;
        let receipt = client.submit(submission).map_err(RuntimeError::backend)?;
        if command.detach {
            return Ok(PublicRunResult::Detached(receipt));
        }
        Ok(PublicRunResult::Foreground {
            client,
            receipt,
            poll_interval: poll_interval(&settings),
        })
    })();

    match result {
        Ok(PublicRunResult::Detached(receipt)) => crate::write_outcome(
            &AppOutcome::ok("run", serialize_value(&receipt).unwrap_or(Value::Null)),
            output,
        ),
        Ok(PublicRunResult::Foreground {
            client,
            receipt,
            poll_interval,
        }) => {
            let job_series_id = receipt.job_series_id.clone();
            wait_for_job(
                client,
                &job_series_id,
                "run",
                output,
                poll_interval,
                Some(receipt),
            )
        }
        Err(error) => write_runtime_error("run", error, output),
    }
}

enum PublicRunResult {
    Detached(JobReceipt),
    Foreground {
        client: LocalJobClient,
        receipt: JobReceipt,
        poll_interval: Duration,
    },
}

pub(crate) fn execute_job(
    command: &JobCommand,
    config_path: Option<&Path>,
    output: OutputMode,
) -> i32 {
    if matches!(
        command,
        JobCommand::Rerun(_) | JobCommand::Forget(_) | JobCommand::Prune
    ) {
        return crate::write_outcome(
            &AppOutcome::error(
                job_command_name(command),
                "command.stage_not_available",
                "command is reserved for M5-A3",
                "M5-A2 implements list, status, wait, events, and cancel",
            ),
            output,
        );
    }
    let settings = match runtime_settings(config_path) {
        Ok(settings) => settings,
        Err(error) => return write_runtime_error(job_command_name(command), error, output),
    };
    let mut client = match daemon_client(&settings) {
        Ok(client) => client,
        Err(error) => return write_runtime_error(job_command_name(command), error, output),
    };
    match command {
        JobCommand::List => match client.list(&JobListQuery {
            states: Vec::new(),
            limit: 1_000,
        }) {
            Ok(snapshots) => crate::write_outcome(
                &AppOutcome::ok(
                    "job list",
                    serialize_value(&snapshots).unwrap_or(Value::Null),
                ),
                output,
            ),
            Err(error) => write_runtime_error("job list", RuntimeError::backend(error), output),
        },
        JobCommand::Status { job_id } => {
            let id = match job_series_id(job_id) {
                Ok(id) => id,
                Err(error) => return write_runtime_error("job status", error, output),
            };
            match client.status(&id) {
                Ok(snapshot) => crate::write_outcome(
                    &AppOutcome::ok(
                        "job status",
                        serialize_value(&snapshot).unwrap_or(Value::Null),
                    ),
                    output,
                ),
                Err(error) => {
                    write_runtime_error("job status", RuntimeError::backend(error), output)
                }
            }
        }
        JobCommand::Wait { job_id } => {
            let id = match job_series_id(job_id) {
                Ok(id) => id,
                Err(error) => return write_runtime_error("job wait", error, output),
            };
            wait_for_job(
                client,
                &id,
                "job wait",
                output,
                poll_interval(&settings),
                None,
            )
        }
        JobCommand::Events {
            job_id,
            since,
            follow,
        } => {
            let id = match job_id.as_deref().map(job_series_id).transpose() {
                Ok(id) => id,
                Err(error) => return write_runtime_error("job events", error, output),
            };
            stream_events(
                client,
                id,
                *since,
                *follow,
                output,
                poll_interval(&settings),
            )
        }
        JobCommand::Cancel { job_id, force } => {
            let id = match job_series_id(job_id) {
                Ok(id) => id,
                Err(error) => return write_runtime_error("job cancel", error, output),
            };
            let mode = if *force {
                CancelMode::Force
            } else {
                CancelMode::Safe
            };
            match cancel_reliably(&mut client, &id, mode) {
                Ok(snapshot) => crate::write_outcome(
                    &AppOutcome::ok(
                        "job cancel",
                        serialize_value(&snapshot).unwrap_or(Value::Null),
                    ),
                    output,
                ),
                Err(error) => {
                    write_runtime_error("job cancel", RuntimeError::backend(error), output)
                }
            }
        }
        JobCommand::Rerun(_) | JobCommand::Forget(_) | JobCommand::Prune => 1,
    }
}

fn job_command_name(command: &JobCommand) -> &'static str {
    match command {
        JobCommand::List => "job list",
        JobCommand::Status { .. } => "job status",
        JobCommand::Wait { .. } => "job wait",
        JobCommand::Events { .. } => "job events",
        JobCommand::Cancel { .. } => "job cancel",
        JobCommand::Rerun(_) => "job rerun",
        JobCommand::Forget(_) => "job forget",
        JobCommand::Prune => "job prune",
    }
}

fn cancel_reliably(
    client: &mut LocalJobClient,
    job_series_id: &JobSeriesId,
    mode: CancelMode,
) -> Result<JobSnapshot, JobBackendError> {
    let started = Instant::now();
    loop {
        match client.cancel(job_series_id, mode) {
            Ok(snapshot) => return Ok(snapshot),
            Err(error) => {
                if let Ok(snapshot) = client.status(job_series_id) {
                    let reached = match mode {
                        CancelMode::Safe => {
                            matches!(snapshot.state, JobState::Cancelling | JobState::Cancelled)
                        }
                        CancelMode::Force => snapshot.state == JobState::Interrupted,
                    };
                    if reached {
                        return Ok(snapshot);
                    }
                }
                if !transient_backend(&error) || started.elapsed() >= Duration::from_secs(10) {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn wait_for_job(
    client: LocalJobClient,
    job_series_id: &JobSeriesId,
    command: &str,
    output: OutputMode,
    poll_interval: Duration,
    receipt: Option<JobReceipt>,
) -> i32 {
    if output == OutputMode::Json {
        loop {
            match client.status(job_series_id) {
                Ok(snapshot) if snapshot.state.is_terminal() => {
                    let exit_code = snapshot.state.wait_exit_code().unwrap_or(1);
                    let run_success = snapshot.state.run_success();
                    return crate::write_outcome(
                        &AppOutcome::ok(command, serialize_value(&snapshot).unwrap_or(Value::Null))
                            .with_run_success(run_success)
                            .with_exit_code(exit_code),
                        output,
                    );
                }
                Ok(_) => thread::sleep(poll_interval),
                Err(error) => {
                    return write_runtime_error(command, RuntimeError::backend(error), output);
                }
            }
        }
    }

    let mut writer = io::stdout().lock();
    let mut sequence = 1_u64;
    if let Some(receipt) = receipt
        && emit_stream_value(
            &mut writer,
            output,
            command,
            &mut sequence,
            serialize_value(&receipt).unwrap_or(Value::Null),
        )
        .is_err()
    {
        return 1;
    }
    let mut cursor = None;
    loop {
        let events = match client.events(&EventQuery {
            job_series_id: Some(job_series_id.clone()),
            after_sequence: cursor,
            limit: 10_000,
        }) {
            Ok(events) => events,
            Err(error) => {
                return finish_stream_error(
                    &mut writer,
                    output,
                    command,
                    &mut sequence,
                    RuntimeError::backend(error),
                );
            }
        };
        for event in events {
            cursor = Some(event.sequence);
            if emit_job_event(&mut writer, output, command, &mut sequence, &event).is_err() {
                return 1;
            }
        }
        let snapshot = match client.status(job_series_id) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return finish_stream_error(
                    &mut writer,
                    output,
                    command,
                    &mut sequence,
                    RuntimeError::backend(error),
                );
            }
        };
        if snapshot.state.is_terminal() {
            let exit_code = snapshot.state.wait_exit_code().unwrap_or(1);
            if emit_stream_value(
                &mut writer,
                output,
                command,
                &mut sequence,
                serialize_value(&snapshot).unwrap_or(Value::Null),
            )
            .is_err()
                || emit_stream_summary(
                    &mut writer,
                    output,
                    command,
                    sequence,
                    true,
                    Some(snapshot.state.run_success()),
                )
                .is_err()
            {
                return 1;
            }
            return exit_code;
        }
        thread::sleep(poll_interval);
    }
}

fn stream_events(
    client: LocalJobClient,
    job_series_id: Option<JobSeriesId>,
    since: Option<u64>,
    follow: bool,
    output: OutputMode,
    poll_interval: Duration,
) -> i32 {
    if output == OutputMode::Json && !follow {
        return match client.events(&EventQuery {
            job_series_id,
            after_sequence: since,
            limit: 10_000,
        }) {
            Ok(events) => crate::write_outcome(
                &AppOutcome::ok(
                    "job events",
                    serialize_value(&events).unwrap_or(Value::Null),
                ),
                output,
            ),
            Err(error) => write_runtime_error("job events", RuntimeError::backend(error), output),
        };
    }

    let mut writer = io::stdout().lock();
    let mut sequence = 1_u64;
    let mut cursor = since;
    loop {
        let events = match client.events(&EventQuery {
            job_series_id: job_series_id.clone(),
            after_sequence: cursor,
            limit: 10_000,
        }) {
            Ok(events) => events,
            Err(error) => {
                return finish_stream_error(
                    &mut writer,
                    output,
                    "job events",
                    &mut sequence,
                    RuntimeError::backend(error),
                );
            }
        };
        let saturated = events.len() == 10_000;
        for event in events {
            cursor = Some(event.sequence);
            if emit_job_event(&mut writer, output, "job events", &mut sequence, &event).is_err() {
                return 1;
            }
        }
        if !follow {
            return if emit_stream_summary(&mut writer, output, "job events", sequence, true, None)
                .is_ok()
            {
                0
            } else {
                1
            };
        }
        if let Some(id) = &job_series_id {
            match client.status(id) {
                Ok(snapshot) if snapshot.state.is_terminal() && !saturated => {
                    return if emit_stream_summary(
                        &mut writer,
                        output,
                        "job events",
                        sequence,
                        true,
                        Some(snapshot.state.run_success()),
                    )
                    .is_ok()
                    {
                        0
                    } else {
                        1
                    };
                }
                Ok(_) => {}
                Err(error) => {
                    return finish_stream_error(
                        &mut writer,
                        output,
                        "job events",
                        &mut sequence,
                        RuntimeError::backend(error),
                    );
                }
            }
        }
        if !saturated {
            thread::sleep(poll_interval);
        }
    }
}

fn emit_job_event(
    writer: &mut impl Write,
    output: OutputMode,
    command: &str,
    sequence: &mut u64,
    event: &JobEvent,
) -> io::Result<()> {
    match output {
        OutputMode::Human => {
            let kind = serde_json::to_value(event.kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "event".into());
            let message = event.message.as_deref().unwrap_or("");
            writeln!(
                writer,
                "[{}] {:?} {kind} {message}",
                event.sequence, event.state
            )
        }
        OutputMode::Jsonl => emit_stream_value(
            writer,
            output,
            command,
            sequence,
            serialize_value(event).unwrap_or(Value::Null),
        ),
        OutputMode::Json => Ok(()),
    }
}

fn emit_stream_value(
    writer: &mut impl Write,
    output: OutputMode,
    command: &str,
    sequence: &mut u64,
    value: Value,
) -> io::Result<()> {
    match output {
        OutputMode::Human => writeln!(
            writer,
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".into())
        ),
        OutputMode::Jsonl => {
            let line = crate::app::render_stream_data(command, *sequence, value)
                .map_err(io::Error::other)?;
            *sequence = sequence.saturating_add(1);
            writeln!(writer, "{line}")
        }
        OutputMode::Json => Ok(()),
    }
}

fn emit_stream_summary(
    writer: &mut impl Write,
    output: OutputMode,
    command: &str,
    sequence: u64,
    ok: bool,
    run_success: Option<bool>,
) -> io::Result<()> {
    match output {
        OutputMode::Human => Ok(()),
        OutputMode::Jsonl => writeln!(
            writer,
            "{}",
            crate::app::render_stream_summary(command, sequence, ok, run_success)
                .map_err(io::Error::other)?
        ),
        OutputMode::Json => Ok(()),
    }
}

fn finish_stream_error(
    writer: &mut impl Write,
    output: OutputMode,
    command: &str,
    sequence: &mut u64,
    error: RuntimeError,
) -> i32 {
    let diagnostic = Diagnostic::error(error.code, error.message).with_hint(error.hint);
    let result = match output {
        OutputMode::Human => writeln!(writer, "{}: {}", diagnostic.code(), diagnostic.message()),
        OutputMode::Jsonl => {
            let line = crate::app::render_stream_diagnostic(command, *sequence, &diagnostic)
                .map_err(io::Error::other);
            match line {
                Ok(line) => {
                    let first = writeln!(writer, "{line}");
                    *sequence = sequence.saturating_add(1);
                    first.and_then(|()| {
                        emit_stream_summary(writer, output, command, *sequence, false, None)
                    })
                }
                Err(error) => Err(error),
            }
        }
        OutputMode::Json => Ok(()),
    };
    if result.is_ok() { error.exit_code } else { 1 }
}

fn write_runtime_error(command: &str, error: RuntimeError, output: OutputMode) -> i32 {
    crate::write_outcome(
        &AppOutcome::error(command, &error.code, error.message, error.hint)
            .with_exit_code(error.exit_code),
        output,
    )
}

fn serialize_value(value: &impl Serialize) -> Result<Value, RuntimeError> {
    serde_json::to_value(value).map_err(|error| {
        RuntimeError::internal(
            "runtime.serialization",
            format!("serialize result: {error}"),
        )
    })
}

fn runtime_settings(config_path: Option<&Path>) -> Result<RuntimeSettings, RuntimeError> {
    crate::configuration::runtime_settings(config_path).map_err(|error| RuntimeError {
        code: error.code.into(),
        message: error.message,
        hint: "inspect the selected machine configuration".into(),
        exit_code: 1,
    })
}

fn resolve_submission(command: &StagedRunCommand) -> Result<SubmitRequest, RuntimeError> {
    match &command.input {
        RunInput::Project { project, profile } => {
            let resolved =
                crate::project::resolve_runtime_run(project, profile).map_err(|error| {
                    RuntimeError {
                        code: error.code.into(),
                        message: error.message,
                        hint: "inspect project validate, data-plan, and finalize".into(),
                        exit_code: 1,
                    }
                })?;
            Ok(SubmitRequest {
                input: JobRunInput::Project {
                    project_root: resolved.project_root,
                    profile_name: resolved.profile_name,
                },
                resources: resources_from_execution(&resolved.run_profile.execution)?,
            })
        }
        RunInput::Direct { case, run_profile } => {
            let resolved = resolve_direct(case, run_profile)?;
            Ok(SubmitRequest {
                input: JobRunInput::Direct {
                    case_path: resolved.run_profile.case_path.clone(),
                    run_profile_path: resolved.run_profile_path,
                },
                resources: resources_from_execution(&resolved.run_profile.execution)?,
            })
        }
    }
}

struct DirectResolution {
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    run_profile_path: PathBuf,
}

fn resolve_direct(
    case_path: &Path,
    run_profile_path: &Path,
) -> Result<DirectResolution, RuntimeError> {
    let case_path = canonical_file(case_path, "run.case_invalid")?;
    let run_profile_path = canonical_file(run_profile_path, "run.profile_invalid")?;
    let profile_root = run_profile_path.parent().ok_or_else(|| {
        RuntimeError::product("run.profile_invalid", "RunProfile path has no parent")
    })?;
    let run_profile =
        expand_run_profile_file(&run_profile_path, &LocalRefResolver::new(profile_root))
            .map_err(|error| RuntimeError::product("run.profile_invalid", error.to_string()))?;
    if run_profile.case_path != case_path {
        return Err(RuntimeError::product(
            "run.profile_case_mismatch",
            "explicit --case must exactly match the resolved RunProfile case_path",
        ));
    }
    let case_root = case_path
        .parent()
        .ok_or_else(|| RuntimeError::product("run.case_invalid", "Case path has no parent"))?;
    let case = expand_case_file(&case_path, &LocalRefResolver::new(case_root))
        .map_err(|error| RuntimeError::product("run.case_invalid", error.to_string()))?;
    if IntentValidator::validate(&case, ValidationIntent::Simulation).has_errors() {
        return Err(RuntimeError::product(
            "run.case_invalid",
            "resolved Case does not satisfy simulation intent",
        ));
    }
    Ok(DirectResolution {
        case,
        run_profile,
        run_profile_path,
    })
}

fn canonical_file(path: &Path, code: &'static str) -> Result<PathBuf, RuntimeError> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| RuntimeError::product(code, format!("{}: {error}", path.display())))?;
    if !canonical.is_file() {
        return Err(RuntimeError::product(
            code,
            format!("{} is not a regular file", canonical.display()),
        ));
    }
    Ok(canonical)
}

fn resources_from_execution(execution: &ExecutionSpec) -> Result<ResourceRequest, RuntimeError> {
    let worker_threads = u32::try_from(execution.worker_threads).map_err(|_| {
        RuntimeError::product("run.resources_invalid", "worker thread count exceeds u32")
    })?;
    let memory_mib = execution
        .memory_budget_bytes
        .checked_add(MIB - 1)
        .ok_or_else(|| RuntimeError::product("run.resources_invalid", "memory budget overflow"))?
        / MIB;
    let resources = ResourceRequest {
        cpu_slots: worker_threads,
        memory_mib: memory_mib.max(1),
        worker_threads,
    };
    resources
        .validate()
        .map_err(|error| RuntimeError::product(error.code(), "invalid RunProfile resources"))?;
    Ok(resources)
}

fn job_series_id(value: &str) -> Result<JobSeriesId, RuntimeError> {
    validate_uuid_v7(value, "job.invalid_identity")?;
    Ok(JobSeriesId(value.into()))
}

fn validate_uuid_v7(value: &str, code: &'static str) -> Result<(), RuntimeError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| RuntimeError::product(code, "identity is not a UUID"))?;
    if parsed.get_version_num() != 7 {
        return Err(RuntimeError::product(code, "identity is not UUID-v7"));
    }
    Ok(())
}

fn daemon_client(settings: &RuntimeSettings) -> Result<LocalJobClient, RuntimeError> {
    let client = LocalJobClient::new(&settings.endpoint, DAEMON_CONNECT_TIMEOUT);
    if client.ping().is_ok() {
        return Ok(client);
    }
    start_daemon(settings)?;
    let started = Instant::now();
    loop {
        let client = LocalJobClient::new(&settings.endpoint, DAEMON_CONNECT_TIMEOUT);
        if client.ping().is_ok() {
            return Ok(client);
        }
        if started.elapsed() >= DAEMON_START_TIMEOUT {
            return Err(RuntimeError::product(
                "daemon.start_timeout",
                "local daemon did not become reachable within 10 seconds",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn start_daemon(settings: &RuntimeSettings) -> Result<(), RuntimeError> {
    let executable = std::env::current_exe().map_err(|error| {
        RuntimeError::product(
            "daemon.launch_failed",
            format!("locate executable: {error}"),
        )
    })?;
    let mut command = ProcessCommand::new(executable);
    command
        .arg("__daemon")
        .arg("--config")
        .arg(&settings.config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_background_process(&mut command);
    let _inheritance_guard = suppress_standard_handle_inheritance().map_err(|error| {
        RuntimeError::product(
            "daemon.launch_failed",
            format!("isolate daemon standard handles: {error}"),
        )
    })?;
    command.spawn().map_err(|error| {
        RuntimeError::product("daemon.launch_failed", format!("start daemon: {error}"))
    })?;
    Ok(())
}

#[cfg(windows)]
fn configure_background_process(command: &mut ProcessCommand) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};

    command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

#[cfg(unix)]
fn configure_background_process(command: &mut ProcessCommand) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

fn recover_daemon_workers(
    catalog: &mut LocalJobCatalog,
    daemon_instance_id: &str,
    at: Timestamp,
) -> Result<(), RuntimeError> {
    let mut decisions = BTreeMap::new();
    for (snapshot, lease) in catalog
        .recovery_candidates()
        .map_err(RuntimeError::backend)?
    {
        let worker_confirmed_live = lease.as_ref().is_some_and(|worker| {
            process_identity_matches(&ProcessIdentity {
                pid: worker.worker_pid,
                start_token: worker.worker_start_token.clone(),
            })
            // A process-probe failure is not proof of death. Preserve the
            // active attempt and retry on the next daemon tick.
            .unwrap_or(true)
        });
        let decision = if worker_confirmed_live {
            WorkerRecoveryDisposition::Reattach
        } else {
            recovery_manifest_disposition(&snapshot, at)?
        };
        decisions.insert(snapshot.run_id, decision);
    }
    catalog
        .recover_workers_with(daemon_instance_id, at, |run_id, _| {
            decisions
                .remove(run_id)
                .unwrap_or(WorkerRecoveryDisposition::Interrupt)
        })
        .map_err(RuntimeError::backend)?;
    Ok(())
}

fn recovery_manifest_disposition(
    snapshot: &JobSnapshot,
    at: Timestamp,
) -> Result<WorkerRecoveryDisposition, RuntimeError> {
    let Some(output_directory) = snapshot.output_directory.as_ref() else {
        return Ok(WorkerRecoveryDisposition::Interrupt);
    };
    let manifest_path = output_directory.join("run-manifest.json");
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkerRecoveryDisposition::Interrupt);
        }
        Err(error) => {
            return Err(RuntimeError::product(
                "daemon.recovery_manifest_read_failed",
                format!("read {}: {error}", manifest_path.display()),
            ));
        }
    };
    let mut manifest = match serde_json::from_slice::<RunManifest>(&bytes) {
        Ok(manifest) => manifest,
        // A corrupt manifest is forensic evidence. Do not replace it with a
        // syntactically valid document that would hide the original failure.
        Err(_) => return Ok(WorkerRecoveryDisposition::Interrupt),
    };
    if manifest.validate().is_err()
        || manifest.job_series_id != snapshot.job_series_id
        || manifest.run_id != snapshot.run_id
        || manifest.attempt != snapshot.attempt
    {
        return Ok(WorkerRecoveryDisposition::Interrupt);
    }
    let terminal = match manifest.status {
        RunLifecycleStatus::Running => {
            manifest.status = RunLifecycleStatus::Interrupted;
            manifest.finished_at = Some(at);
            manifest.provenance = None;
            manifest.failure = Some(RunFailure {
                code: "run.interrupted.worker_lost".into(),
                message: "worker process disappeared before catalog terminalization".into(),
            });
            let mut store = AtomicRunManifestStore::new(&manifest_path);
            store.persist(&manifest).map_err(|error| {
                RuntimeError::product(
                    "daemon.recovery_manifest_persist_failed",
                    format!("persist {}: {error}", manifest_path.display()),
                )
            })?;
            JobState::Interrupted
        }
        RunLifecycleStatus::Complete => JobState::Complete,
        RunLifecycleStatus::CompletedWithParticleErrors => JobState::CompletedWithParticleErrors,
        RunLifecycleStatus::Failed => JobState::Failed,
        RunLifecycleStatus::Cancelled => JobState::Cancelled,
        RunLifecycleStatus::Interrupted => JobState::Interrupted,
    };
    Ok(WorkerRecoveryDisposition::ReconcileTerminal(terminal))
}

fn run_daemon(config_path: &Path) -> Result<(), RuntimeError> {
    let settings = runtime_settings(Some(config_path))?;
    let capacity = SchedulerCapacity {
        cpu_slots: settings.cpu_slots,
        memory_mib: settings.schedulable_memory_mib,
    };
    capacity
        .validate()
        .map_err(|error| RuntimeError::product("daemon.invalid_capacity", format!("{error:?}")))?;
    let identity = current_process_identity()
        .map_err(|error| RuntimeError::product("daemon.identity_failed", error.to_string()))?;
    let daemon_instance_id = Uuid::now_v7().to_string();
    let mut scheduler_catalog =
        LocalJobCatalog::open(&settings.catalog_path).map_err(RuntimeError::backend)?;
    let replace = match scheduler_catalog
        .daemon_lease()
        .map_err(RuntimeError::backend)?
    {
        Some(current) => {
            let current_identity = ProcessIdentity {
                pid: current.daemon_pid,
                start_token: current.daemon_start_token.clone(),
            };
            if process_identity_matches(&current_identity).map_err(|error| {
                RuntimeError::product("daemon.owner_probe_failed", error.to_string())
            })? {
                return Ok(());
            }
            Some(current.daemon_instance_id)
        }
        None => None,
    };
    let lease = DaemonLease {
        daemon_instance_id: daemon_instance_id.clone(),
        daemon_pid: identity.pid,
        daemon_start_token: identity.start_token.clone(),
        heartbeat_at: timestamp_now()?,
    };
    scheduler_catalog
        .claim_daemon(&lease, replace.as_deref())
        .map_err(RuntimeError::backend)?;
    let setup = (|| {
        recover_daemon_workers(
            &mut scheduler_catalog,
            &daemon_instance_id,
            timestamp_now()?,
        )?;
        prepare_endpoint(&settings.endpoint)?;
        let server = LocalJobServer::bind(&settings.endpoint).map_err(|error| {
            RuntimeError::product("daemon.bind_failed", format!("bind local IPC: {error}"))
        })?;
        let server_catalog =
            LocalJobCatalog::open(&settings.catalog_path).map_err(RuntimeError::backend)?;
        let backend = DaemonControlBackend::new(server_catalog, capacity, HostTerminator)
            .map_err(RuntimeError::backend)?;
        Ok::<_, RuntimeError>((server, backend))
    })();
    let (mut server, mut backend) = match setup {
        Ok(value) => value,
        Err(error) => {
            scheduler_catalog
                .release_daemon(&daemon_instance_id, &identity.start_token)
                .map_err(RuntimeError::backend)?;
            return Err(error);
        }
    };
    let shutdown_token = Uuid::now_v7().to_string();
    let server_shutdown_token = shutdown_token.clone();
    let daemon_started = Instant::now();
    let activity_ms = Arc::new(AtomicU64::new(0));
    let server_activity = activity_ms.clone();
    let server_failed = Arc::new(AtomicBool::new(false));
    let server_failed_flag = server_failed.clone();
    let server_error = Arc::new(Mutex::new(None::<String>));
    let server_error_slot = server_error.clone();
    let server_thread = thread::spawn(move || {
        let mut consecutive_errors = 0_u8;
        loop {
            match server.serve_one_controlled(&mut backend, Some(&server_shutdown_token)) {
                Ok(true) => {
                    consecutive_errors = 0;
                    server_activity.store(
                        u64::try_from(daemon_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        Ordering::Release,
                    );
                }
                Ok(false) => break,
                Err(error) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    if consecutive_errors >= 32 {
                        if let Ok(mut slot) = server_error_slot.lock() {
                            *slot = Some(error.to_string());
                        }
                        server_failed_flag.store(true, Ordering::Release);
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            }
        }
    });

    let loop_result = (|| -> Result<(), RuntimeError> {
        let executable = std::env::current_exe().map_err(|error| {
            RuntimeError::product(
                "daemon.launch_failed",
                format!("locate executable: {error}"),
            )
        })?;
        let log_root = settings
            .catalog_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("worker-logs");
        let mut processes =
            HostWorkerProcesses::new(executable, settings.config_path.clone(), log_root);
        let tick = Duration::from_millis(settings.sample_interval_ms.clamp(100, 1_000));
        let idle_timeout = (settings.idle_shutdown_seconds != 0)
            .then(|| Duration::from_secs(settings.idle_shutdown_seconds));
        loop {
            if server_failed.load(Ordering::Acquire) {
                let message = server_error
                    .lock()
                    .ok()
                    .and_then(|value| value.clone())
                    .unwrap_or_else(|| "local IPC server stopped".into());
                return Err(RuntimeError::product("daemon.ipc_failed", message));
            }
            processes.reap_finished();
            let at = timestamp_now()?;
            scheduler_catalog
                .heartbeat_daemon(&daemon_instance_id, &identity.start_token, at)
                .map_err(RuntimeError::backend)?;
            recover_daemon_workers(&mut scheduler_catalog, &daemon_instance_id, at)?;
            let reserve_bytes = settings.memory_reserve_mib.saturating_mul(MIB);
            let external_pressure = reserve_bytes != 0
                && available_memory_bytes()
                    .map(|available| available < reserve_bytes)
                    .unwrap_or(true);
            if external_pressure {
                scheduler_catalog
                    .warn_queued_external_memory_pressure(at)
                    .map_err(RuntimeError::backend)?;
            }
            dispatch_once(
                &mut scheduler_catalog,
                capacity,
                &daemon_instance_id,
                external_pressure,
                at,
                &mut processes,
            )
            .map_err(RuntimeError::backend)?;

            let idle = scheduler_catalog
                .queued_attempts()
                .map_err(RuntimeError::backend)?
                .is_empty()
                && scheduler_catalog
                    .active_resource_usage()
                    .map_err(RuntimeError::backend)?
                    .cpu_slots
                    == 0;
            if idle
                && idle_timeout.is_some_and(|timeout| {
                    let last_activity = Duration::from_millis(activity_ms.load(Ordering::Acquire));
                    daemon_started.elapsed().saturating_sub(last_activity) >= timeout
                })
            {
                let client = LocalJobClient::new(&settings.endpoint, DAEMON_CONNECT_TIMEOUT);
                client
                    .shutdown(shutdown_token.clone())
                    .map_err(RuntimeError::backend)?;
                return Ok(());
            }
            thread::sleep(tick);
        }
    })();

    if !server_thread.is_finished() {
        let client = LocalJobClient::new(&settings.endpoint, DAEMON_CONNECT_TIMEOUT);
        let _ = client.shutdown(shutdown_token);
    }
    let join_result = server_thread.join().map_err(|_| {
        RuntimeError::product("daemon.ipc_failed", "local IPC server thread panicked")
    });
    let release_result = scheduler_catalog
        .release_daemon(&daemon_instance_id, &identity.start_token)
        .map_err(RuntimeError::backend);
    loop_result?;
    join_result?;
    release_result?;
    Ok(())
}

#[cfg(unix)]
fn prepare_endpoint(endpoint: &str) -> Result<(), RuntimeError> {
    use std::os::unix::fs::FileTypeExt;

    let path = Path::new(endpoint);
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|error| {
                RuntimeError::product(
                    "daemon.bind_failed",
                    format!("remove stale socket: {error}"),
                )
            })
        }
        Ok(_) => Err(RuntimeError::product(
            "daemon.bind_failed",
            "refusing to replace a non-socket object at the local IPC endpoint",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RuntimeError::product(
            "daemon.bind_failed",
            format!("inspect local IPC endpoint: {error}"),
        )),
    }
}

#[cfg(windows)]
fn prepare_endpoint(_endpoint: &str) -> Result<(), RuntimeError> {
    Ok(())
}

struct HostWorkerProcesses {
    executable: PathBuf,
    config_path: PathBuf,
    log_root: PathBuf,
    children: BTreeMap<RunId, Child>,
}

impl HostWorkerProcesses {
    fn new(executable: PathBuf, config_path: PathBuf, log_root: PathBuf) -> Self {
        Self {
            executable,
            config_path,
            log_root,
            children: BTreeMap::new(),
        }
    }

    fn reap_finished(&mut self) {
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
struct HostTerminator;

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

fn run_worker(config_path: &Path, run_id: RunId) -> Result<(), RuntimeError> {
    let settings = runtime_settings(Some(config_path))?;
    let identity = current_process_identity()
        .map_err(|error| RuntimeError::product("worker.identity_failed", error.to_string()))?;
    let mut catalog =
        LocalJobCatalog::open(&settings.catalog_path).map_err(RuntimeError::backend)?;
    wait_for_worker_lease(&catalog, &run_id, &identity)?;
    let snapshot = catalog
        .snapshot_for_run(&run_id)
        .map_err(RuntimeError::backend)?;
    if snapshot.state.is_terminal() {
        return Ok(());
    }
    if finish_pre_manifest_cancel_if_requested(&mut catalog, &run_id, &identity.start_token)? {
        return Ok(());
    }

    // Setup failures are durable job outcomes, not worker-process failures.
    macro_rules! setup_or_finish {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    finish_worker_failure(
                        &mut catalog,
                        &run_id,
                        &identity.start_token,
                        &error.code,
                        &error.message,
                    )?;
                    return Ok(());
                }
            }
        };
    }

    let resolved = setup_or_finish!(resolve_worker_input(&snapshot.input));
    let actual_resources =
        setup_or_finish!(resources_from_execution(&resolved.run_profile.execution));
    if actual_resources != snapshot.resources {
        finish_worker_failure(
            &mut catalog,
            &run_id,
            &identity.start_token,
            "worker.input_changed",
            "RunProfile resources changed after durable submission",
        )?;
        return Ok(());
    }
    if finish_pre_manifest_cancel_if_requested(&mut catalog, &run_id, &identity.start_token)? {
        return Ok(());
    }
    let output_directory = setup_or_finish!(
        run_directory_path(
            &resolved.run_profile.output_root,
            &resolved.case.metadata.name,
            &snapshot.run_id,
        )
        .map_err(|error| RuntimeError::product(error.code(), format!("{error:?}")))
    );
    retry_catalog_write(|| catalog.set_output_directory(&run_id, &output_directory))?;
    let control_before_build = catalog
        .worker_control(&run_id, &identity.start_token)
        .map_err(RuntimeError::backend)?;
    if control_before_build == trajecta_job::catalog::WorkerControl::Continue {
        mark_worker_running_reliably(&mut catalog, &run_id, &identity.start_token)?;
    }
    let mut runner = setup_or_finish!(
        build_runner_for_attempt(
            resolved.case,
            resolved.run_profile,
            RunnerAttemptIdentity {
                job_series_id: snapshot.job_series_id.clone(),
                run_id: snapshot.run_id.clone(),
                attempt: snapshot.attempt,
            },
        )
        .map_err(|error| RuntimeError::product(error.code(), format!("{error:?}")))
    );
    let mut control = setup_or_finish!(
        CatalogRunnerControl::open(
            &settings.catalog_path,
            run_id.clone(),
            identity.start_token.clone(),
            poll_interval(&settings),
        )
        .map_err(RuntimeError::backend)
    );
    let (terminal, code, message) = match runner.run_with_control(&mut control) {
        Ok(RunOutcome::Complete) => (
            JobState::Complete,
            "worker.complete",
            "simulation completed without abnormal particle termination".into(),
        ),
        Ok(RunOutcome::CompletedWithParticleErrors) => (
            JobState::CompletedWithParticleErrors,
            "worker.completed_with_particle_errors",
            "simulation completed with abnormal particle termination".into(),
        ),
        Ok(RunOutcome::Cancelled) => (
            JobState::Cancelled,
            "worker.cancelled",
            "simulation finalized at a safe macro-step cancellation boundary".into(),
        ),
        Err(error) => (JobState::Failed, error.code(), format!("{error:?}")),
    };
    finish_worker_reliably(
        &mut catalog,
        &run_id,
        &identity.start_token,
        terminal,
        TransitionDiagnostic {
            code: Some(code.into()),
            message: Some(message),
        },
    )?;
    Ok(())
}

struct WorkerResolution {
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
}

fn resolve_worker_input(input: &JobRunInput) -> Result<WorkerResolution, RuntimeError> {
    match input {
        JobRunInput::Project {
            project_root,
            profile_name,
        } => {
            let resolved = crate::project::resolve_runtime_run(project_root, profile_name)
                .map_err(|error| RuntimeError::product(error.code, error.message))?;
            Ok(WorkerResolution {
                case: resolved.case,
                run_profile: resolved.run_profile,
            })
        }
        JobRunInput::Direct {
            case_path,
            run_profile_path,
        } => {
            let resolved = resolve_direct(case_path, run_profile_path)?;
            Ok(WorkerResolution {
                case: resolved.case,
                run_profile: resolved.run_profile,
            })
        }
    }
}

fn wait_for_worker_lease(
    catalog: &LocalJobCatalog,
    run_id: &RunId,
    identity: &ProcessIdentity,
) -> Result<(), RuntimeError> {
    let started = Instant::now();
    loop {
        match catalog
            .worker_lease(run_id)
            .map_err(RuntimeError::backend)?
        {
            Some(lease)
                if lease.worker_pid == identity.pid
                    && lease.worker_start_token == identity.start_token =>
            {
                return Ok(());
            }
            Some(_) => {
                return Err(RuntimeError::product(
                    "worker.lease_mismatch",
                    "catalog worker lease does not match this process identity",
                ));
            }
            None => {
                let snapshot = catalog
                    .snapshot_for_run(run_id)
                    .map_err(RuntimeError::backend)?;
                if snapshot.state.is_terminal() {
                    return Ok(());
                }
                if started.elapsed() >= WORKER_ATTACH_TIMEOUT {
                    return Err(RuntimeError::product(
                        "worker.lease_timeout",
                        "daemon did not attach the worker lease within 10 seconds",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn finish_worker_failure(
    catalog: &mut LocalJobCatalog,
    run_id: &RunId,
    worker_start_token: &str,
    code: &str,
    message: &str,
) -> Result<(), RuntimeError> {
    finish_worker_reliably(
        catalog,
        run_id,
        worker_start_token,
        JobState::Failed,
        TransitionDiagnostic {
            code: Some(code.into()),
            message: Some(message.into()),
        },
    )
}

fn finish_pre_manifest_cancel_if_requested(
    catalog: &mut LocalJobCatalog,
    run_id: &RunId,
    worker_start_token: &str,
) -> Result<bool, RuntimeError> {
    match catalog
        .worker_control(run_id, worker_start_token)
        .map_err(RuntimeError::backend)?
    {
        trajecta_job::catalog::WorkerControl::Continue => Ok(false),
        trajecta_job::catalog::WorkerControl::SafeCancel => {
            finish_worker_reliably(
                catalog,
                run_id,
                worker_start_token,
                JobState::Cancelled,
                TransitionDiagnostic {
                    code: Some("worker.cancelled_before_manifest".into()),
                    message: Some("safe cancellation completed before run artifacts".into()),
                },
            )?;
            Ok(true)
        }
        trajecta_job::catalog::WorkerControl::ForceStop => Err(RuntimeError::product(
            "worker.force_stop_pending",
            "force-stop reached cooperative worker setup before process termination",
        )),
    }
}

fn mark_worker_running_reliably(
    catalog: &mut LocalJobCatalog,
    run_id: &RunId,
    worker_start_token: &str,
) -> Result<(), RuntimeError> {
    let started = Instant::now();
    loop {
        match catalog.mark_worker_running(run_id, worker_start_token, timestamp_now()?) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if let Ok(snapshot) = catalog.snapshot_for_run(run_id) {
                    if matches!(snapshot.state, JobState::Running | JobState::Cancelling) {
                        return Ok(());
                    }
                }
                if !transient_catalog_lock(&error) || started.elapsed() >= Duration::from_secs(10) {
                    return Err(RuntimeError::backend(error));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn finish_worker_reliably(
    catalog: &mut LocalJobCatalog,
    run_id: &RunId,
    worker_start_token: &str,
    terminal: JobState,
    diagnostic: TransitionDiagnostic,
) -> Result<(), RuntimeError> {
    let started = Instant::now();
    loop {
        match catalog.finish_worker(
            run_id,
            worker_start_token,
            terminal,
            timestamp_now()?,
            diagnostic.clone(),
        ) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if let Ok(snapshot) = catalog.snapshot_for_run(run_id) {
                    if snapshot.state == terminal
                        && catalog.worker_lease(run_id).ok().flatten().is_none()
                    {
                        return Ok(());
                    }
                }
                if !transient_catalog_lock(&error) || started.elapsed() >= Duration::from_secs(10) {
                    return Err(RuntimeError::backend(error));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn retry_catalog_write<T>(
    mut operation: impl FnMut() -> Result<T, JobBackendError>,
) -> Result<T, RuntimeError> {
    let started = Instant::now();
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error)
                if transient_catalog_lock(&error)
                    && started.elapsed() < Duration::from_secs(10) =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(RuntimeError::backend(error)),
        }
    }
}

fn transient_catalog_lock(error: &JobBackendError) -> bool {
    let JobBackendError::Storage(message) = error else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    message.contains("locked") || message.contains("busy")
}

fn transient_backend(error: &JobBackendError) -> bool {
    transient_catalog_lock(error) || matches!(error, JobBackendError::Unavailable(_))
}

fn poll_interval(settings: &RuntimeSettings) -> Duration {
    Duration::from_millis(settings.sample_interval_ms.max(100))
}

fn timestamp_now() -> Result<Timestamp, RuntimeError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RuntimeError::internal("runtime.clock", error.to_string()))?;
    let seconds = i64::try_from(duration.as_secs())
        .map_err(|error| RuntimeError::internal("runtime.clock", error.to_string()))?;
    Timestamp::new(seconds, duration.subsec_nanos())
        .map_err(|error| RuntimeError::internal("runtime.clock", format!("{error:?}")))
}

#[derive(Clone, Debug)]
struct RuntimeError {
    code: String,
    message: String,
    hint: String,
    exit_code: i32,
}

impl RuntimeError {
    fn product(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: "inspect daemon, job status, and persistent events".into(),
            exit_code: 1,
        }
    }

    fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: "preserve runtime catalog and worker logs for diagnosis".into(),
            exit_code: 1,
        }
    }

    fn backend(error: JobBackendError) -> Self {
        Self::product(error.code(), error.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn recovery_manifest() -> RunManifest {
        let manifest = serde_json::from_value::<RunManifest>(serde_json::json!({
            "schema_version": "trajecta.run-manifest/v1",
            "job_series_id": "018f0000-0000-7000-8000-000000000001",
            "attempt": 1,
            "run_id": "018f0000-0000-7000-8000-000000000002",
            "case_name": "recovery",
            "status": "running",
            "started_at": {"seconds_since_unix_epoch": 1, "nanosecond": 0},
            "software": {"crate_versions": {"trajecta-core": "0.0.0"}},
            "inputs": {
                "case_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "run_profile_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
                "dataset_lock_sha256": {},
                "dataset_profile_sha256": {},
                "dataset_content_sha256": {}
            },
            "execution": {
                "worker_threads": 1,
                "memory_budget_bytes": 1,
                "executor": "rayon",
                "reader_backends": {},
                "io_counters": {}
            },
            "numerical": {
                "random_seed": 1,
                "integrator": "rk2_spherical/v0",
                "boundary_policies": [],
                "population": "release_driven/v1",
                "particle_state_sink": "particle_state_sqlite/v1",
                "tolerance_registry": "test/v1",
                "tolerances": {"test": 0.0},
                "deterministic": true
            },
            "geometries": [],
            "effective_outputs": [{
                "product": "particle_state/v1",
                "schedule": {"mode": "endpoints"},
                "sink": {"model": "particle_state_sqlite/v1"}
            }],
            "sqlite": {
                "schema_version": 1,
                "relative_path": "particles.sqlite",
                "journal_mode": "WAL",
                "synchronous": "NORMAL",
                "row_counts": {}
            },
            "terminations": {"normal_count": 0, "abnormal_count": 0, "by_reason": {}},
            "mass_ledger": []
        }))
        .unwrap();
        manifest.validate().unwrap();
        manifest
    }

    fn recovery_snapshot(output_directory: PathBuf) -> JobSnapshot {
        JobSnapshot {
            schema_version: "trajecta.job-record/v1".into(),
            job_series_id: JobSeriesId("018f0000-0000-7000-8000-000000000001".into()),
            run_id: RunId("018f0000-0000-7000-8000-000000000002".into()),
            attempt: 1,
            state: JobState::Running,
            input: JobRunInput::Project {
                project_root: output_directory.clone(),
                profile_name: "default".into(),
            },
            resources: ResourceRequest {
                cpu_slots: 1,
                memory_mib: 1,
                worker_threads: 1,
            },
            created_at: Timestamp::UNIX_EPOCH,
            started_at: Some(Timestamp::UNIX_EPOCH),
            finished_at: None,
            queue_position: None,
            output_directory: Some(output_directory),
        }
    }

    #[test]
    fn resource_rounding_is_explicit_and_never_zero() {
        let execution = ExecutionSpec {
            worker_threads: 3,
            memory_budget_bytes: MIB + 1,
            executor: "rayon".into(),
            meteorology_reader: Default::default(),
        };
        assert_eq!(
            resources_from_execution(&execution).unwrap(),
            ResourceRequest {
                cpu_slots: 3,
                memory_mib: 2,
                worker_threads: 3,
            }
        );
    }

    #[test]
    fn identity_parser_requires_uuid_v7() {
        assert!(job_series_id("018f0000-0000-7000-8000-000000000001").is_ok());
        assert!(job_series_id("018f0000-0000-4000-8000-000000000001").is_err());
        assert!(job_series_id("not-a-uuid").is_err());
    }

    #[test]
    fn recovery_marks_a_valid_running_manifest_interrupted() {
        let temp = tempfile::tempdir().unwrap();
        let output_directory = temp.path().canonicalize().unwrap();
        let path = output_directory.join("run-manifest.json");
        let mut store = AtomicRunManifestStore::new(&path);
        store.persist(&recovery_manifest()).unwrap();

        assert_eq!(
            recovery_manifest_disposition(
                &recovery_snapshot(output_directory),
                Timestamp::new(2, 0).unwrap(),
            )
            .unwrap(),
            WorkerRecoveryDisposition::ReconcileTerminal(JobState::Interrupted)
        );
        let persisted: RunManifest = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        persisted.validate().unwrap();
        assert_eq!(persisted.status, RunLifecycleStatus::Interrupted);
        assert_eq!(persisted.finished_at, Some(Timestamp::new(2, 0).unwrap()));
        assert_eq!(
            persisted
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("run.interrupted.worker_lost")
        );
        assert!(persisted.provenance.is_none());
    }

    #[test]
    fn recovery_adopts_an_existing_terminal_manifest_without_rewriting_it() {
        let temp = tempfile::tempdir().unwrap();
        let output_directory = temp.path().canonicalize().unwrap();
        let path = output_directory.join("run-manifest.json");
        let mut manifest = recovery_manifest();
        manifest.status = RunLifecycleStatus::Failed;
        manifest.finished_at = Some(Timestamp::new(2, 0).unwrap());
        manifest.failure = Some(RunFailure {
            code: "run.failed".into(),
            message: "already finalized".into(),
        });
        let mut store = AtomicRunManifestStore::new(&path);
        store.persist(&manifest).unwrap();
        let before = fs::read(&path).unwrap();

        assert_eq!(
            recovery_manifest_disposition(
                &recovery_snapshot(output_directory),
                Timestamp::new(3, 0).unwrap(),
            )
            .unwrap(),
            WorkerRecoveryDisposition::ReconcileTerminal(JobState::Failed)
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[cfg(windows)]
    #[test]
    fn daemon_bind_failure_releases_the_claimed_catalog_lease() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        fs::write(
            &config_path,
            r#"schema_version = "trajecta.config/v1"
default_reader_backend = "rust"

[daemon]
idle_shutdown_seconds = 1
local_ipc_only = true

[resources]
cpu_slots = 1
memory_pool_mib = 2
memory_reserve_mib = 1

[monitoring]
sample_interval_ms = 100
maximum_median_overhead_percent = 1.0

[profile_templates]
"#,
        )
        .unwrap();
        let settings = runtime_settings(Some(&config_path)).unwrap();
        let _occupied = LocalJobServer::bind(&settings.endpoint).unwrap();

        let error = run_daemon(&config_path).unwrap_err();
        assert_eq!(error.code, "daemon.bind_failed");
        let catalog = LocalJobCatalog::open(&settings.catalog_path).unwrap();
        assert!(catalog.daemon_lease().unwrap().is_none());
    }
}
