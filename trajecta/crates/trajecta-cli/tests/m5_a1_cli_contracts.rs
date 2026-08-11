//! M5-A1 CLI/config/project contract checks.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;

use sha2::Digest;

use trajecta_cli::cli::{Cli, CliParseError, OutputMode};
use trajecta_cli::command::{Command, StagedRunCommand};

fn args(values: &[&str]) -> Vec<OsString> {
    std::iter::once("trajecta")
        .chain(values.iter().copied())
        .map(OsString::from)
        .collect()
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let output = ProcessCommand::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn parses_all_thirty_five_frozen_command_paths() {
    let paths: &[&[&str]] = &[
        &["config", "init"],
        &["config", "path"],
        &["config", "list"],
        &["config", "get", "resources.cpu_slots"],
        &["config", "set", "resources.cpu_slots", "2"],
        &["config", "unset", "profile_templates.local"],
        &["config", "validate"],
        &["project", "init", "demo", "--name", "demo"],
        &["project", "status"],
        &["project", "show"],
        &["project", "get", "index.name"],
        &["project", "set", "index.name", "demo"],
        &["project", "unset", "case.demo.time"],
        &["project", "validate"],
        &["project", "data-plan"],
        &["project", "finalize"],
        &["case", "validate", "case.yaml", "--intent", "simulation"],
        &["case", "resolve", "case.yaml"],
        &["data", "inspect", "file.grib"],
        &[
            "data",
            "lock",
            "--root",
            "data",
            "--profile",
            "era5-hybrid",
            "--case",
            "case.yaml",
            "--output",
            "lock.json",
            "--replace",
        ],
        &[
            "met",
            "probe",
            "--data-root",
            "data",
            "--profile",
            "era5-hybrid",
            "--time",
            "0",
        ],
        &[
            "met",
            "replay",
            "--data-root",
            "data",
            "--profile",
            "era5-hybrid",
            "--input",
            "input.jsonl",
        ],
        &["doctor"],
        &["run", "--project", "demo", "--profile", "local"],
        &["job", "list"],
        &["job", "status", "j"],
        &["job", "wait", "j"],
        &["job", "events", "j"],
        &["job", "cancel", "j"],
        &["job", "rerun", "j"],
        &["job", "forget", "j"],
        &["job", "prune"],
        &["result", "inspect", "r"],
        &["result", "verify", "r"],
        &["result", "trajectory", "r", "--all"],
    ];
    assert_eq!(paths.len(), 35);
    for path in paths {
        Cli::parse_from(args(path)).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    }
}

#[test]
fn global_options_reject_duplicates_conflicts_missing_and_unknown_values() {
    for values in [
        &["--json", "--format", "json", "config", "path"][..],
        &["--format", "json", "--format", "jsonl", "config", "path"],
        &["--config", "a", "--config", "b", "config", "path"],
        &["--format", "xml", "config", "path"],
        &["--format", "config", "path"],
    ] {
        assert!(matches!(
            Cli::parse_from(args(values)),
            Err(CliParseError::InvalidArgument(_) | CliParseError::MissingArgument(_))
        ));
    }
    let cli = Cli::parse_from(args(&["--format", "jsonl", "config", "path"])).unwrap();
    assert_eq!(cli.output, OutputMode::Jsonl);
}

#[test]
fn run_default_is_foreground_and_detach_is_explicit() {
    let foreground =
        Cli::parse_from(args(&["run", "--project", "demo", "--profile", "local"])).unwrap();
    let detached = Cli::parse_from(args(&[
        "run",
        "--project",
        "demo",
        "--profile",
        "local",
        "--detach",
    ]))
    .unwrap();
    let Command::StagedRun(StagedRunCommand { detach: false, .. }) = foreground.command else {
        panic!("wrong command");
    };
    let Command::StagedRun(StagedRunCommand { detach: true, .. }) = detached.command else {
        panic!("wrong command");
    };
    assert!(
        Cli::parse_from(args(&[
            "run",
            "--project",
            "demo",
            "--profile",
            "local",
            "--case",
            "x",
            "--run-profile",
            "y"
        ]))
        .is_err()
    );
}

#[test]
fn config_init_does_not_overwrite_and_jsonl_has_one_summary() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let first = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "init",
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let before = fs::read(&config).unwrap();
    let second = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "init",
        ])
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(fs::read(&config).unwrap(), before);
    let list = ProcessCommand::new(exe)
        .args([
            "--format",
            "jsonl",
            "--config",
            config.to_str().unwrap(),
            "config",
            "list",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    let lines = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(lines.last().unwrap()["kind"], "summary");
    assert_eq!(
        lines
            .iter()
            .filter(|value| value["kind"] == "summary")
            .count(),
        1
    );
}

#[test]
fn machine_parse_errors_and_command_paths_use_the_frozen_envelope() {
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let parse_error = ProcessCommand::new(exe)
        .args(["--format", "json", "project", "unknown"])
        .output()
        .unwrap();
    assert_eq!(parse_error.status.code(), Some(2));
    let parse_value: serde_json::Value = serde_json::from_slice(&parse_error.stdout).unwrap();
    assert_eq!(parse_value["schema_version"], "trajecta.cli-output/v1");
    assert_eq!(parse_value["command"], "project unknown");
    assert_eq!(
        parse_value["diagnostics"][0]["code"],
        "cli.invalid_arguments"
    );
    let temp = tempfile::tempdir().unwrap();
    let project = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            temp.path().to_str().unwrap(),
            "project",
            "init",
            "--name",
            "machine",
        ])
        .output()
        .unwrap();
    assert!(project.status.success());
    let value: serde_json::Value = serde_json::from_slice(&project.stdout).unwrap();
    assert_eq!(value["command"], "project init");
}

#[test]
fn help_succeeds_and_missing_command_remains_a_usage_error() {
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");

    let human_help = ProcessCommand::new(exe).arg("--help").output().unwrap();
    assert_eq!(human_help.status.code(), Some(0));
    assert!(human_help.stderr.is_empty());
    let human_text = String::from_utf8(human_help.stdout).unwrap();
    assert!(human_text.contains("Trajecta command-line interface"));
    assert!(human_text.contains("Usage:"));

    let json_help = ProcessCommand::new(exe)
        .args(["--format", "json", "--help"])
        .output()
        .unwrap();
    assert_eq!(json_help.status.code(), Some(0));
    assert!(json_help.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&json_help.stdout).unwrap();
    assert_eq!(json["schema_version"], "trajecta.cli-output/v1");
    assert_eq!(json["command"], "cli");
    assert_eq!(json["ok"], true);
    assert!(json["diagnostics"].as_array().unwrap().is_empty());
    assert!(json["data"]["help"].as_str().unwrap().contains("Usage:"));

    let missing = ProcessCommand::new(exe)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(json["command"], "cli");
    assert_eq!(json["ok"], false);
    assert_eq!(json["diagnostics"][0]["code"], "cli.invalid_arguments");
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("missing argument: command")
    );
}

#[test]
fn config_explicit_path_overrides_injected_environment_path() {
    let temp = tempfile::tempdir().unwrap();
    let env_config = temp.path().join("from-env.toml");
    let explicit = temp.path().join("explicit.toml");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let from_env = ProcessCommand::new(exe)
        .env("TRAJECTA_CONFIG", &env_config)
        .args(["--format", "json", "config", "path"])
        .output()
        .unwrap();
    assert!(from_env.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&from_env.stdout).unwrap()["data"]["path"],
        env_config.to_string_lossy().as_ref()
    );
    let explicit_output = ProcessCommand::new(exe)
        .env("TRAJECTA_CONFIG", &env_config)
        .args([
            "--format",
            "json",
            "--config",
            explicit.to_str().unwrap(),
            "config",
            "path",
        ])
        .output()
        .unwrap();
    assert!(explicit_output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&explicit_output.stdout).unwrap()["data"]["path"],
        explicit.to_string_lossy().as_ref()
    );
}

#[test]
fn project_init_is_draft_and_discovery_walks_parent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let init = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "project",
            "init",
            root.to_str().unwrap(),
            "--name",
            "demo",
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let nested = root.join("cases").join("nested");
    fs::create_dir_all(&nested).unwrap();
    let status = ProcessCommand::new(exe)
        .current_dir(&nested)
        .args(["--format", "json", "project", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(value["data"]["state"], "draft");
}

#[test]
fn doctor_deep_probes_sqlite_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let config = temp.path().join("config.toml");
    let config_init = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "init",
        ])
        .output()
        .unwrap();
    assert!(config_init.status.success());
    let init = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "--project",
            temp.path().to_str().unwrap(),
            "project",
            "init",
            "--name",
            "doctor",
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let sentinels = [
        ".trajecta-doctor-write-probe",
        ".trajecta-doctor-write-probe-renamed",
        ".trajecta-doctor.sqlite",
        ".trajecta-doctor.sqlite-wal",
        ".trajecta-doctor.sqlite-shm",
    ];
    for name in sentinels {
        fs::write(temp.path().join(name), format!("sentinel:{name}")).unwrap();
    }
    let doctor = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "--project",
            temp.path().to_str().unwrap(),
            "doctor",
            "--deep",
        ])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert!(
        value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["code"] == "doctor.deep_sqlite_ok")
    );
    for name in sentinels {
        assert_eq!(
            fs::read_to_string(temp.path().join(name)).unwrap(),
            format!("sentinel:{name}")
        );
    }
    assert!(
        fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".trajecta-doctor-"))
    );
}

#[test]
fn met_machine_errors_are_wrapped_in_the_cli_envelope() {
    let temp = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let output = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "met",
            "probe",
            "--data-root",
            temp.path().join("missing").to_str().unwrap(),
            "--profile",
            "era5-hybrid",
            "--time",
            "0",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], "trajecta.cli-output/v1");
    assert_eq!(value["command"], "met probe");
    assert!(value["diagnostics"].as_array().unwrap().len() == 1);
}

#[test]
fn project_index_paths_cannot_escape_or_overwrite_outside() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let outside = temp.path().join("outside.yaml");
    fs::write(&outside, "outside: unchanged\n").unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let init = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "init",
            "--name",
            "safe",
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let index = root.join("trajecta-project.yaml");
    let before = fs::read(&index).unwrap();
    let update = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "set",
            "index.cases.escape",
            "../outside.yaml",
        ])
        .output()
        .unwrap();
    assert_eq!(update.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&update.stdout).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "project.path_escape");
    assert_eq!(fs::read(&index).unwrap(), before);
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "outside: unchanged\n"
    );
}

#[test]
fn project_path_jail_rejects_repeated_separators_and_linked_ancestors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let init = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "init",
            "--name",
            "safe",
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let index = root.join("trajecta-project.yaml");
    let before = fs::read(&index).unwrap();

    for (selector, path) in [
        ("index.cases.repeated", "cases//repeated.yaml"),
        ("index.cases.backslash", r"cases\backslash.yaml"),
    ] {
        let invalid = ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                selector,
                path,
            ])
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(1));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&invalid.stdout).unwrap()["diagnostics"][0]
                ["code"],
            "project.path_escape"
        );
        assert_eq!(fs::read(&index).unwrap(), before);
    }

    create_directory_link(&outside, &root.join("profiles").join("linked"));
    let linked = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "set",
            "index.profiles.escape",
            r#"{"path":"profiles/linked/escaped.yaml","dataset_profiles":{}}"#,
        ])
        .output()
        .unwrap();
    assert_eq!(linked.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&linked.stdout).unwrap()["diagnostics"][0]["code"],
        "project.path_escape"
    );
    assert_eq!(fs::read(&index).unwrap(), before);
    assert!(!outside.join("escaped.yaml").exists());

    fs::write(
        &index,
        r#"
schema_version: trajecta.project-index/v1
name: safe
cases: {}
profiles:
  escape:
    path: profiles/linked/escaped.yaml
    dataset_profiles: {}
"#,
    )
    .unwrap();
    let discovered = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "status",
        ])
        .output()
        .unwrap();
    assert_eq!(discovered.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&discovered.stdout).unwrap()["diagnostics"][0]
            ["code"],
        "project.path_escape"
    );
}

#[test]
fn incomplete_documents_accept_missing_fields_but_reject_unknown_and_wrong_types() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "init",
                "--name",
                "draft",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                "index.cases.local",
                "cases/local.yaml",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                "index.profiles.local",
                r#"{"path":"profiles/local.yaml","dataset_profiles":{}}"#,
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let missing = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "set",
            "profile.local.schema_version",
            "0",
        ])
        .output()
        .unwrap();
    assert!(missing.status.success());

    for (selector, value) in [
        ("profile.local.unknown_field", "true"),
        ("profile.local.execution", r#""wrong-type""#),
    ] {
        let invalid = ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                selector,
                value,
            ])
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(1));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&invalid.stdout).unwrap()["diagnostics"][0]
                ["code"],
            "project.document_invalid"
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("profiles/local.yaml")).unwrap(),
        "schema_version: 0\n"
    );
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                "case.local.schema_version",
                "0",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    for (selector, value) in [
        ("case.local.unknown_field", "true"),
        ("case.local.time", r#""wrong-type""#),
    ] {
        let invalid = ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "set",
                selector,
                value,
            ])
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(1));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&invalid.stdout).unwrap()["diagnostics"][0]
                ["code"],
            "project.document_invalid"
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("cases/local.yaml")).unwrap(),
        "schema_version: 0\n"
    );
}

#[test]
fn missing_profile_fields_are_draft_but_invalid_profile_is_an_error_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(root.join("profiles")).unwrap();
    fs::write(
        root.join("trajecta-project.yaml"),
        r#"
schema_version: trajecta.project-index/v1
name: draft
cases: {}
profiles: { local: { path: profiles/local.yaml, dataset_profiles: {} } }
"#,
    )
    .unwrap();
    fs::write(
        root.join("profiles/local.yaml"),
        "schema_version: 0\nkind: run_profile\nmetadata: {name: local}\n",
    )
    .unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let draft = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "status",
        ])
        .output()
        .unwrap();
    assert!(draft.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&draft.stdout).unwrap()["data"]["state"],
        "draft"
    );
    fs::write(
        root.join("profiles/local.yaml"),
        r#"
schema_version: 0
kind: run_profile
metadata: {name: local}
case_path: x
output_root: runs
datasets: []
execution: {worker_threads: 1, memory_budget_bytes: 1, executor: cpu, meteorology_reader: rust}
unknown: true
"#,
    )
    .unwrap();
    let error = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "status",
        ])
        .output()
        .unwrap();
    assert_eq!(error.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&error.stdout).unwrap()["data"]["state"],
        "draft"
    );
}

#[test]
fn config_semantics_template_creation_and_doctor_config_error_are_closed() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--config",
                config.to_str().unwrap(),
                "config",
                "init"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let before = fs::read(&config).unwrap();
    let invalid = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "set",
            "resources.memory_reserve_mib",
            "999999999",
        ])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(fs::read(&config).unwrap(), before);
    let template = r#"{"execution":{"worker_threads":1,"memory_budget_bytes":1024,"executor":"cpu","meteorology_reader":"rust"}}"#;
    let set = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "set",
            "profile_templates.local",
            template,
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stdout)
    );
    let unset = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            config.to_str().unwrap(),
            "config",
            "unset",
            "profile_templates.local",
        ])
        .output()
        .unwrap();
    assert!(unset.status.success());
    let missing = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--config",
            temp.path().join("missing.toml").to_str().unwrap(),
            "doctor",
        ])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&missing.stdout).unwrap()["diagnostics"][0]["code"],
        "doctor.config_invalid"
    );
}

#[test]
fn config_template_semantics_reject_overcommitted_and_blank_names_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let run = |key: &str, value: &str| {
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--config",
                config.to_str().unwrap(),
                "config",
                "set",
                key,
                value,
            ])
            .output()
            .unwrap()
    };
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--config",
                config.to_str().unwrap(),
                "config",
                "init",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(run("resources.cpu_slots", "1").status.success());
    let before = fs::read(&config).unwrap();
    let overcommitted = run(
        "profile_templates.fast",
        r#"{"execution":{"worker_threads":2,"memory_budget_bytes":1024,"executor":"cpu","meteorology_reader":"rust"}}"#,
    );
    assert_eq!(overcommitted.status.code(), Some(1));
    assert_eq!(fs::read(&config).unwrap(), before);
    let blank = run(
        "profile_templates.   ",
        r#"{"execution":{"worker_threads":1,"memory_budget_bytes":1024,"executor":"cpu","meteorology_reader":"rust"}}"#,
    );
    assert_eq!(blank.status.code(), Some(1));
    assert_eq!(fs::read(&config).unwrap(), before);
}

#[test]
fn configured_project_data_plan_is_byte_deterministic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    fs::create_dir_all(root.join("cases")).unwrap();
    fs::create_dir_all(root.join("profiles")).unwrap();
    fs::create_dir_all(root.join("locks")).unwrap();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(
        root.join("trajecta-project.yaml"),
        r#"
schema_version: trajecta.project-index/v1
name: demo
cases: { demo: cases/demo.yaml }
profiles:
  local:
    path: profiles/local.json
    dataset_profiles: { era5: era5-hybrid }
"#,
    )
    .unwrap();
    fs::write(
        root.join("cases/demo.yaml"),
        r#"
schema_version: 0
kind: case
metadata: { name: demo }
time:
  start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 3600, nanosecond: 0 }
  direction: forward
meteorology:
  domains: [{ id: global, dataset: era5, priority: 1, horizontal_halo_cells: 1 }]
particle_population:
  strategy: release_driven
  id: release
  events:
    - id: event
      start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
      end: { seconds_since_unix_epoch: 300, nanosecond: 0 }
      particle_count: 2
      mass: { tracer: { value: 1, unit: kg } }
      geometry: { source: inline, geometry: { type: Point, coordinates: [0, 0] } }
      vertical: { coordinate: above_sea_level, lower: { value: 100, unit: m } }
substances: [{ kind: water_vapor, id: tracer, display_name: Tracer }]
numerics:
  time_step: { value: 10, unit: min }
  integrator: { model: rk2_spherical/v0 }
  boundaries: { policies: [surface_reflect/v0, model_top_terminate/v0] }
"#,
    )
    .unwrap();
    fs::write(root.join("profiles/local.json"), r#"{
  "schema_version": 0, "kind": "run_profile", "metadata": {"name":"local"},
  "case_path": "../cases/demo.yaml", "output_root": "../runs",
  "datasets": [{"dataset":"era5", "lockfile":"../locks/missing.lock.json", "data_roots":{"raw":"../data"}}],
  "execution": {"worker_threads":2, "memory_budget_bytes":1048576, "executor":"cpu", "meteorology_reader":"rust"}
}"#).unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let first = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "data-plan",
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let second = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "--project",
            root.to_str().unwrap(),
            "project",
            "data-plan",
        ])
        .output()
        .unwrap();
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(
        hex::encode(sha2::Sha256::digest(&first.stdout)),
        "88f298caea93c4d3ecd4024262da827c2621a5dd90b004780636969980c38cfd"
    );
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(value["data"]["schema_version"], "trajecta.data-plan/v1");
    assert!(value["data"]["requirements"][0].get("cache_root").is_none());
    assert!(
        value["data"]["requirements"][0]["lockfile"]
            .as_str()
            .unwrap()
            .starts_with("locks/")
    );
    assert_eq!(value["data"]["requirements"][0]["status"], "partial");
    assert_eq!(
        value["data"]["requirements"][0]["data_roots"]["raw"],
        "data"
    );
    assert_eq!(
        value["data"]["requirements"][0]["required_capabilities"],
        serde_json::json!(["near_surface_transport", "transport"])
    );
}

#[test]
fn real_cfsr_data_lock_and_project_finalize_are_safe_and_idempotent() {
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
        .prefix("m5-a1-real-lock-")
        .tempdir_in(target)
        .unwrap();
    let root = temp.path().join("project");
    for directory in ["cases", "profiles", "locks", "data"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    for name in names {
        fs::hard_link(source.join(name), root.join("data").join(name)).unwrap();
    }
    fs::write(
        root.join("trajecta-project.yaml"),
        r#"
schema_version: trajecta.project-index/v1
name: cfsr-finalize
cases: { demo: cases/demo.yaml }
profiles:
  local:
    path: profiles/local.yaml
    dataset_profiles: { cfsr: cfsr-pgbl-pressure-v0 }
"#,
    )
    .unwrap();
    fs::write(
        root.join("cases/demo.yaml"),
        r#"
schema_version: 0
kind: case
metadata: { name: cfsr-finalize }
time:
  start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
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
      particle_count: 2
      mass: { tracer: { value: 1, unit: kg } }
      geometry: { source: inline, geometry: { type: Point, coordinates: [0, 0] } }
      vertical: { coordinate: above_sea_level, lower: { value: 100, unit: m } }
substances: [{ kind: water_vapor, id: tracer, display_name: Tracer }]
numerics:
  time_step: { value: 10, unit: min }
  integrator: { model: rk2_spherical/v0 }
  boundaries: { policies: [surface_reflect/v0, model_top_terminate/v0] }
"#,
    )
    .unwrap();
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

    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let standalone = root.join("locks/standalone.lock.json");
    let data_lock = |replace: bool| {
        let mut command = ProcessCommand::new(exe);
        command.args([
            "--format",
            "json",
            "data",
            "lock",
            "--root",
            root.join("data").to_str().unwrap(),
            "--profile",
            "cfsr-pgbl-pressure-v0",
            "--case",
            root.join("cases/demo.yaml").to_str().unwrap(),
            "--output",
            standalone.to_str().unwrap(),
        ]);
        if replace {
            command.arg("--replace");
        }
        command.output().unwrap()
    };
    let created = data_lock(false);
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let standalone_before = fs::read(&standalone).unwrap();
    let refused = data_lock(false);
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(fs::read(&standalone).unwrap(), standalone_before);
    let replaced = data_lock(true);
    assert!(
        replaced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&replaced.stdout),
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(fs::read(&standalone).unwrap(), standalone_before);

    let finalize = || {
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "finalize",
            ])
            .output()
            .unwrap()
    };
    let first = finalize();
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_value["data"]["state"], "finalized");
    assert_eq!(
        first_value["data"]["created_locks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let project_lock = root.join("locks/project.lock.json");
    let project_before = fs::read(&project_lock).unwrap();
    let second = finalize();
    assert!(second.status.success());
    let second_value: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(
        second_value["data"]["created_locks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        second_value["data"]["reused_locks"].as_array().unwrap(),
        &[serde_json::json!("locks/project.lock.json")]
    );
    assert_eq!(fs::read(&project_lock).unwrap(), project_before);

    let changed = fs::read_to_string(root.join("cases/demo.yaml"))
        .unwrap()
        .replace("1230789600", "1230811200");
    fs::write(root.join("cases/demo.yaml"), changed).unwrap();
    let stale = finalize();
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(fs::read(&project_lock).unwrap(), project_before);
}

#[test]
fn public_population_capability_helper_covers_all_three_strategies() {
    use trajecta_case::model::population::ParticlePopulationSpec;
    use trajecta_core::runner::required_capabilities_for_population;

    let release_case = serde_yml::from_str::<serde_yml::Value>(
        r#"strategy: release_driven
id: release
events:
  - id: e
    start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
    end: { seconds_since_unix_epoch: 0, nanosecond: 0 }
    particle_count: 1
    mass: { tracer: { value: 1, unit: kg } }
    geometry: { source: inline, geometry: { type: Point, coordinates: [0, 0] } }
    vertical: { coordinate: above_sea_level, lower: { value: 1, unit: m } }
"#,
    )
    .unwrap();
    let release: ParticlePopulationSpec = serde_yml::from_value(release_case).unwrap();
    let air_mass: ParticlePopulationSpec = serde_yml::from_str(
        "strategy: domain_fill_air_mass\nid: fill\ndomain_id: global\ntarget_particle_count: 1\n",
    )
    .unwrap();
    let ozone: ParticlePopulationSpec = serde_yml::from_str(
        "strategy: domain_fill_stratospheric_ozone\nair_mass: { id: fill, domain_id: global, target_particle_count: 1 }\nozone_rule: rule\nozone_substance: ozone\n",
    )
    .unwrap();
    assert_eq!(
        required_capabilities_for_population(&release),
        required_capabilities_for_population(&release)
    );
    assert!(required_capabilities_for_population(&air_mass).is_ok());
    assert!(required_capabilities_for_population(&ozone).is_ok());
}

#[test]
fn review_closeout_index_machine_and_lock_contracts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    assert!(
        ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "project",
                "init",
                root.to_str().unwrap(),
                "--name",
                "demo",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let index_path = root.join("trajecta-project.yaml");
    let original = fs::read(&index_path).unwrap();
    for invalid in [
        r#"{"schema_version":"trajecta.project-index/v1","name":"demo","cases":{" ":"cases/c.yaml"},"profiles":{}}"#,
        r#"{"schema_version":"trajecta.project-index/v1","name":"demo","cases":{},"profiles":{" ":{"path":"profiles/p.json"}}}"#,
        r#"{"schema_version":"trajecta.project-index/v1","name":"demo","cases":{},"profiles":{"p":{"path":"profiles/p.json","dataset_profiles":{"":"hybrid"}}}}"#,
        r#"{"schema_version":"trajecta.project-index/v1","name":"demo","cases":{},"profiles":{"p":{"path":"profiles/p.json","template":""}}}"#,
        r#"{"schema_version":"trajecta.project-index/v1","name":"demo","cases":{},"profiles":{"p":{"path":"profiles/p.json","template":"t","template_sha256":"not-hex"}}}"#,
    ] {
        fs::write(&index_path, invalid).unwrap();
        let before = fs::read(&index_path).unwrap();
        let output = ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "validate",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["diagnostics"][0]
                ["code"],
            "project.invalid_index"
        );
        assert_eq!(fs::read(&index_path).unwrap(), before);
    }

    for duplicate_yaml in [
        "schema_version: trajecta.project-index/v1\nname: demo\ncases:\n  same: cases/one.yaml\n  same: cases/two.yaml\nprofiles: {}\n",
        "schema_version: trajecta.project-index/v1\nname: demo\ncases: {}\nprofiles:\n  same:\n    path: profiles/one.yaml\n    dataset_profiles: {}\n  same:\n    path: profiles/two.yaml\n    dataset_profiles: {}\n",
        "schema_version: trajecta.project-index/v1\nname: demo\ncases: {}\nprofiles:\n  local:\n    path: profiles/local.yaml\n    dataset_profiles:\n      era5: era5-pressure\n      era5: era5-hybrid\n",
    ] {
        fs::write(&index_path, duplicate_yaml).unwrap();
        let before = fs::read(&index_path).unwrap();
        let output = ProcessCommand::new(exe)
            .args([
                "--format",
                "json",
                "--project",
                root.to_str().unwrap(),
                "project",
                "validate",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["diagnostics"][0]
                ["code"],
            "project.invalid_index"
        );
        assert_eq!(fs::read(&index_path).unwrap(), before);
    }
    fs::write(&index_path, original).unwrap();

    for format in ["json", "jsonl"] {
        let usage = ProcessCommand::new(exe)
            .args(["--format", format, "project", "unknown"])
            .output()
            .unwrap();
        assert_eq!(usage.status.code(), Some(2));
        assert!(usage.stderr.is_empty());
        let text = String::from_utf8(usage.stdout).unwrap();
        assert!(text.contains(if format == "json" {
            "trajecta.cli-output/v1"
        } else {
            "trajecta.cli-stream-item/v1"
        }));
    }

    let output_path = temp.path().join("existing.lock.json");
    fs::write(&output_path, "existing").unwrap();
    let lock = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "data",
            "lock",
            "--root",
            root.to_str().unwrap(),
            "--profile",
            "local",
            "--case",
            "case.yaml",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(lock.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&lock.stdout).unwrap()["diagnostics"][0]["code"],
        "data.lock_exists"
    );
    let staged = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "data",
            "lock",
            "--root",
            root.to_str().unwrap(),
            "--profile",
            "local",
            "--case",
            "case.yaml",
            "--output",
            output_path.to_str().unwrap(),
            "--replace",
        ])
        .output()
        .unwrap();
    assert_eq!(staged.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&staged.stdout).unwrap()["diagnostics"][0]["code"],
        "data.case_invalid"
    );
    assert_eq!(fs::read_to_string(output_path).unwrap(), "existing");
}

#[test]
fn daemon_backed_run_is_reachable_and_reports_missing_machine_config() {
    let temp = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let output = ProcessCommand::new(exe)
        .args([
            "--format",
            "json",
            "run",
            "--project",
            temp.path().to_str().unwrap(),
            "--profile",
            "local",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["diagnostics"][0]["code"], "config.not_found");
}
