#!/usr/bin/env python3
"""Run MIT-side FLEXPART oracle comparison for one or all frozen families.

The GPL harness and this comparison are deliberately separate gates. This
runner removes old subject/report files before invoking Rust, validates both
schemas and identities, and gives partial coverage, hard scientific failures,
and unavailable infrastructure distinct exit codes.

Exit codes (per family process; multi-family uses the worst):
  0 complete comparison passed hard gates
  1 one or more hard-gate rules failed
  2 external prerequisite/tooling blocked the comparison
  3 valid comparison is partial or otherwise incomplete
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/m3-oracle"
MANIFEST = ROOT / "testdata/REAL_MET_MANIFEST.json"
REGISTRY = ROOT / "testdata/M3_TOLERANCES.v1.json"
ORACLE_SCHEMA = ROOT / "testdata/M3_FLEXPART_ORACLE.schema.json"
REPORT_SCHEMA = ROOT / "testdata/M3_COMPARISON_REPORT.schema.json"
SUMMARY = OUT / "COMPARISON_SUMMARY.json"
ROLLUP = OUT / "FAMILY_ROLLUP.json"

FAMILIES: dict[str, dict[str, Any]] = {
    "era5_pressure": {
        "oracle": OUT / "artifacts/era5_pressure_oracle.json",
        "query": OUT / "queries/era5_pressure_queries.json",
        "profile": ROOT / "crates/trajecta-met/profiles/era5_cf_pressure_netcdf_v0.yaml",
        "profile_name": "era5-cf-pressure-netcdf-v0",
        "data_root": ROOT / "target/test-data/era5-cds-pressure-official/ready",
        "subject": OUT / "artifacts/era5_pressure_trajecta_subject.json",
        "common_report": OUT / "reports/era5_pressure_common_semantics.json",
        "difference_report": OUT / "reports/era5_pressure_difference_report.json",
        "stdout_log": OUT / "logs/era5_pressure_oracle_comparison.stdout.log",
        "stderr_log": OUT / "logs/era5_pressure_oracle_comparison.stderr.log",
    },
    "era5_hybrid": {
        "oracle": OUT / "artifacts/era5_hybrid_oracle.json",
        "query": OUT / "queries/era5_hybrid_queries.json",
        "profile": ROOT / "crates/trajecta-met/profiles/era5_cds_hybrid137_v0.yaml",
        "profile_name": "era5-cds-hybrid137-v0",
        "data_root": ROOT / "target/test-data/era5-cds-hybrid137-official/ready",
        "subject": OUT / "artifacts/era5_hybrid_trajecta_subject.json",
        "common_report": OUT / "reports/era5_hybrid_common_semantics.json",
        "difference_report": OUT / "reports/era5_hybrid_difference_report.json",
        "stdout_log": OUT / "logs/era5_hybrid_oracle_comparison.stdout.log",
        "stderr_log": OUT / "logs/era5_hybrid_oracle_comparison.stderr.log",
    },
    "cfsr_pressure": {
        "oracle": OUT / "artifacts/cfsr_pressure_oracle.json",
        "query": OUT / "queries/cfsr_pressure_queries.json",
        "profile": ROOT / "crates/trajecta-met/profiles/cfsr_pgbl_pressure_v0.yaml",
        "profile_name": "cfsr-pgbl-pressure-v0",
        "data_root": ROOT / "target/test-data/cfsr-ncei-pgbl-official",
        "subject": OUT / "artifacts/cfsr_pressure_trajecta_subject.json",
        "common_report": OUT / "reports/cfsr_pressure_common_semantics.json",
        "difference_report": OUT / "reports/cfsr_pressure_difference_report.json",
        "stdout_log": OUT / "logs/cfsr_pressure_oracle_comparison.stdout.log",
        "stderr_log": OUT / "logs/cfsr_pressure_oracle_comparison.stderr.log",
    },
}

EXIT_BY_STATUS = {
    "passed": 0,
    "hard_gate_failed": 1,
    "external_blocked": 2,
    "partial": 3,
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def validate_schema(instance: dict[str, Any], schema_path: Path) -> None:
    try:
        from jsonschema import Draft202012Validator
    except ImportError as error:
        raise RuntimeError(
            "jsonschema with Draft 2020-12 support is required; no weak fallback"
        ) from error
    schema = load_json(schema_path)
    errors = sorted(
        Draft202012Validator(schema).iter_errors(instance),
        key=lambda error: list(error.absolute_path),
    )
    if errors:
        details = "; ".join(
            f"{list(error.absolute_path)}: {error.message}" for error in errors[:12]
        )
        raise ValueError(f"schema validation failed for {schema_path.name}: {details}")


def require_path(path: Path, *, directory: bool = False) -> None:
    exists = path.is_dir() if directory else path.is_file()
    if not exists:
        kind = "directory" if directory else "file"
        raise FileNotFoundError(f"required {kind} missing: {path}")


def classify(
    oracle_status: str,
    report_status: str,
    hard_failures: int,
    rust_returncode: int,
) -> str:
    if hard_failures > 0 or report_status == "failed":
        return "hard_gate_failed"
    # Any genuinely unregistered variant remains partial, not infrastructure failure.
    if report_status == "unvalidated":
        return "partial"
    if rust_returncode not in (0, 3):
        return "external_blocked"
    if oracle_status != "complete" or report_status == "incomplete":
        return "partial"
    if report_status == "passed" and rust_returncode == 0:
        return "passed"
    return "external_blocked"


def artifact_entry(path: Path) -> dict[str, Any] | None:
    if not path.is_file():
        return None
    stat = path.stat()
    return {
        "path": path.relative_to(ROOT).as_posix(),
        "size": stat.st_size,
        "sha256": sha256_file(path),
        "mtime_unix_ns": stat.st_mtime_ns,
    }


def verify_fresh(path: Path, started_ns: int) -> None:
    require_path(path)
    if path.stat().st_mtime_ns + 2_000_000_000 < started_ns:
        raise RuntimeError(f"stale artifact produced: {path}")


def scope_counts(records: list[dict[str, Any]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for record in records:
        scope = str(record.get("sample_scope") or "unknown")
        counts[scope] = counts.get(scope, 0) + 1
    return counts


def finish_family(
    *,
    family: str,
    run_id: str,
    started_at: str,
    status: str,
    reason: str,
    oracle_status: str | None,
    rust_returncode: int | None = None,
    common_report: dict[str, Any] | None = None,
    difference_report: dict[str, Any] | None = None,
    subject: dict[str, Any] | None = None,
    paths: dict[str, Path],
) -> dict[str, Any]:
    hard_failures = 0
    common_reported_rows = 0
    difference_failures = 0
    if common_report is not None:
        hard_failures = sum(
            row.get("decision") == "hard_gate" and row.get("status") == "fail"
            for row in common_report.get("results", [])
        )
        common_reported_rows = sum(
            row.get("decision") == "report_only" and row.get("status") == "reported"
            for row in common_report.get("results", [])
        )
    if difference_report is not None:
        difference_failures = sum(
            row.get("status") == "fail" for row in difference_report.get("results", [])
        )
    summary = {
        "run_id": run_id,
        "family": family,
        "started_at_utc": started_at,
        "finished_at_utc": datetime.now(timezone.utc).isoformat(),
        "status": status,
        "reason": reason,
        "oracle_status": oracle_status,
        "rust_returncode": rust_returncode,
        "hard_gate_failures": hard_failures,
        "common_reported_rows": common_reported_rows,
        "difference_failures": difference_failures,
        "common_report_status": None
        if common_report is None
        else common_report.get("status"),
        "difference_report_status": None
        if difference_report is None
        else difference_report.get("status"),
        "subject_records": None if subject is None else len(subject.get("records", [])),
        "artifacts": [
            entry
            for entry in (
                artifact_entry(paths["oracle"]),
                artifact_entry(paths["subject"]),
                artifact_entry(paths["common_report"]),
                artifact_entry(paths["difference_report"]),
            )
            if entry is not None
        ],
    }
    return summary


def run_family(family: str) -> tuple[int, dict[str, Any]]:
    paths = FAMILIES[family]
    run_id = str(uuid.uuid4())
    started_at = datetime.now(timezone.utc).isoformat()
    oracle_status: str | None = None
    try:
        for path in (
            paths["oracle"],
            paths["query"],
            MANIFEST,
            paths["profile"],
            REGISTRY,
            ORACLE_SCHEMA,
            REPORT_SCHEMA,
        ):
            require_path(path)
        require_path(paths["data_root"], directory=True)
        oracle = load_json(paths["oracle"])
        validate_schema(oracle, ORACLE_SCHEMA)
        oracle_status = str(oracle.get("status"))
        if oracle.get("input", {}).get("dataset_family") != family:
            raise ValueError(
                f"oracle dataset_family={oracle.get('input', {}).get('dataset_family')} "
                f"does not match requested {family}"
            )
        if len(oracle.get("records", [])) != 75:
            raise ValueError(f"{family} oracle must contain exactly 75 records")
        counts = scope_counts(oracle.get("records", []))
        expected = {
            "native_anchor": 27,
            "interpolated_common": 18,
            "surface_layer": 12,
            "modern_difference": 18,
        }
        for scope, want in expected.items():
            if counts.get(scope, 0) != want:
                raise ValueError(f"{family} scope counts drifted: {counts}")
    except (FileNotFoundError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        summary = finish_family(
            family=family,
            run_id=run_id,
            started_at=started_at,
            status="external_blocked",
            reason=str(error),
            oracle_status=oracle_status,
            paths=paths,
        )
        return EXIT_BY_STATUS["external_blocked"], summary

    for path in (
        paths["subject"],
        paths["common_report"],
        paths["difference_report"],
        paths["stdout_log"],
        paths["stderr_log"],
    ):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.unlink(missing_ok=True)

    started_ns = time.time_ns()
    command = [
        "cargo",
        "run",
        "--offline",
        "-p",
        "trajecta-met",
        "--example",
        "adjudicate_flexpart_oracle",
        "--",
        "--oracle",
        str(paths["oracle"]),
        "--query",
        str(paths["query"]),
        "--manifest",
        str(MANIFEST),
        "--profile-path",
        str(paths["profile"]),
        "--profile-name",
        paths["profile_name"],
        "--data-root",
        str(paths["data_root"]),
        "--registry",
        str(REGISTRY),
        "--subject-out",
        str(paths["subject"]),
        "--report-out",
        str(paths["common_report"]),
        "--difference-report-out",
        str(paths["difference_report"]),
    ]
    print("+", " ".join(command), flush=True)
    try:
        process = subprocess.run(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
            env={**os.environ, "CARGO_TERM_COLOR": "never"},
        )
    except OSError as error:
        summary = finish_family(
            family=family,
            run_id=run_id,
            started_at=started_at,
            status="external_blocked",
            reason=f"failed to launch Rust comparison: {error}",
            oracle_status=oracle_status,
            paths=paths,
        )
        return EXIT_BY_STATUS["external_blocked"], summary

    paths["stdout_log"].write_text(process.stdout or "", encoding="utf-8", newline="\n")
    paths["stderr_log"].write_text(process.stderr or "", encoding="utf-8", newline="\n")
    if process.stdout:
        print(process.stdout[-4000:])
    if process.stderr:
        print(process.stderr[-4000:], file=sys.stderr)

    try:
        verify_fresh(paths["subject"], started_ns)
        verify_fresh(paths["common_report"], started_ns)
        verify_fresh(paths["difference_report"], started_ns)
        subject = load_json(paths["subject"])
        common_report = load_json(paths["common_report"])
        difference_report = load_json(paths["difference_report"])
        validate_schema(common_report, REPORT_SCHEMA)
        validate_schema(difference_report, REPORT_SCHEMA)
        oracle_sha = sha256_file(paths["oracle"])
        if subject.get("reference_sha256") != oracle_sha:
            raise ValueError("subject reference SHA does not pin this oracle artifact")
        if common_report.get("comparison", {}).get("reference_sha256") != oracle_sha:
            raise ValueError("common report reference SHA does not pin this oracle")
        if difference_report.get("comparison", {}).get("reference_sha256") != oracle_sha:
            raise ValueError("difference report reference SHA does not pin this oracle")
        if subject.get("query_sha256") != oracle["input"]["query_sha256"]:
            raise ValueError("subject query SHA differs from frozen oracle identity")
        expected_ok = sum(row.get("status") == "ok" for row in oracle["records"])
        if len(subject.get("records", [])) != expected_ok:
            raise ValueError(
                "subject coverage differs from oracle ok coverage: "
                f"subject={len(subject.get('records', []))} oracle_ok={expected_ok}"
            )
        # Hard-gate and difference targets must stay separate.
        common_target = common_report.get("comparison", {}).get("comparison_target")
        difference_target = difference_report.get("comparison", {}).get(
            "comparison_target"
        )
        if common_target != "flexpart_common_semantics":
            raise ValueError(f"common report target drifted: {common_target}")
        if difference_target != "flexpart_difference_report":
            raise ValueError(f"difference report target drifted: {difference_target}")
    except (FileNotFoundError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        summary = finish_family(
            family=family,
            run_id=run_id,
            started_at=started_at,
            status="external_blocked",
            reason=str(error),
            oracle_status=oracle_status,
            rust_returncode=process.returncode,
            paths=paths,
        )
        return EXIT_BY_STATUS["external_blocked"], summary

    hard_failures = sum(
        row.get("decision") == "hard_gate" and row.get("status") == "fail"
        for row in common_report.get("results", [])
    )
    common_reported_rows = sum(
        row.get("decision") == "report_only" and row.get("status") == "reported"
        for row in common_report.get("results", [])
    )
    status = classify(
        oracle_status or "failed",
        str(common_report.get("status")),
        hard_failures,
        process.returncode,
    )
    reasons = {
        "passed": (
            "complete oracle comparison passed all registered hard gates; "
            f"{common_reported_rows} common-semantics rows remain explicit report-only"
        ),
        "hard_gate_failed": f"{hard_failures} hard-gate result rows failed",
        "partial": (
            "comparison is valid but incomplete/unvalidated "
            f"(common_report.status={common_report.get('status')})"
        ),
        "external_blocked": (
            f"Rust comparison returned {process.returncode} without a classifiable result"
        ),
    }
    summary = finish_family(
        family=family,
        run_id=run_id,
        started_at=started_at,
        status=status,
        reason=reasons[status],
        oracle_status=oracle_status,
        rust_returncode=process.returncode,
        common_report=common_report,
        difference_report=difference_report,
        subject=subject,
        paths=paths,
    )
    return EXIT_BY_STATUS[status], summary


def write_rollup(family_summaries: list[dict[str, Any]]) -> None:
    worst = 0
    for summary in family_summaries:
        worst = max(worst, EXIT_BY_STATUS.get(summary["status"], 2))
    rollup = {
        "schema_hint": "trajecta.m3.oracle_family_rollup/v1",
        "generated_at_utc": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "worst_exit_code": worst,
        "families": family_summaries,
        "note": (
            "report_only difference failures never rewrite registry thresholds; "
            "hard_gate_failed is reserved for flexpart_common_semantics."
        ),
    }
    OUT.mkdir(parents=True, exist_ok=True)
    ROLLUP.write_text(json.dumps(rollup, indent=2) + "\n", encoding="utf-8", newline="\n")
    SUMMARY.write_text(json.dumps(rollup, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps({"rollup": str(ROLLUP), "worst_exit_code": worst, "families": [
        {"family": s["family"], "status": s["status"], "reason": s["reason"]}
        for s in family_summaries
    ]}, indent=2))


def self_test() -> int:
    cases = [
        (("complete", "passed", 0, 0), "passed"),
        (("partial", "incomplete", 0, 3), "partial"),
        (("partial", "incomplete", 2, 1), "hard_gate_failed"),
        (("complete", "unvalidated", 0, 2), "partial"),
    ]
    for arguments, expected in cases:
        actual = classify(*arguments)
        if actual != expected:
            raise AssertionError(f"classify{arguments}={actual}, expected {expected}")
    print("run_m3_oracle_comparison self-test: OK")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--family",
        action="append",
        choices=sorted(FAMILIES),
        help="Family to run (repeatable). Default: all three.",
    )
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    families = args.family or sorted(FAMILIES)
    summaries: list[dict[str, Any]] = []
    worst = 0
    for family in families:
        code, summary = run_family(family)
        summaries.append(summary)
        worst = max(worst, code)
    write_rollup(summaries)
    return worst


if __name__ == "__main__":
    raise SystemExit(main())
