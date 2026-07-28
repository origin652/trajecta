//! Executable checks for M5 job-control type semantics.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::{fs, path::Path};

use trajecta_case::model::time::Timestamp;
use trajecta_core::manifest::{JobSeriesId, RunId, RunLifecycleStatus};
use trajecta_job::model::{
    JOB_EVENT_SCHEMA_ID, JOB_RECORD_SCHEMA_ID, JobEvent, JobEventKind, JobReceipt, JobSnapshot,
    JobState, ResourceRequest, RunInput,
};

fn series() -> JobSeriesId {
    JobSeriesId("018f0000-0000-7000-8000-000000000000".into())
}

fn run() -> RunId {
    RunId("018f0000-0000-7000-8000-000000000001".into())
}

fn resources() -> ResourceRequest {
    ResourceRequest {
        cpu_slots: 4,
        memory_mib: 2048,
        worker_threads: 4,
    }
}

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
}

fn absolute_path(suffix: &str) -> PathBuf {
    std::env::current_dir().unwrap().join(suffix)
}

#[test]
fn run_and_job_terminal_exit_semantics_match() {
    for (run_status, job_status) in [
        (RunLifecycleStatus::Running, JobState::Running),
        (RunLifecycleStatus::Complete, JobState::Complete),
        (
            RunLifecycleStatus::CompletedWithParticleErrors,
            JobState::CompletedWithParticleErrors,
        ),
        (RunLifecycleStatus::Failed, JobState::Failed),
        (RunLifecycleStatus::Cancelled, JobState::Cancelled),
        (RunLifecycleStatus::Interrupted, JobState::Interrupted),
    ] {
        assert_eq!(JobState::from(run_status), job_status);
        assert_eq!(run_status.run_success(), job_status.run_success());
        assert_eq!(run_status.wait_exit_code(), job_status.wait_exit_code());
    }
}

#[test]
fn snapshots_require_terminal_time_and_normalized_paths() {
    let mut snapshot = JobSnapshot {
        schema_version: JOB_RECORD_SCHEMA_ID.into(),
        job_series_id: series(),
        run_id: run(),
        attempt: 1,
        state: JobState::Queued,
        input: RunInput::Project {
            project_root: absolute_path("demo-project"),
            profile_name: "local".into(),
        },
        resources: resources(),
        created_at: Timestamp::UNIX_EPOCH,
        started_at: None,
        finished_at: None,
        queue_position: Some(0),
        output_directory: None,
    };
    snapshot.validate().unwrap();
    snapshot.state = JobState::Complete;
    assert!(snapshot.validate().is_err());
    snapshot.queue_position = None;
    snapshot.started_at = Some(Timestamp::UNIX_EPOCH);
    snapshot.finished_at = Some(Timestamp::UNIX_EPOCH);
    snapshot.output_directory = Some(absolute_path("demo-project/runs/attempt"));
    snapshot.validate().unwrap();
}

#[test]
fn persistent_event_roundtrips_without_consumption_state() {
    let event = JobEvent {
        schema_version: JOB_EVENT_SCHEMA_ID.into(),
        sequence: 1,
        emitted_at: Timestamp::UNIX_EPOCH,
        job_series_id: series(),
        run_id: run(),
        attempt: 1,
        kind: JobEventKind::StateTransition,
        state: JobState::Queued,
        progress: None,
        resource: None,
        code: None,
        message: Some("accepted into durable queue".into()),
        artifact_path: None,
    };
    event.validate().unwrap();
    let encoded = serde_json::to_string(&event).unwrap();
    let decoded: JobEvent = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, event);
    assert!(!encoded.contains("consumed"));
}

#[test]
fn resources_are_explicit_and_cannot_oversubscribe_slots() {
    resources().validate().unwrap();
    assert!(
        ResourceRequest {
            cpu_slots: 2,
            memory_mib: 2048,
            worker_threads: 4,
        }
        .validate()
        .is_err()
    );
}

#[test]
fn receipts_are_queued_and_run_inputs_reject_unknown_fields() {
    let mut receipt = JobReceipt {
        job_series_id: series(),
        run_id: run(),
        attempt: 1,
        state: JobState::Queued,
    };
    receipt.validate().unwrap();
    receipt.state = JobState::Starting;
    assert!(receipt.validate().is_err());

    let with_unknown = serde_json::json!({
        "mode": "project",
        "project_root": absolute_path("demo-project"),
        "profile_name": "local",
        "scientific_override": true
    });
    assert!(serde_json::from_value::<RunInput>(with_unknown).is_err());
}

#[test]
fn compiled_job_states_match_the_frozen_machine_contract() {
    let contract: serde_json::Value = serde_json::from_slice(
        &fs::read(workspace().join("testdata/M5_JOB_CONTRACT.v1.json")).unwrap(),
    )
    .unwrap();
    let compiled = [
        JobState::Queued,
        JobState::Starting,
        JobState::Running,
        JobState::Cancelling,
        JobState::Complete,
        JobState::CompletedWithParticleErrors,
        JobState::Failed,
        JobState::Cancelled,
        JobState::Interrupted,
    ]
    .into_iter()
    .map(|state| {
        serde_json::to_value(state)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    })
    .collect::<Vec<_>>();
    assert_eq!(
        contract["states"],
        serde_json::Value::Array(
            compiled
                .into_iter()
                .map(serde_json::Value::String)
                .collect()
        )
    );
    assert_eq!(contract["automatic_retry"], false);
    assert_eq!(contract["pruning"]["mode"], "dry_run");
    assert_eq!(contract["pruning"]["delete_enabled"], false);
}

#[test]
fn compiled_transitions_match_the_frozen_machine_contract() {
    let contract: serde_json::Value = serde_json::from_slice(
        &fs::read(workspace().join("testdata/M5_JOB_CONTRACT.v1.json")).unwrap(),
    )
    .unwrap();
    let states = [
        JobState::Queued,
        JobState::Starting,
        JobState::Running,
        JobState::Cancelling,
        JobState::Complete,
        JobState::CompletedWithParticleErrors,
        JobState::Failed,
        JobState::Cancelled,
        JobState::Interrupted,
    ];
    for source in states {
        let source_name = serde_json::to_value(source)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        for target in states {
            let target_name = serde_json::to_value(target)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            let contracted = contract["transitions"][&source_name]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some(&target_name));
            assert_eq!(
                source.allows_transition_to(target),
                contracted,
                "transition mismatch: {source_name} -> {target_name}"
            );
        }
    }
}
