//! M5-A2 real-process daemon, worker, and foreground-run contracts.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use trajecta_local_ipc::{ProcessIdentity, force_terminate_process, process_identity_matches};

#[derive(Debug)]
struct LiveAttempt {
    job_series_id: String,
    run_id: String,
    output_directory: PathBuf,
    worker: ProcessIdentity,
    daemon: ProcessIdentity,
    daemon_instance_id: String,
}

fn cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trajecta-cli"))
        .args(arguments)
        .output()
        .unwrap()
}

fn initialize_config(root: &Path, idle_seconds: u64) -> PathBuf {
    let config = root.join("config.toml");
    let init = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "config",
        "init",
    ]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stdout)
    );
    for (key, value) in [
        ("resources.memory_reserve_mib", "0"),
        ("resources.memory_pool_mib", "2048"),
        ("resources.memory_reserve_mib", "256"),
    ] {
        let updated = cli(&[
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "set",
            key,
            value,
        ]);
        assert!(
            updated.status.success(),
            "{}",
            String::from_utf8_lossy(&updated.stdout)
        );
    }
    let idle = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "config",
        "set",
        "daemon.idle_shutdown_seconds",
        &idle_seconds.to_string(),
    ]);
    assert!(
        idle.status.success(),
        "{}",
        String::from_utf8_lossy(&idle.stdout)
    );
    config
}

fn set_config(config: &Path, key: &str, value: &str) {
    let updated = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "config",
        "set",
        key,
        value,
    ]);
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stdout)
    );
}

fn wait_for_daemon_release(catalog: &Path) {
    let started = Instant::now();
    loop {
        let connection = Connection::open(catalog).unwrap();
        let owners = connection
            .query_row("SELECT COUNT(*) FROM daemon_lease", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        if owners == 0 {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(100));
    }
}

fn daemon_instance(catalog: &Path) -> String {
    Connection::open(catalog)
        .unwrap()
        .query_row(
            "SELECT daemon_instance_id FROM daemon_lease WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn artifact_fingerprint(root: &Path) -> BTreeMap<PathBuf, (u64, String)> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                pending.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let digest = hex::encode(Sha256::digest(fs::read(&path).unwrap()));
                files.insert(relative, (metadata.len(), digest));
            }
        }
    }
    files
}

fn successful_json(config: &Path, arguments: &[&str]) -> serde_json::Value {
    let mut command = vec!["--format", "json", "--config", config.to_str().unwrap()];
    command.extend_from_slice(arguments);
    let output = cli(&command);
    assert!(
        output.status.success(),
        "arguments={arguments:?} stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn verify_attempt_history_contract(
    config: &Path,
    catalog: &Path,
    job_series_id: &str,
    first_run_id: &str,
    first_output: &Path,
) {
    let first_verification =
        successful_json(config, &["result", "verify", job_series_id, "--full"]);
    assert_eq!(first_verification["data"]["mode"], "full");
    assert_eq!(first_verification["data"]["run_id"], first_run_id);
    let canonical_digest = first_verification["data"]["canonical_output_sha256"]
        .as_str()
        .unwrap();
    let first_report = fs::read_to_string(first_output.join("run-report.md")).unwrap();
    assert!(first_report.contains(canonical_digest));

    let rerun = successful_json(config, &["job", "rerun", job_series_id]);
    assert_eq!(rerun["data"]["job_series_id"], job_series_id);
    assert_eq!(rerun["data"]["attempt"], 2);
    assert_eq!(rerun["data"]["state"], "queued");
    let second_run_id = rerun["data"]["run_id"].as_str().unwrap();
    assert_ne!(second_run_id, first_run_id);

    let waited = successful_json(config, &["job", "wait", job_series_id]);
    assert_eq!(waited["data"]["run_id"], second_run_id);
    assert_eq!(waited["data"]["attempt"], 2);
    assert_eq!(waited["data"]["state"], "complete");

    let second_verification =
        successful_json(config, &["result", "verify", second_run_id, "--full"]);
    assert_eq!(
        second_verification["data"]["canonical_output_sha256"],
        canonical_digest
    );
    let first_report = fs::read_to_string(first_output.join("run-report.md")).unwrap();
    assert!(first_report.contains(second_run_id));
    let second_output = PathBuf::from(waited["data"]["output_directory"].as_str().unwrap());
    assert!(
        fs::read_to_string(second_output.join("run-report.md"))
            .unwrap()
            .contains(canonical_digest)
    );
    let second_report_path = second_output.join("run-report.md");
    fs::remove_file(&second_report_path).unwrap();
    fs::create_dir(&second_report_path).unwrap();
    let report_failure = successful_json(config, &["result", "verify", second_run_id, "--full"]);
    assert!(
        report_failure["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| {
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic["code"] == "report.refresh_failed")
            })
    );
    fs::remove_dir(&second_report_path).unwrap();
    successful_json(config, &["run", "report", "--result", second_run_id]);

    let verified_files = artifact_fingerprint(first_output);
    let before_prune = artifact_fingerprint(first_output);
    assert_eq!(before_prune, verified_files);
    let plan = successful_json(config, &["job", "prune"]);
    assert_eq!(plan["data"]["mode"], "dry_run");
    assert_eq!(plan["data"]["delete_enabled"], false);
    let candidates = plan["data"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["run_id"], first_run_id);
    assert_eq!(candidates[0]["superseded_by"], second_run_id);
    assert_eq!(candidates[0]["protected"], false);
    assert_eq!(artifact_fingerprint(first_output), before_prune);

    let connection = Connection::open(catalog).unwrap();
    let attempts = connection
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE job_series_id = ?1",
            [job_series_id],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    let verifications = connection
        .query_row(
            "SELECT COUNT(*) FROM full_verifications
             JOIN jobs USING (run_id) WHERE jobs.job_series_id = ?1",
            [job_series_id],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    let superseded_by = connection
        .query_row(
            "SELECT superseded_by FROM attempt_supersession WHERE run_id = ?1",
            [first_run_id],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(verifications, 2);
    assert_eq!(superseded_by, second_run_id);
    drop(connection);

    successful_json(config, &["job", "forget", job_series_id]);
    assert!(
        fs::read_to_string(first_output.join("run-report.md"))
            .unwrap()
            .contains("\"visible_in_routine_list\": false")
    );
    let list = successful_json(config, &["job", "list"]);
    assert!(
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|snapshot| { snapshot["job_series_id"].as_str() != Some(job_series_id) })
    );
    let old_attempt = successful_json(config, &["result", "verify", first_run_id]);
    assert_eq!(old_attempt["data"]["run_id"], first_run_id);
    let hidden = successful_json(config, &["result", "inspect", first_run_id]);
    assert_eq!(hidden["data"]["catalog"]["visible_in_routine_list"], false);
    assert_eq!(hidden["data"]["catalog"]["superseded_by"], second_run_id);
    let hidden_files = artifact_fingerprint(first_output);

    let previous_daemon = daemon_instance(catalog);
    wait_for_daemon_release(catalog);
    let status = successful_json(config, &["job", "status", job_series_id]);
    assert_eq!(status["data"]["run_id"], second_run_id);
    assert_eq!(status["data"]["attempt"], 2);
    assert_ne!(daemon_instance(catalog), previous_daemon);
    assert_eq!(artifact_fingerprint(first_output), hidden_files);
}

fn wait_for_job(
    config: &Path,
    job_series_id: &str,
    condition: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let status = cli(&[
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "job",
            "status",
            job_series_id,
        ]);
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stdout)
        );
        let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        if condition(&value["data"]) {
            return value["data"].clone();
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_live_attempt(catalog: &Path) -> LiveAttempt {
    let started = Instant::now();
    loop {
        let connection = Connection::open(catalog).unwrap();
        let row = connection
            .query_row(
                "SELECT j.job_series_id, j.run_id, j.state, j.output_directory,
                        w.worker_pid, w.worker_start_token,
                        d.daemon_pid, d.daemon_start_token, d.daemon_instance_id
                 FROM jobs j
                 JOIN worker_leases w ON w.run_id = j.run_id
                 JOIN daemon_lease d ON d.singleton = 1
                 WHERE j.queue_order = (SELECT MAX(queue_order) FROM jobs)",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .unwrap();
        if let Some((
            job_series_id,
            run_id,
            state,
            Some(output_directory),
            worker_pid,
            worker_start_token,
            daemon_pid,
            daemon_start_token,
            daemon_instance_id,
        )) = row
        {
            let output_directory = PathBuf::from(output_directory);
            if state == "running" && output_directory.join("run-manifest.json").is_file() {
                return LiveAttempt {
                    job_series_id,
                    run_id,
                    output_directory,
                    worker: ProcessIdentity {
                        pid: u32::try_from(worker_pid).unwrap(),
                        start_token: worker_start_token,
                    },
                    daemon: ProcessIdentity {
                        pid: u32::try_from(daemon_pid).unwrap(),
                        start_token: daemon_start_token,
                    },
                    daemon_instance_id,
                };
            }
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_process_exit(identity: &ProcessIdentity) {
    let started = Instant::now();
    loop {
        if !process_identity_matches(identity).unwrap_or(false) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_worker_reattachment(
    catalog: &Path,
    run_id: &str,
    previous_daemon_instance_id: &str,
) -> (String, ProcessIdentity) {
    let started = Instant::now();
    loop {
        let connection = Connection::open(catalog).unwrap();
        let row = connection
            .query_row(
                "SELECT w.daemon_instance_id, w.worker_pid, w.worker_start_token,
                        d.daemon_instance_id
                 FROM worker_leases w
                 JOIN daemon_lease d ON d.singleton = 1
                 WHERE w.run_id = ?1",
                [run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .unwrap();
        if let Some((_worker_daemon, worker_pid, worker_start_token, daemon_instance)) =
            row.filter(|(worker_daemon, _, _, daemon_instance)| {
                worker_daemon == daemon_instance && daemon_instance != previous_daemon_instance_id
            })
        {
            return (
                daemon_instance,
                ProcessIdentity {
                    pid: u32::try_from(worker_pid).unwrap(),
                    start_token: worker_start_token,
                },
            );
        }
        assert!(started.elapsed() < Duration::from_secs(20));
        thread::sleep(Duration::from_millis(100));
    }
}

fn write_case(root: &Path, end_seconds: i64, particle_count: u64) {
    let body = r#"
schema_version: 0
kind: case
metadata: { name: cfsr-runtime }
time:
  start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
  end: { seconds_since_unix_epoch: END_SECONDS, nanosecond: 0 }
  direction: forward
meteorology:
  domains: [{ id: global, dataset: cfsr, priority: 1, horizontal_halo_cells: 1 }]
particle_population:
  strategy: release_driven
  id: release
  events:
    - id: event
      start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
      end: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
      particle_count: PARTICLE_COUNT
      mass: { tracer: { value: 1, unit: kg } }
      geometry: { source: inline, geometry: { type: Point, coordinates: [0, 0] } }
      vertical: { coordinate: above_sea_level, lower: { value: 100, unit: m } }
substances: [{ kind: water_vapor, id: tracer, display_name: Tracer }]
numerics:
  time_step: { value: 10, unit: min }
  integrator: { model: rk2_spherical/v0 }
  boundaries: { policies: [surface_reflect/v0, model_top_terminate/v0] }
"#
    .replace("END_SECONDS", &end_seconds.to_string())
    .replace("PARTICLE_COUNT", &particle_count.to_string());
    fs::write(root.join("cases/demo.yaml"), body).unwrap();
}

#[test]
fn daemon_starts_on_demand_persists_catalog_and_releases_idle_lease() {
    let temp = tempfile::tempdir().unwrap();
    let config = initialize_config(temp.path(), 1);
    let list = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "list",
    ]);
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(value["data"], serde_json::json!([]));

    let catalog = temp.path().join("runtime/jobs.sqlite3");
    assert!(catalog.is_file());
    let started = Instant::now();
    loop {
        let connection = Connection::open(&catalog).unwrap();
        let owners = connection
            .query_row("SELECT COUNT(*) FROM daemon_lease", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        if owners == 0 {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(8));
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn real_zero_duration_cfsr_run_uses_daemon_worker_and_terminal_identity() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tools/flexctl/target/test-data/cfsr/20090101/raw");
    let names = [
        "pgbl00.gdas.2009010100.grb2",
        "pgbl00.gdas.2009010106.grb2",
        "pgbl00.gdas.2009010112.grb2",
    ];
    if !names.iter().all(|name| source.join(name).is_file()) {
        eprintln!("skip: real CFSR 00/06/12 fixtures are unavailable");
        return;
    }

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    fs::create_dir_all(&target).unwrap();
    let temp = tempfile::Builder::new()
        .prefix("m5-a2-runtime-")
        .tempdir_in(target)
        .unwrap();
    let temp = temp.keep();
    let config = initialize_config(&temp, 2);
    let root = temp.join("project");
    for directory in ["cases", "profiles", "locks", "data"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for name in names {
        let destination = root.join("data").join(name);
        fs::hard_link(source.join(name), &destination)
            .or_else(|_| fs::copy(source.join(name), destination).map(|_| ()))
            .unwrap();
    }
    fs::write(
        root.join("trajecta-project.yaml"),
        r#"
schema_version: trajecta.project-index/v1
name: cfsr-runtime
cases: { demo: cases/demo.yaml }
profiles:
  local:
    path: profiles/local.yaml
    dataset_profiles: { cfsr: cfsr-pgbl-pressure-v0 }
"#,
    )
    .unwrap();
    write_case(&root, 1_230_789_600, 2);
    fs::write(
        root.join("profiles/local.yaml"),
        r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: ../cases/demo.yaml
output_root: ../runs
datasets:
  - dataset: cfsr
    lockfile: ../locks/project.lock.json
    data_roots: { raw: ../data }
execution:
  worker_threads: 1
  memory_budget_bytes: 536870912
  executor: cpu
  meteorology_reader: rust
"#,
    )
    .unwrap();

    let run = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "--project",
        root.to_str().unwrap(),
        "run",
        "--profile",
        "local",
    ]);
    let value: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    if !run.status.success() {
        let series = value["data"]["job_series_id"].as_str().unwrap_or("");
        let run_id = value["data"]["run_id"].as_str().unwrap_or("");
        let events = cli(&[
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "job",
            "events",
            series,
        ]);
        let worker_log = fs::read_to_string(
            temp.join("runtime/worker-logs")
                .join(format!("{run_id}.log")),
        )
        .unwrap_or_else(|error| format!("<worker log unavailable: {error}>"));
        panic!(
            "stdout={} stderr={} events={} worker_log={worker_log}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&events.stdout)
        );
    }
    assert_eq!(value["run_success"], true);
    assert_eq!(value["data"]["state"], "complete");
    assert_eq!(value["data"]["attempt"], 1);
    let series = value["data"]["job_series_id"].as_str().unwrap();
    let run_id = value["data"]["run_id"].as_str().unwrap();
    let output_directory = PathBuf::from(value["data"]["output_directory"].as_str().unwrap());
    assert!(output_directory.join("run-manifest.json").is_file());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(output_directory.join("run-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["job_series_id"], series);
    assert_eq!(manifest["run_id"], run_id);
    assert_eq!(manifest["attempt"], 1);
    let started = Instant::now();
    while !output_directory.join("run-report.md").is_file() {
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(25));
    }

    // A3 result products consume the same production artifact that the daemon
    // completed above; no hand-written manifest or SQLite fixture is used.
    let inspect = cli(&[
        "--format",
        "json",
        "result",
        "inspect",
        output_directory.to_str().unwrap(),
    ]);
    assert!(inspect.status.success());
    let inspect: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(inspect["command"], "result inspect");
    assert_eq!(inspect["data"]["identity"]["run_id"], run_id);
    assert_eq!(
        inspect["data"]["schema_version"],
        "trajecta.result-inspection/v1"
    );
    assert!(inspect["data"]["artifacts"].is_array());
    assert!(inspect["data"]["quality"].is_object());
    assert!(inspect["data"]["particles"].is_object());
    assert_eq!(inspect["data"]["particles"]["particle_adjoint_count"], 0);
    assert_eq!(inspect["data"]["particles"]["process_summary_count"], 0);
    assert_eq!(inspect["data"]["particles"]["process_event_count"], 0);
    let catalog_inspect = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "result",
        "inspect",
        series,
    ]);
    assert!(catalog_inspect.status.success());
    let catalog_inspect: serde_json::Value =
        serde_json::from_slice(&catalog_inspect.stdout).unwrap();
    assert_eq!(catalog_inspect["data"]["identity"]["run_id"], run_id);
    assert_eq!(
        catalog_inspect["data"]["catalog"]["visible_in_routine_list"],
        true
    );
    assert!(catalog_inspect["data"]["catalog"]["full_verification"].is_null());

    let particle_id: i64 = Connection::open(output_directory.join("particles.sqlite"))
        .unwrap()
        .query_row(
            "SELECT particle_id FROM particle ORDER BY particle_id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let processes = cli(&[
        "--format",
        "json",
        "result",
        "processes",
        output_directory.to_str().unwrap(),
        "--particle",
        &particle_id.to_string(),
        "--events",
        "--max-records",
        "1",
    ]);
    assert!(processes.status.success());
    let processes: serde_json::Value = serde_json::from_slice(&processes.stdout).unwrap();
    assert_eq!(processes["command"], "result processes");
    assert_eq!(
        processes["data"]["schema_version"],
        "trajecta.process-query/v1"
    );
    assert_eq!(processes["data"]["result"]["sqlite_user_version"], 2);
    assert_eq!(processes["data"]["filters"]["particle_ids"][0], particle_id);
    assert_eq!(processes["data"]["groups"], serde_json::json!([]));
    assert_eq!(processes["data"]["events"], serde_json::json!([]));
    assert_eq!(processes["data"]["event_count_total"], 0);
    assert_eq!(processes["data"]["event_count_returned"], 0);
    assert_eq!(processes["data"]["truncated"], false);

    let processes_jsonl = cli(&[
        "--format",
        "jsonl",
        "result",
        "processes",
        output_directory.to_str().unwrap(),
    ]);
    assert!(processes_jsonl.status.success());
    let process_items: Vec<serde_json::Value> = String::from_utf8(processes_jsonl.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(process_items.len(), 2);
    assert_eq!(process_items[0]["kind"], "data");
    assert_eq!(process_items[0]["sequence"], 1);
    assert_eq!(process_items[1]["kind"], "summary");
    assert_eq!(process_items[1]["sequence"], 2);
    let missing_process = cli(&[
        "--format",
        "human",
        "result",
        "processes",
        output_directory.to_str().unwrap(),
        "--particle",
        "9223372036854775807",
    ]);
    assert_eq!(missing_process.status.code(), Some(1));
    assert!(missing_process.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing_process.stderr).contains("result.particle_not_found"));

    let trajectory = cli(&[
        "--format",
        "json",
        "result",
        "trajectory",
        output_directory.to_str().unwrap(),
        "--particle-id",
        &particle_id.to_string(),
    ]);
    assert!(trajectory.status.success());
    let trajectory: serde_json::Value = serde_json::from_slice(&trajectory.stdout).unwrap();
    assert_eq!(trajectory["command"], "result trajectory");
    assert!(trajectory.get("exit_code").is_none());
    let records = trajectory["data"]["records"]
        .as_array()
        .expect("trajectory JSON data contains records array");
    assert!(!records.is_empty());
    assert_eq!(
        trajectory["data"]["schema_version"],
        "trajecta.trajectory-stream/v1"
    );
    assert_eq!(trajectory["data"]["run_id"], run_id);

    let trajectory_jsonl = cli(&[
        "--format",
        "jsonl",
        "result",
        "trajectory",
        output_directory.to_str().unwrap(),
        "--particle-id",
        &particle_id.to_string(),
    ]);
    assert!(trajectory_jsonl.status.success());
    let jsonl: Vec<serde_json::Value> = String::from_utf8(trajectory_jsonl.stdout)
        .expect("JSONL is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSONL item"))
        .collect();
    assert!(jsonl.len() >= 3);
    for (index, item) in jsonl.iter().enumerate() {
        assert_eq!(item["sequence"], u64::try_from(index + 1).unwrap());
    }
    assert_eq!(
        jsonl.first().and_then(|item| item.get("kind")),
        Some(&serde_json::Value::String("data".to_owned()))
    );
    assert_eq!(
        jsonl.last().and_then(|item| item.get("kind")),
        Some(&serde_json::Value::String("summary".to_owned()))
    );
    assert!(jsonl.last().unwrap()["summary"].is_object());
    assert!(jsonl.last().unwrap().get("data").is_none());

    let trajectory_human = cli(&[
        "--format",
        "human",
        "result",
        "trajectory",
        output_directory.to_str().unwrap(),
        "--particle-id",
        &particle_id.to_string(),
    ]);
    assert!(trajectory_human.status.success());
    let human = String::from_utf8(trajectory_human.stdout).expect("human output is UTF-8");
    assert!(human.starts_with("trajectory run_id="));
    assert!(!human.contains("{\"schema_version\""));

    let missing_trajectory = cli(&[
        "--format",
        "json",
        "result",
        "trajectory",
        output_directory.to_str().unwrap(),
        "--particle-id",
        "0",
    ]);
    assert_eq!(missing_trajectory.status.code(), Some(1));
    assert!(missing_trajectory.stderr.is_empty());
    let missing_trajectory: serde_json::Value =
        serde_json::from_slice(&missing_trajectory.stdout).unwrap();
    assert!(
        missing_trajectory["diagnostics"]
            .as_array()
            .is_some_and(|diagnostics| diagnostics
                .iter()
                .any(|diagnostic| { diagnostic["code"] == "result.particle_not_found" }))
    );

    let report = cli(&[
        "--format",
        "json",
        "run",
        "report",
        "--result",
        output_directory.to_str().unwrap(),
    ]);
    assert!(report.status.success());
    let report: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["data"]["path"], "run-report.md");
    let report_bytes = fs::read(output_directory.join("run-report.md")).unwrap();
    assert!(report_bytes.ends_with(b"\n"));
    let report_text = String::from_utf8(report_bytes.clone()).expect("report is UTF-8");
    for section in [
        "# Trajecta run report",
        "## Identity",
        "## Lifecycle",
        "## Inputs and software",
        "## Execution resources",
        "## Particles and terminations",
        "## Quality summary",
        "## Mass ledger",
        "## Verification and supersession",
        "## Artifacts and forensic pointers",
        "run-report.md is a derived human-readable view and is not part of the scientific digest identity.",
    ] {
        assert!(
            report_text.contains(section),
            "missing report section: {section}"
        );
    }
    let second_report = cli(&[
        "--format",
        "json",
        "run",
        "report",
        "--result",
        output_directory.to_str().unwrap(),
    ]);
    assert!(second_report.status.success());
    assert_eq!(
        fs::read(output_directory.join("run-report.md")).unwrap(),
        report_bytes
    );
    assert!(fs::read_dir(&output_directory).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".trajecta-run-report-")
    }));

    let events = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "events",
        series,
    ]);
    assert!(events.status.success());
    let events: serde_json::Value = serde_json::from_slice(&events.stdout).unwrap();
    assert!(
        events["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| { event["state"] == "complete" && event["kind"] == "state_transition" })
    );

    let catalog = temp.join("runtime/jobs.sqlite3");
    verify_attempt_history_contract(&config, &catalog, series, run_id, &output_directory);

    // A readable manifest with an unreadable SQLite artifact is an inspectable
    // partial result: the CLI returns one structured diagnostic, rather than
    // hiding the manifest identity or reporting a false successful SQLite view.
    fs::write(
        output_directory.join("particles.sqlite"),
        b"not a sqlite database",
    )
    .expect("corrupt copied result sqlite");
    let partial_inspect = cli(&[
        "--format",
        "json",
        "result",
        "inspect",
        output_directory.to_str().unwrap(),
    ]);
    assert!(partial_inspect.status.success());
    let partial_inspect: serde_json::Value =
        serde_json::from_slice(&partial_inspect.stdout).expect("partial inspect json");
    assert_eq!(partial_inspect["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(
        partial_inspect["diagnostics"][0]["code"],
        "result.inspect_sqlite_unavailable"
    );
    assert_eq!(partial_inspect["data"]["identity"]["run_id"], run_id);
    assert!(partial_inspect["data"]["quality"].is_null());

    let started = Instant::now();
    loop {
        let connection = Connection::open(&catalog).unwrap();
        let owners = connection
            .query_row("SELECT COUNT(*) FROM daemon_lease", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        if owners == 0 {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(100));
    }
    for (key, value) in [
        ("resources.memory_reserve_mib", "0"),
        ("resources.memory_pool_mib", "1000000"),
        ("resources.memory_reserve_mib", "999000"),
    ] {
        let updated = cli(&[
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "set",
            key,
            value,
        ]);
        assert!(updated.status.success());
    }
    let detached = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "--project",
        root.to_str().unwrap(),
        "run",
        "--profile",
        "local",
        "--detach",
    ]);
    assert!(detached.status.success());
    let detached: serde_json::Value = serde_json::from_slice(&detached.stdout).unwrap();
    assert_eq!(detached["data"]["state"], "queued");
    let queued_series = detached["data"]["job_series_id"].as_str().unwrap();
    let pressure_events = {
        let started = Instant::now();
        loop {
            let events = cli(&[
                "--format",
                "json",
                "--config",
                config.to_str().unwrap(),
                "job",
                "events",
                queued_series,
            ]);
            assert!(events.status.success());
            let value: serde_json::Value = serde_json::from_slice(&events.stdout).unwrap();
            if value["data"].as_array().unwrap().iter().any(|event| {
                event["kind"] == "warning"
                    && event["code"] == "daemon.external_memory_pressure"
                    && event["state"] == "queued"
            }) {
                break value;
            }
            assert!(started.elapsed() < Duration::from_secs(8));
            thread::sleep(Duration::from_millis(100));
        }
    };
    let same_cursor_consumer = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "events",
        queued_series,
    ]);
    assert!(same_cursor_consumer.status.success());
    let same_cursor_consumer: serde_json::Value =
        serde_json::from_slice(&same_cursor_consumer.stdout).unwrap();
    assert_eq!(same_cursor_consumer["data"], pressure_events["data"]);
    let cancelled = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "cancel",
        queued_series,
    ]);
    assert!(
        cancelled.status.success(),
        "stdout={} stderr={} forensic={}",
        String::from_utf8_lossy(&cancelled.stdout),
        String::from_utf8_lossy(&cancelled.stderr),
        temp.display()
    );
    let cancelled: serde_json::Value = serde_json::from_slice(&cancelled.stdout).unwrap();
    assert_eq!(cancelled["data"]["state"], "cancelled");
    assert!(cancelled["data"].get("started_at").is_none());
    assert!(cancelled["data"].get("output_directory").is_none());

    wait_for_daemon_release(&catalog);

    for (key, value) in [
        ("resources.memory_reserve_mib", "0"),
        ("resources.memory_pool_mib", "2048"),
        ("resources.memory_reserve_mib", "256"),
    ] {
        set_config(&config, key, value);
    }
    write_case(&root, 1_230_789_600, 10_000);
    let profile_path = root.join("profiles/local.yaml");
    let cancel_profile = fs::read_to_string(&profile_path)
        .unwrap()
        .replace("../locks/project.lock.json", "../locks/cancel.lock.json");
    fs::write(&profile_path, cancel_profile).unwrap();

    let safe = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "--project",
        root.to_str().unwrap(),
        "run",
        "--profile",
        "local",
        "--detach",
    ]);
    assert!(
        safe.status.success(),
        "stdout={} stderr={} forensic={}",
        String::from_utf8_lossy(&safe.stdout),
        String::from_utf8_lossy(&safe.stderr),
        temp.display()
    );
    let safe: serde_json::Value = serde_json::from_slice(&safe.stdout).unwrap();
    let safe_series = safe["data"]["job_series_id"].as_str().unwrap();
    let safe_running = wait_for_job(&config, safe_series, |snapshot| {
        snapshot["state"] == "running"
            && snapshot["output_directory"]
                .as_str()
                .is_some_and(|path| Path::new(path).join("run-manifest.json").is_file())
    });
    let safe_cancel = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "cancel",
        safe_series,
    ]);
    assert!(
        safe_cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&safe_cancel.stdout)
    );
    let safe_terminal = wait_for_job(&config, safe_series, |snapshot| {
        snapshot["state"] == "cancelled"
    });
    let safe_output = PathBuf::from(
        safe_terminal["output_directory"]
            .as_str()
            .or_else(|| safe_running["output_directory"].as_str())
            .unwrap(),
    );
    let safe_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(safe_output.join("run-manifest.json")).unwrap()).unwrap();
    assert_eq!(safe_manifest["status"], "cancelled");
    assert!(safe_manifest["finished_at"].is_object());
    assert!(safe_manifest["provenance"].is_object());
    assert!(safe_manifest.get("failure").is_none());
    assert!(safe_output.join("particles.sqlite").is_file());
    let started = Instant::now();
    while !safe_output.join("run-report.md").is_file() {
        assert!(started.elapsed() < Duration::from_secs(10));
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        fs::read_to_string(safe_output.join("run-report.md"))
            .unwrap()
            .contains("cancelled")
    );
    let safe_wal = safe_output.join("particles.sqlite-wal");
    assert!(!safe_wal.exists() || fs::metadata(safe_wal).unwrap().len() == 0);
    let safe_wait = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "wait",
        safe_series,
    ]);
    assert_eq!(safe_wait.status.code(), Some(1));
    let safe_wait: serde_json::Value = serde_json::from_slice(&safe_wait.stdout).unwrap();
    assert_eq!(safe_wait["data"]["state"], "cancelled");
    assert_eq!(safe_wait["run_success"], false);
    wait_for_daemon_release(&catalog);

    let forced = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "--project",
        root.to_str().unwrap(),
        "run",
        "--profile",
        "local",
        "--detach",
    ]);
    assert!(forced.status.success());
    let forced: serde_json::Value = serde_json::from_slice(&forced.stdout).unwrap();
    let forced_series = forced["data"]["job_series_id"].as_str().unwrap();
    let forced_running = wait_for_job(&config, forced_series, |snapshot| {
        snapshot["state"] == "running"
            && snapshot["output_directory"]
                .as_str()
                .is_some_and(|path| Path::new(path).join("run-manifest.json").is_file())
    });
    let force_cancel = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "cancel",
        forced_series,
        "--force",
    ]);
    assert!(
        force_cancel.status.success(),
        "{}",
        String::from_utf8_lossy(&force_cancel.stdout)
    );
    let force_cancel: serde_json::Value = serde_json::from_slice(&force_cancel.stdout).unwrap();
    assert_eq!(force_cancel["data"]["state"], "interrupted");
    let forced_output = PathBuf::from(forced_running["output_directory"].as_str().unwrap());
    let forced_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(forced_output.join("run-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(forced_manifest["status"], "interrupted");
    assert!(forced_manifest["finished_at"].is_object());
    assert_eq!(
        forced_manifest["failure"]["code"],
        "run.interrupted.worker_lost"
    );
    assert!(forced_manifest.get("provenance").is_none());
    assert!(
        fs::read_to_string(forced_output.join("run-report.md"))
            .unwrap()
            .contains("run.interrupted.worker_lost")
    );
    wait_for_daemon_release(&catalog);

    let mut foreground = Command::new(env!("CARGO_BIN_EXE_trajecta-cli"))
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "--project",
            root.to_str().unwrap(),
            "run",
            "--profile",
            "local",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let live = wait_for_live_attempt(&catalog);
    assert!(foreground.try_wait().unwrap().is_none());
    assert!(process_identity_matches(&live.worker).unwrap());
    assert!(process_identity_matches(&live.daemon).unwrap());

    foreground.kill().unwrap();
    let foreground_status = foreground.wait().unwrap();
    assert!(!foreground_status.success());
    assert!(process_identity_matches(&live.worker).unwrap());
    let after_disconnect = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "status",
        &live.job_series_id,
    ]);
    assert!(after_disconnect.status.success());
    let after_disconnect: serde_json::Value =
        serde_json::from_slice(&after_disconnect.stdout).unwrap();
    assert_eq!(after_disconnect["data"]["state"], "running");

    force_terminate_process(&live.daemon).unwrap();
    wait_for_process_exit(&live.daemon);
    assert!(process_identity_matches(&live.worker).unwrap());
    let after_restart = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "status",
        &live.job_series_id,
    ]);
    assert!(
        after_restart.status.success(),
        "stdout={} stderr={} forensic={}",
        String::from_utf8_lossy(&after_restart.stdout),
        String::from_utf8_lossy(&after_restart.stderr),
        temp.display()
    );
    let (_new_daemon_instance_id, reattached_worker) =
        wait_for_worker_reattachment(&catalog, &live.run_id, &live.daemon_instance_id);
    assert_eq!(reattached_worker, live.worker);
    assert!(process_identity_matches(&reattached_worker).unwrap());
    let recovery_events = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "events",
        &live.job_series_id,
    ]);
    assert!(recovery_events.status.success());
    let recovery_events: serde_json::Value =
        serde_json::from_slice(&recovery_events.stdout).unwrap();
    assert!(
        recovery_events["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| { event["code"] == "worker.reattached" && event["state"] == "running" })
    );

    let stop_recovered = cli(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "job",
        "cancel",
        &live.job_series_id,
        "--force",
    ]);
    assert!(
        stop_recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&stop_recovered.stdout)
    );
    let stop_recovered: serde_json::Value = serde_json::from_slice(&stop_recovered.stdout).unwrap();
    assert_eq!(stop_recovered["data"]["state"], "interrupted");
    wait_for_process_exit(&reattached_worker);
    let recovered_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(live.output_directory.join("run-manifest.json")).unwrap())
            .unwrap();
    assert_eq!(recovered_manifest["status"], "interrupted");
    assert!(
        fs::read_to_string(live.output_directory.join("run-report.md"))
            .unwrap()
            .contains("run.interrupted.worker_lost")
    );
    wait_for_daemon_release(&catalog);
    fs::remove_dir_all(temp).unwrap();
}
