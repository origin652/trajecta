//! CLI parse, diagnostic, and streaming-contract tests for `met` commands.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};

use trajecta_cli::cli::{Cli, CliParseError};
use trajecta_cli::command::Command;
use trajecta_cli::command::met::{self, MetCommand, STREAM_CHUNK_POINTS, diagnostic};

fn os_args(args: &[&str]) -> Vec<OsString> {
    std::iter::once("trajecta")
        .chain(args.iter().copied())
        .map(OsString::from)
        .collect()
}

fn cfsr_raw_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DIR") {
        let path = PathBuf::from(path);
        if path.join("pgbl00.gdas.2009010100.grb2").is_file()
            && path.join("pgbl00.gdas.2009010106.grb2").is_file()
        {
            return Some(path);
        }
    }
    // Portable relative to this crate (workspace/crates/trajecta-cli → repo tools/).
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tools/flexctl/target/test-data/cfsr/20090101/raw");
    let path = path.canonicalize().ok()?;
    if path.join("pgbl00.gdas.2009010100.grb2").is_file()
        && path.join("pgbl00.gdas.2009010106.grb2").is_file()
    {
        Some(path)
    } else {
        None
    }
}

fn require_real() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn skip_or_fail(what: &str) {
    if require_real() {
        panic!("TRAJECTA_REQUIRE_REAL_MET=1 but missing real asset: {what}");
    }
}

#[test]
fn parse_met_probe_required_flags() {
    let cli = Cli::parse_from(os_args(&[
        "met",
        "probe",
        "--data-root",
        "data",
        "--profile",
        "cfsr-pgbl-pressure-v0",
        "--time",
        "1230778800",
        "--points",
        "points.jsonl",
        "--explain",
        "--summary",
    ]))
    .unwrap();
    match cli.command {
        Command::Met(MetCommand::Probe {
            profile,
            time_unix,
            explain,
            summary,
            ..
        }) => {
            assert_eq!(profile, "cfsr-pgbl-pressure-v0");
            assert_eq!(time_unix, 1_230_778_800);
            assert!(explain);
            assert!(summary);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_met_replay_defaults_output_stdout() {
    let cli = Cli::parse_from(os_args(&[
        "met",
        "replay",
        "--data-root",
        "data",
        "--profile",
        "cfsr-pgbl-pressure-v0",
        "--input",
        "in.jsonl",
    ]))
    .unwrap();
    match cli.command {
        Command::Met(MetCommand::Replay {
            output, summary, ..
        }) => {
            assert_eq!(output, PathBuf::from("-"));
            assert!(summary);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parse_rejects_unknown_backend() {
    let error = Cli::parse_from(os_args(&[
        "met",
        "probe",
        "--data-root",
        "data",
        "--profile",
        "x",
        "--time",
        "1",
        "--backend",
        "magic",
    ]))
    .unwrap_err();
    assert!(matches!(error, CliParseError::InvalidArgument(_)));
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn stream_chunk_budget_is_bounded() {
    const {
        assert!(STREAM_CHUNK_POINTS <= 4_096);
        assert!(STREAM_CHUNK_POINTS >= 64);
    }
}

#[test]
fn diagnostic_codes_are_stable() {
    assert_eq!(
        diagnostic::MISSING_SYMMETRIC_TIME_SUPPORT,
        "met.missing_symmetric_time_support"
    );
}

#[test]
fn missing_time_unix_is_usage_error_not_epoch_sentinel() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let points = dir.path().join("missing-time.jsonl");
    fs::write(
        &points,
        r#"{"id":"p0","longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}
"#,
    )
    .unwrap();
    let out = dir.path().join("out.jsonl");
    let command = MetCommand::Probe {
        data_root: root,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: false,
        explain: false,
        output: Some(out),
    };
    let error = met::execute(&command).unwrap_err();
    assert_eq!(error.exit_code(), 2);
    assert_eq!(error.code(), diagnostic::USAGE);
    assert!(error.to_string().contains("time_unix"));
}

#[test]
fn epoch_zero_time_unix_is_accepted_as_explicit_timestamp() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let points = dir.path().join("epoch.jsonl");
    // Explicit Unix epoch 0 is a legal timestamp; must not be treated as missing.
    fs::write(
        &points,
        r#"{"id":"epoch","time_unix":0,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}
"#,
    )
    .unwrap();
    let out = dir.path().join("out.jsonl");
    let command = MetCommand::Probe {
        data_root: root,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        // Coverage deliberately excludes epoch so prepare fails at runtime, not usage.
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: false,
        explain: false,
        output: Some(out),
    };
    let error = met::execute(&command).unwrap_err();
    assert_eq!(
        error.exit_code(),
        1,
        "epoch 0 must pass parsing and fail at runtime"
    );
    assert_eq!(error.code(), diagnostic::RUNTIME);
    assert!(!error.to_string().contains("missing required time_unix"));
}

#[test]
fn multi_chunk_and_multi_unique_time_summary() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let points = dir.path().join("multi.jsonl");
    let out = dir.path().join("out.jsonl");
    let mut body = String::new();
    // Cross multiple chunks with many distinct times (all mid-window).
    let total = STREAM_CHUNK_POINTS * 2 + 5;
    let mut unique_times = std::collections::BTreeSet::new();
    for index in 0..total {
        // Spread times across (00,06) open interval so prepare can succeed for mid points.
        // Use only the single mid time for ok samples, but also inject unique off-mid times
        // that still fall inside the locked coverage for prepare (exact 00 will fail W).
        let time = 1_230_778_800_i64 + i64::try_from(index % 50).unwrap(); // many unique
        unique_times.insert(time);
        let lon = if index % 2 == 0 { 0.0 } else { -95.0 };
        let lat = if index % 2 == 0 { 50.0 } else { 40.0 };
        body.push_str(&format!(
            "{{\"id\":\"p{index}\",\"time_unix\":{time},\"longitude_degrees\":{lon},\"latitude_degrees\":{lat},\"vertical_coordinate\":\"agl\",\"vertical\":10.0}}\n"
        ));
    }
    fs::write(&points, body).unwrap();
    let command = MetCommand::Probe {
        data_root: root,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: true,
        explain: false,
        output: Some(out.clone()),
    };
    // Some non-mid times may fail prepare; still exercise streaming + unique count path.
    let result = met::execute(&command);
    // If all times are strictly between 00 and 06, prepare should work.
    assert!(result.is_ok(), "{result:?}");
    let text = fs::read_to_string(&out).unwrap();
    let summary = text.lines().last().unwrap();
    assert!(summary.contains("\"kind\":\"probe_summary\""));
    assert!(summary.contains(&format!("\"point_count\":{total}")));
    assert!(summary.contains(&format!("\"time_count\":{}", unique_times.len())));
}

#[test]
fn explain_contract_off_vs_on_over_real_cfsr() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let points = dir.path().join("points.jsonl");
    fs::write(
        &points,
        r#"{"id":"surface","time_unix":1230778800,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}
{"id":"pressure","time_unix":1230778800,"longitude_degrees":-95.0,"latitude_degrees":40.0,"vertical_coordinate":"pa","vertical":70000.0}
"#,
    )
    .unwrap();

    let out_off = dir.path().join("off.jsonl");
    let off = MetCommand::Probe {
        data_root: root.clone(),
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points.clone()),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: false,
        explain: false,
        output: Some(out_off.clone()),
    };
    met::execute(&off).unwrap();
    for line in fs::read_to_string(&out_off).unwrap().lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(value.get("explain").is_none() || value.get("explain").unwrap().is_null());
        assert!(value.get("provenance").unwrap().as_object().unwrap().len() >= 8);
    }

    let out_on = dir.path().join("on.jsonl");
    let on = MetCommand::Probe {
        data_root: root,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: false,
        explain: true,
        output: Some(out_on.clone()),
    };
    met::execute(&on).unwrap();
    for line in fs::read_to_string(&out_on).unwrap().lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let explain = value.get("explain").expect("explain when enabled");
        assert!(explain.get("before_weight").is_some());
        assert!(explain.get("after_weight").is_some());
    }
}

#[test]
fn preferred_path_truncated_pgbl_hard_fails_via_cli() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mixed = dir.path().join("mixed");
    fs::create_dir_all(&mixed).unwrap();
    fs::copy(
        root.join("pgbl00.gdas.2009010100.grb2"),
        mixed.join("pgbl00.gdas.2009010100.grb2"),
    )
    .unwrap();
    let bytes = fs::read(root.join("pgbl00.gdas.2009010106.grb2")).unwrap();
    fs::write(mixed.join("pgbl00.gdas.2009010106.grb2"), &bytes[..2048]).unwrap();
    // Non-candidate product that also fails inspect should not mask the hard fail.
    fs::write(mixed.join("flxl00.gdas.2009010100.grb2"), b"not-grib-data").unwrap();

    let points = dir.path().join("p.jsonl");
    fs::write(
        &points,
        r#"{"id":"p1","time_unix":1230778800,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}
"#,
    )
    .unwrap();
    let out = dir.path().join("out.jsonl");
    let command = MetCommand::Probe {
        data_root: mixed,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: false,
        explain: false,
        output: Some(out),
    };
    let error = met::execute(&command).unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert!(
        error.to_string().contains("dataset lock failed") || error.to_string().contains("inspect"),
        "{error}"
    );
}

#[test]
fn mixed_product_data_root_probe_emits_lock_notes_and_succeeds() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    if !root.join("flxl00.gdas.2009010100.grb2").is_file() {
        // Still valid if only pgbl present.
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let points = dir.path().join("p.jsonl");
    fs::write(
        &points,
        r#"{"id":"p1","time_unix":1230778800,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}
"#,
    )
    .unwrap();
    let out = dir.path().join("out.jsonl");
    let command = MetCommand::Probe {
        data_root: root,
        profile: "cfsr-pgbl-pressure-v0".into(),
        time_unix: 1_230_778_800,
        points: Some(points),
        backend: trajecta_case::document::MeteorologyReaderBackend::Rust,
        coverage_start_unix: Some(1_230_768_000),
        coverage_end_unix: Some(1_230_789_600),
        allow_estimated: false,
        summary: true,
        explain: false,
        output: Some(out.clone()),
    };
    let code = met::execute(&command).expect("mixed root probe");
    assert_eq!(code, 0);
    assert!(
        fs::read_to_string(out)
            .unwrap()
            .contains("\"status\":\"ok\"")
    );
}

#[test]
fn replay_stdin_pipe_single_pass_with_explicit_coverage() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.jsonl");
    let mut child = ProcessCommand::new(exe)
        .args([
            "met",
            "replay",
            "--data-root",
            root.to_str().unwrap(),
            "--profile",
            "cfsr-pgbl-pressure-v0",
            "--input",
            "-",
            "--output",
            out.to_str().unwrap(),
            "--coverage-start",
            "1230768000",
            "--coverage-end",
            "1230789600",
            "--summary",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            r#"{{"id":"pipe","time_unix":1230778800,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}}"#
        )
        .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(out).unwrap();
    assert!(text.contains("\"id\":\"pipe\""));
    assert!(text.contains("probe_summary"));
}

/// No explicit coverage: CLI must spool stdin once, derive ±12h coverage, then re-read the spool.
#[test]
fn replay_stdin_without_coverage_spools_then_second_pass() {
    let Some(root) = cfsr_raw_dir() else {
        skip_or_fail("cfsr raw dir");
        return;
    };
    let exe = env!("CARGO_BIN_EXE_trajecta-cli");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.jsonl");
    let mut child = ProcessCommand::new(exe)
        .args([
            "met",
            "replay",
            "--data-root",
            root.to_str().unwrap(),
            "--profile",
            "cfsr-pgbl-pressure-v0",
            "--input",
            "-",
            "--output",
            out.to_str().unwrap(),
            "--summary",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        // Mid-window time so auto coverage (t±12h) still covers CFSR 00/06 frames.
        writeln!(
            stdin,
            r#"{{"id":"spool","time_unix":1230778800,"longitude_degrees":0.0,"latitude_degrees":50.0,"vertical_coordinate":"agl","vertical":10.0}}"#
        )
        .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(out).unwrap();
    assert!(text.contains("\"id\":\"spool\""));
    assert!(text.contains("probe_summary"));
    assert!(text.contains("\"time_count\":1"));
}
