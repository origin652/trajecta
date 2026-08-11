#!/usr/bin/env python3
"""Run and compare the M4-A3 pure-Rust/native ozone trajectory matrix.

This is an A-owned evidence runner.  It executes the same frozen 1,000-particle,
two-step real-data case once with the pure-Rust reader and once with the native
reader, then compares logical SQLite rows.  Full trajectory differences are
report-only; missing coverage, non-finite values, corrupt artifacts, incomplete
runs, or a silently wrong backend are blockers.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import shutil
import sqlite3
import struct
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

from measure_rust_native_diff import native_env


ROOT = Path(__file__).resolve().parents[1]
FAMILIES = ("era5-pressure", "era5-hybrid", "cfsr-pressure")
DIRECTIONS = ("forward", "backward")
BACKENDS = ("rust", "native")
TEST_FILTER = "real_stratospheric_ozone_three_families_forward_backward"
TEST_NAME = "m4_a3_real_data"

FEATURE_BY_FAMILY = {
    "era5-pressure": "trajecta-met/native-netcdf",
    "era5-hybrid": "trajecta-met/native-netcdf",
    "cfsr-pressure": "trajecta-met/native-eccodes",
}


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, indent=2, ensure_ascii=False, allow_nan=False) + "\n",
        encoding="utf-8",
    )


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def host_identity() -> dict[str, Any]:
    identity: dict[str, Any] = {
        "collected_at_utc": utc_now(),
        "python": sys.version,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "repo_root": str(ROOT),
    }
    for command, key in (
        (["rustc", "-Vv"], "rustc"),
        (["cargo", "-V"], "cargo"),
        (["git", "rev-parse", "HEAD"], "git_head"),
        (["git", "status", "--short"], "git_status_short"),
    ):
        try:
            identity[key] = subprocess.check_output(
                command,
                cwd=ROOT,
                text=True,
                stderr=subprocess.STDOUT,
            ).strip()
        except Exception as error:  # noqa: BLE001 - evidence collection
            identity[key] = f"ERROR: {error}"
    return identity


def cell_directory(
    artifact_root: Path,
    particles: int,
    family: str,
    direction: str,
    backend: str,
) -> Path:
    return artifact_root / "windows" / f"p{particles}" / family / direction / backend


def cargo_command(family: str) -> list[str]:
    return [
        "cargo",
        "test",
        "--offline",
        "--release",
        "-p",
        "trajecta-core",
        "--features",
        FEATURE_BY_FAMILY[family],
        "--test",
        TEST_NAME,
        TEST_FILTER,
        "--",
        "--ignored",
        "--nocapture",
        "--exact",
    ]


def run_cell(
    artifact_root: Path,
    *,
    particles: int,
    duration_seconds: int,
    family: str,
    direction: str,
    backend: str,
) -> dict[str, Any]:
    output = cell_directory(artifact_root, particles, family, direction, backend)
    output.mkdir(parents=True, exist_ok=False)
    env = native_env(with_eccodes=family == "cfsr-pressure")
    env.update(
        {
            "TRAJECTA_M4_A3_PARTICLES": str(particles),
            "TRAJECTA_M4_A3_DURATION_SECONDS": str(duration_seconds),
            "TRAJECTA_M4_A3_FAMILY": family,
            "TRAJECTA_M4_A3_DIRECTION": direction,
            "TRAJECTA_M4_A3_READER_BACKEND": backend,
            "TRAJECTA_M4_A3_ARTIFACT_DIR": str(output.resolve()),
            "TRAJECTA_REQUIRE_REAL_MET": "1",
            "RUST_BACKTRACE": "1",
        }
    )
    command = cargo_command(family)
    print(
        f"[run] {family}/{direction}/{backend} -> {output.relative_to(ROOT)}",
        flush=True,
    )
    started = time.monotonic()
    process = subprocess.run(command, cwd=ROOT, env=env, text=True, capture_output=True)
    elapsed = time.monotonic() - started
    (output / "stdout.log").write_text(
        process.stdout or "", encoding="utf-8", errors="replace"
    )
    (output / "stderr.log").write_text(
        process.stderr or "", encoding="utf-8", errors="replace"
    )
    result = {
        "family": family,
        "direction": direction,
        "backend": backend,
        "feature": FEATURE_BY_FAMILY[family],
        "command": command,
        "exit_code": process.returncode,
        "elapsed_seconds": elapsed,
        "artifact_directory": str(output.relative_to(ROOT).as_posix()),
        "native_environment": {
            key: env.get(key)
            for key in (
                "NETCDF_DIR",
                "ECCODES_DEFINITION_PATH",
                "LIBCLANG_PATH",
                "PKG_CONFIG_PATH",
            )
            if env.get(key)
        },
    }
    write_json(output / "CELL_EXECUTION.json", result)
    return result


def locate_run(cell: Path, expected_backend: str) -> dict[str, Any]:
    blockers: list[str] = []
    summary_path = cell / "M4_A3_REAL_MATRIX_SUMMARY.json"
    if not summary_path.is_file():
        return {"blockers": ["missing matrix summary"]}
    summary = read_json(summary_path)
    runs = summary.get("runs") if isinstance(summary, dict) else None
    if not isinstance(runs, list) or len(runs) != 1:
        return {"blockers": [f"expected one summary run, found {runs!r}"]}
    evidence = runs[0]
    if evidence.get("reader_backend") != expected_backend:
        blockers.append(
            "summary reader backend "
            f"{evidence.get('reader_backend')!r} != {expected_backend!r}"
        )
    relative_manifest = evidence.get("manifest_relative_path")
    if not isinstance(relative_manifest, str):
        return {"blockers": blockers + ["missing manifest_relative_path"]}
    manifest_path = cell / Path(relative_manifest)
    sqlite_path = manifest_path.parent / "particles.sqlite"
    bundle_path = manifest_path.parent / "provenance-bundle.json"
    for label, path in (
        ("manifest", manifest_path),
        ("sqlite", sqlite_path),
        ("provenance bundle", bundle_path),
    ):
        if not path.is_file():
            blockers.append(f"missing {label}: {path}")
    manifest: dict[str, Any] = {}
    if manifest_path.is_file():
        manifest = read_json(manifest_path)
        if manifest.get("status") != "complete":
            blockers.append(f"manifest status={manifest.get('status')!r}")
        abnormal = (manifest.get("terminations") or {}).get("abnormal_count")
        if abnormal != 0:
            blockers.append(f"manifest abnormal_count={abnormal!r}")
        readers = (manifest.get("execution") or {}).get("reader_backends") or {}
        if set(readers.values()) != {expected_backend}:
            blockers.append(f"manifest reader_backends={readers!r}")
    if evidence.get("status") != "complete":
        blockers.append(f"summary status={evidence.get('status')!r}")
    if evidence.get("abnormal_terminations") != 0:
        blockers.append(
            f"summary abnormal_terminations={evidence.get('abnormal_terminations')!r}"
        )
    return {
        "blockers": blockers,
        "summary_path": summary_path,
        "summary": summary,
        "run_evidence": evidence,
        "manifest_path": manifest_path,
        "manifest": manifest,
        "sqlite_path": sqlite_path,
        "bundle_path": bundle_path,
        "sha256": {
            "summary": sha256_file(summary_path),
            "manifest": sha256_file(manifest_path) if manifest_path.is_file() else None,
            "sqlite": sha256_file(sqlite_path) if sqlite_path.is_file() else None,
            "provenance_bundle": (
                sha256_file(bundle_path) if bundle_path.is_file() else None
            ),
        },
    }


def ordered_f64(value: float) -> int:
    if value == 0.0:
        value = 0.0
    bits = struct.unpack(">Q", struct.pack(">d", value))[0]
    if bits & (1 << 63):
        return (~bits) & ((1 << 64) - 1)
    return bits | (1 << 63)


def empty_numeric_metrics() -> dict[str, Any]:
    return {
        "paired_count": 0,
        "value_presence_mismatch_count": 0,
        "nonfinite_count": 0,
        "exact_mismatch_count": 0,
        "max_abs": 0.0,
        "max_rel": 0.0,
        "max_ulps": 0,
        "worst_abs_key": None,
        "worst_rel_key": None,
        "worst_ulp_key": None,
    }


def update_numeric_metrics(
    metrics: dict[str, Any], key: tuple[Any, ...], rust: Any, native: Any
) -> None:
    if rust is None or native is None:
        if rust is not native:
            metrics["value_presence_mismatch_count"] += 1
        return
    rust_value = float(rust)
    native_value = float(native)
    metrics["paired_count"] += 1
    if not math.isfinite(rust_value) or not math.isfinite(native_value):
        metrics["nonfinite_count"] += 1
        return
    if rust_value != native_value:
        metrics["exact_mismatch_count"] += 1
    absolute = abs(rust_value - native_value)
    relative = absolute / max(abs(rust_value), abs(native_value), 1.0e-300)
    ulps = abs(ordered_f64(rust_value) - ordered_f64(native_value))
    display_key = list(key)
    if absolute > metrics["max_abs"]:
        metrics["max_abs"] = absolute
        metrics["worst_abs_key"] = display_key
    if relative > metrics["max_rel"]:
        metrics["max_rel"] = relative
        metrics["worst_rel_key"] = display_key
    if ulps > metrics["max_ulps"]:
        metrics["max_ulps"] = ulps
        metrics["worst_ulp_key"] = display_key


def rows_by_key(
    connection: sqlite3.Connection, query: str, key_columns: int
) -> dict[tuple[Any, ...], tuple[Any, ...]]:
    rows: dict[tuple[Any, ...], tuple[Any, ...]] = {}
    for row in connection.execute(query):
        key = tuple(row[:key_columns])
        if key in rows:
            raise ValueError(f"duplicate logical SQLite key: {key!r}")
        rows[key] = tuple(row[key_columns:])
    return rows


def compare_rows(
    rust: dict[tuple[Any, ...], tuple[Any, ...]],
    native: dict[tuple[Any, ...], tuple[Any, ...]],
    *,
    value_columns: list[str],
    numeric_columns: Iterable[str],
) -> dict[str, Any]:
    numeric = set(numeric_columns)
    rust_keys = set(rust)
    native_keys = set(native)
    only_rust = sorted(rust_keys - native_keys)
    only_native = sorted(native_keys - rust_keys)
    common = sorted(rust_keys & native_keys)
    categorical_mismatches: dict[str, int] = {
        column: 0 for column in value_columns if column not in numeric
    }
    numeric_metrics = {
        column: empty_numeric_metrics() for column in value_columns if column in numeric
    }
    for key in common:
        rust_values = rust[key]
        native_values = native[key]
        for index, column in enumerate(value_columns):
            if column in numeric:
                update_numeric_metrics(
                    numeric_metrics[column], key, rust_values[index], native_values[index]
                )
            elif rust_values[index] != native_values[index]:
                categorical_mismatches[column] += 1
    return {
        "rust_row_count": len(rust),
        "native_row_count": len(native),
        "common_key_count": len(common),
        "only_rust_key_count": len(only_rust),
        "only_native_key_count": len(only_native),
        "only_rust_keys_first_20": [list(key) for key in only_rust[:20]],
        "only_native_keys_first_20": [list(key) for key in only_native[:20]],
        "categorical_mismatch_counts": categorical_mismatches,
        "numeric": numeric_metrics,
    }


TABLES: dict[str, dict[str, Any]] = {
    "particle": {
        "query": """
            SELECT particle_id, population_id, origin_kind, origin_event_id,
                   origin_domain_id, origin_boundary_face_id,
                   birth_seconds, birth_nanosecond,
                   dry_air_mass_kg
            FROM particle ORDER BY particle_id
        """,
        "keys": 1,
        "columns": [
            "population_id",
            "origin_kind",
            "origin_event_id",
            "origin_domain_id",
            "origin_boundary_face_id",
            "birth_seconds",
            "birth_nanosecond",
            "dry_air_mass_kg",
        ],
        "numeric": ["dry_air_mass_kg"],
    },
    "particle_mass": {
        "query": """
            SELECT particle_id, substance_id, mass_kg
            FROM particle_mass ORDER BY particle_id, substance_id
        """,
        "keys": 2,
        "columns": ["mass_kg"],
        "numeric": ["mass_kg"],
    },
    "particle_adjoint": {
        "query": """
            SELECT particle_id, substance_id, adjoint_weight, source_sensitivity
            FROM particle_adjoint ORDER BY particle_id, substance_id
        """,
        "keys": 2,
        "columns": ["adjoint_weight", "source_sensitivity"],
        "numeric": ["adjoint_weight", "source_sensitivity"],
    },
    "output_event": {
        "query": """
            SELECT event_sequence, physical_seconds, physical_nanosecond, event_kind
            FROM output_event ORDER BY event_sequence
        """,
        "keys": 1,
        "columns": ["physical_seconds", "physical_nanosecond", "event_kind"],
        "numeric": [],
    },
    "particle_state": {
        "query": """
            SELECT particle_id, sample_sequence, event_sequence,
                   physical_seconds, physical_nanosecond,
                   integration_offset_ns, elapsed_age_ns,
                   longitude_degrees, latitude_degrees, height_asl_m,
                   particle_status, termination_reason,
                   eastward_wind_m_s, northward_wind_m_s,
                   geometric_vertical_velocity_m_s,
                   air_pressure_pa, air_temperature_k,
                   wind_validity, wind_quality,
                   pressure_validity, pressure_quality,
                   temperature_validity, temperature_quality
            FROM particle_state ORDER BY particle_id, sample_sequence
        """,
        "keys": 2,
        "columns": [
            "event_sequence",
            "physical_seconds",
            "physical_nanosecond",
            "integration_offset_ns",
            "elapsed_age_ns",
            "longitude_degrees",
            "latitude_degrees",
            "height_asl_m",
            "particle_status",
            "termination_reason",
            "eastward_wind_m_s",
            "northward_wind_m_s",
            "geometric_vertical_velocity_m_s",
            "air_pressure_pa",
            "air_temperature_k",
            "wind_validity",
            "wind_quality",
            "pressure_validity",
            "pressure_quality",
            "temperature_validity",
            "temperature_quality",
        ],
        "numeric": [
            "longitude_degrees",
            "latitude_degrees",
            "height_asl_m",
            "eastward_wind_m_s",
            "northward_wind_m_s",
            "geometric_vertical_velocity_m_s",
            "air_pressure_pa",
            "air_temperature_k",
        ],
    },
    "termination": {
        "query": """
            SELECT particle_id, reason, classification,
                   physical_seconds, physical_nanosecond, intersection_fraction
            FROM termination ORDER BY particle_id
        """,
        "keys": 1,
        "columns": [
            "reason",
            "classification",
            "physical_seconds",
            "physical_nanosecond",
            "intersection_fraction",
        ],
        "numeric": ["intersection_fraction"],
    },
}


def compare_pair(rust_run: dict[str, Any], native_run: dict[str, Any]) -> dict[str, Any]:
    blockers = list(rust_run["blockers"]) + list(native_run["blockers"])
    tables: dict[str, Any] = {}
    rust_sqlite = rust_run.get("sqlite_path")
    native_sqlite = native_run.get("sqlite_path")
    if not isinstance(rust_sqlite, Path) or not isinstance(native_sqlite, Path):
        blockers.append("missing SQLite path for comparison")
    elif rust_sqlite.is_file() and native_sqlite.is_file():
        with sqlite3.connect(f"file:{rust_sqlite.as_posix()}?mode=ro", uri=True) as rust_db:
            with sqlite3.connect(
                f"file:{native_sqlite.as_posix()}?mode=ro", uri=True
            ) as native_db:
                for label, connection in (("rust", rust_db), ("native", native_db)):
                    integrity = connection.execute("PRAGMA integrity_check").fetchone()
                    if integrity != ("ok",):
                        blockers.append(f"{label} integrity_check={integrity!r}")
                for table, specification in TABLES.items():
                    rust_rows = rows_by_key(
                        rust_db, specification["query"], specification["keys"]
                    )
                    native_rows = rows_by_key(
                        native_db, specification["query"], specification["keys"]
                    )
                    comparison = compare_rows(
                        rust_rows,
                        native_rows,
                        value_columns=specification["columns"],
                        numeric_columns=specification["numeric"],
                    )
                    tables[table] = comparison
                    if comparison["only_rust_key_count"]:
                        blockers.append(f"{table}: Rust-only logical rows")
                    if comparison["only_native_key_count"]:
                        blockers.append(f"{table}: native-only logical rows")
                    for column, metrics in comparison["numeric"].items():
                        if metrics["nonfinite_count"]:
                            blockers.append(f"{table}.{column}: non-finite values")
                        if metrics["value_presence_mismatch_count"]:
                            blockers.append(f"{table}.{column}: value-presence mismatch")

    rust_manifest = rust_run.get("manifest") or {}
    native_manifest = native_run.get("manifest") or {}
    manifest_comparison = {
        "case_name_equal": rust_manifest.get("case_name")
        == native_manifest.get("case_name"),
        "status_equal": rust_manifest.get("status") == native_manifest.get("status"),
        "dataset_content_sha256_equal": (rust_manifest.get("inputs") or {}).get(
            "dataset_content_sha256"
        )
        == (native_manifest.get("inputs") or {}).get("dataset_content_sha256"),
        "numerical_contract_equal": rust_manifest.get("numerical")
        == native_manifest.get("numerical"),
        "termination_summary_equal": rust_manifest.get("terminations")
        == native_manifest.get("terminations"),
    }
    if not manifest_comparison["dataset_content_sha256_equal"]:
        blockers.append("Rust/native dataset content identities differ")
    return {
        "status": "incomplete" if blockers else "reported",
        "adjudication": "report_only",
        "blockers": sorted(set(blockers)),
        "manifest": manifest_comparison,
        "tables": tables,
        "artifacts": {
            "rust": {
                "summary": str(rust_run.get("summary_path", "")),
                "manifest": str(rust_run.get("manifest_path", "")),
                "sqlite": str(rust_run.get("sqlite_path", "")),
                "sha256": rust_run.get("sha256"),
            },
            "native": {
                "summary": str(native_run.get("summary_path", "")),
                "manifest": str(native_run.get("manifest_path", "")),
                "sqlite": str(native_run.get("sqlite_path", "")),
                "sha256": native_run.get("sha256"),
            },
        },
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifact-root",
        type=Path,
        default=ROOT / "target" / "m4-a3" / "native-diff",
    )
    parser.add_argument("--particles", type=int, default=1_000)
    parser.add_argument("--duration-seconds", type=int, default=600)
    parser.add_argument("--families", nargs="+", choices=FAMILIES, default=list(FAMILIES))
    parser.add_argument(
        "--directions", nargs="+", choices=DIRECTIONS, default=list(DIRECTIONS)
    )
    parser.add_argument("--compare-only", action="store_true")
    parser.add_argument("--force", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.particles <= 0 or args.duration_seconds <= 0:
        raise SystemExit("particles and duration must be positive")
    artifact_root = args.artifact_root.resolve()
    expected_root = (ROOT / "target" / "m4-a3").resolve()
    if expected_root not in artifact_root.parents:
        raise SystemExit(f"artifact root must remain under {expected_root}")
    matrix_root = artifact_root / "windows" / f"p{args.particles}"
    if not args.compare_only and matrix_root.exists():
        if not args.force:
            raise SystemExit(f"artifact matrix already exists: {matrix_root}; use --force")
        shutil.rmtree(matrix_root)

    executions: list[dict[str, Any]] = []
    if not args.compare_only:
        for family in args.families:
            for direction in args.directions:
                for backend in BACKENDS:
                    executions.append(
                        run_cell(
                            artifact_root,
                            particles=args.particles,
                            duration_seconds=args.duration_seconds,
                            family=family,
                            direction=direction,
                            backend=backend,
                        )
                    )

    pairs: list[dict[str, Any]] = []
    global_blockers: list[str] = []
    for family in args.families:
        for direction in args.directions:
            rust_cell = cell_directory(
                artifact_root, args.particles, family, direction, "rust"
            )
            native_cell = cell_directory(
                artifact_root, args.particles, family, direction, "native"
            )
            rust_run = locate_run(rust_cell, "rust")
            native_run = locate_run(native_cell, "native")
            comparison = compare_pair(rust_run, native_run)
            comparison.update({"family": family, "direction": direction})
            pairs.append(comparison)
            global_blockers.extend(
                f"{family}/{direction}: {blocker}"
                for blocker in comparison["blockers"]
            )

    failed_executions = [
        f"{item['family']}/{item['direction']}/{item['backend']}: exit {item['exit_code']}"
        for item in executions
        if item["exit_code"] != 0
    ]
    global_blockers.extend(failed_executions)
    report = {
        "schema_version": "trajecta.m4-a3-rust-native-diff/v1",
        "generated_at_utc": utc_now(),
        "status": "incomplete" if global_blockers else "measured_all_pairs",
        "adjudication": {
            "trajectory_differences": "report_only",
            "hard_requirements": [
                "both executions complete",
                "abnormal_count == 0",
                "requested reader backend recorded in manifest",
                "same frozen dataset content identity",
                "complete logical-row key coverage",
                "finite paired numeric values",
                "SQLite integrity_check == ok",
            ],
        },
        "matrix": {
            "particles": args.particles,
            "duration_seconds": args.duration_seconds,
            "numerical_steps": (args.duration_seconds + 299) // 300,
            "families": args.families,
            "directions": args.directions,
            "backends": list(BACKENDS),
        },
        "host": host_identity(),
        "executions": executions,
        "pairs": pairs,
        "blockers": sorted(set(global_blockers)),
        "notes": [
            "SQLite file bytes and run UUIDs are intentionally not compared.",
            "Numeric trajectory differences do not self-adjudicate a tolerance.",
            "PV60 scalar formula is a separate GPL-oracle hard gate.",
            "Modern spherical Ertel PV is not changed to imitate FLEXPART calcpv.",
        ],
    }
    report_path = artifact_root / "M4_A3_RUST_NATIVE_DIFF.json"
    write_json(report_path, report)
    print(f"wrote {report_path}", flush=True)
    print(f"status={report['status']} blockers={len(report['blockers'])}", flush=True)
    return 0 if not global_blockers else 2


if __name__ == "__main__":
    raise SystemExit(main())
