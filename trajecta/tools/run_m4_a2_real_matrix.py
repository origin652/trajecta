#!/usr/bin/env python3
"""M4-A2 real-data formal matrix orchestrator (A-owned execution).

Runs the ignored integration test cell-by-cell with independent artifact
directories. Never overwrites prior cell evidence. Does not modify science
core. Does not commit.

Formal matrix (12 cells):
  families: era5-pressure, era5-hybrid, cfsr-pressure
  directions: forward, backward
  platforms: windows, wsl
  particles: 10000

Phased helpers:
  smoke64 / precheck1000 / formal10000
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
FAMILIES = ("era5-pressure", "era5-hybrid", "cfsr-pressure")
DIRECTIONS = ("forward", "backward")
TEST_FILTER = "real_air_mass_domain_fill_three_families_forward_backward"
TEST_PACKAGE = "trajecta-core"
TEST_NAME = "m4_a2_real_data"


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256_file(path: Path) -> str | None:
    if not path.is_file():
        return None
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def collect_host_identity(platform_name: str) -> dict[str, Any]:
    identity: dict[str, Any] = {
        "collected_at_utc": utc_now(),
        "platform_label": platform_name,
        "python": sys.version,
        "host_platform": platform.platform(),
        "host_system": platform.system(),
        "host_release": platform.release(),
        "host_machine": platform.machine(),
        "cwd": str(Path.cwd()),
        "repo_root": str(ROOT),
    }
    for cmd, key in (
        (["rustc", "-V"], "rustc"),
        (["cargo", "-V"], "cargo"),
        (["git", "rev-parse", "HEAD"], "git_head"),
        (["git", "status", "-sb"], "git_status_sb"),
        (["git", "rev-parse", "--abbrev-ref", "HEAD"], "git_branch"),
    ):
        try:
            out = subprocess.check_output(cmd, cwd=ROOT, text=True, stderr=subprocess.STDOUT)
            identity[key] = out.strip()
        except Exception as exc:  # noqa: BLE001 - capture for evidence
            identity[key] = f"ERROR: {exc}"
    if platform_name == "wsl":
        try:
            out = subprocess.check_output(
                ["uname", "-a"], text=True, stderr=subprocess.STDOUT
            )
            identity["uname"] = out.strip()
        except Exception as exc:  # noqa: BLE001
            identity["uname"] = f"ERROR: {exc}"
    return identity


def cell_id(platform_name: str, family: str, direction: str, particles: int) -> str:
    return f"{platform_name}__{family}__{direction}__p{particles}"


def cell_dir(artifact_root: Path, platform_name: str, family: str, direction: str, particles: int) -> Path:
    return (
        artifact_root
        / platform_name
        / f"p{particles}"
        / family
        / direction
    )


def find_run_dirs(artifact_dir: Path) -> list[Path]:
    """Locate run directories that contain run-manifest.json under artifact_dir."""
    found: list[Path] = []
    if not artifact_dir.exists():
        return found
    for manifest in artifact_dir.rglob("run-manifest.json"):
        found.append(manifest.parent)
    return sorted(found)


def sqlite_integrity_ok(db_path: Path) -> tuple[bool, str]:
    if not db_path.is_file():
        return False, "missing particles.sqlite"
    # Prefer python sqlite3 stdlib.
    try:
        import sqlite3

        con = sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True)
        try:
            row = con.execute("PRAGMA integrity_check").fetchone()
            msg = str(row[0]) if row else "empty"
            return msg.lower() == "ok", msg
        finally:
            con.close()
    except Exception as exc:  # noqa: BLE001
        return False, f"sqlite open failed: {exc}"


def validate_cell_artifacts(
    artifact_dir: Path,
    *,
    family: str,
    direction: str,
    platform_name: str,
    particles: int,
    duration_seconds: int,
    test_exit_code: int,
) -> dict[str, Any]:
    """Validate required artifacts for one matrix cell."""
    checks: dict[str, Any] = {
        "test_exit_code": test_exit_code,
        "artifact_dir": str(artifact_dir),
        "required_particles": particles,
        "required_duration_seconds": duration_seconds,
        "family": family,
        "direction": direction,
        "platform": platform_name,
    }
    failures: list[str] = []

    summary_path = artifact_dir / "M4_A2_REAL_MATRIX_SUMMARY.json"
    checks["summary_present"] = summary_path.is_file()
    summary = None
    if summary_path.is_file():
        try:
            summary = read_json(summary_path)
            checks["summary"] = summary
            checks["summary_sha256"] = sha256_file(summary_path)
        except Exception as exc:  # noqa: BLE001
            failures.append(f"summary_json_invalid: {exc}")
    else:
        failures.append("missing M4_A2_REAL_MATRIX_SUMMARY.json")

    run_dirs = find_run_dirs(artifact_dir)
    checks["run_dir_count"] = len(run_dirs)
    if not run_dirs:
        failures.append("no run-manifest.json under artifact dir")
        checks["failures"] = failures
        checks["passed"] = False
        return checks

    # Prefer the run referenced by summary if present.
    run_dir = run_dirs[0]
    if summary and isinstance(summary, dict):
        runs = summary.get("runs") or []
        if runs:
            rel = runs[0].get("manifest_relative_path")
            if rel:
                candidate = artifact_dir / Path(rel).parent
                if candidate.is_dir():
                    run_dir = candidate
            # Seeded particles from summary
            seeded = runs[0].get("seeded_particles")
            status = runs[0].get("status")
            abnormal = runs[0].get("abnormal_terminations")
            numerical_steps = runs[0].get("numerical_steps")
            checks["summary_seeded_particles"] = seeded
            checks["summary_status"] = status
            checks["summary_abnormal_terminations"] = abnormal
            checks["summary_numerical_steps"] = numerical_steps
            checks["summary_family"] = runs[0].get("family")
            checks["summary_direction"] = runs[0].get("direction")
            checks["summary_elapsed_ms"] = runs[0].get("elapsed_milliseconds")
            checks["summary_max_ledger_fraction"] = runs[0].get("maximum_ledger_fraction")
            if seeded != particles:
                failures.append(f"seeded_particles {seeded} != {particles}")
            if status != "complete":
                failures.append(f"summary status {status!r} != complete")
            if abnormal not in (0, 0.0, None) and abnormal != 0:
                failures.append(f"abnormal_terminations={abnormal}")
            expected_steps = (duration_seconds + 299) // 300
            if numerical_steps != expected_steps:
                failures.append(
                    f"numerical_steps {numerical_steps} != expected {expected_steps} "
                    f"for duration {duration_seconds}s"
                )
            if runs[0].get("family") != family:
                failures.append(f"summary family mismatch: {runs[0].get('family')}")
            if runs[0].get("direction") != direction:
                failures.append(f"summary direction mismatch: {runs[0].get('direction')}")
            target = summary.get("target_particle_count")
            if target != particles:
                failures.append(f"summary target_particle_count {target} != {particles}")

    checks["run_dir"] = str(run_dir)
    manifest_path = run_dir / "run-manifest.json"
    sqlite_path = run_dir / "particles.sqlite"
    bundle_path = run_dir / "provenance-bundle.json"

    for label, path in (
        ("run-manifest.json", manifest_path),
        ("particles.sqlite", sqlite_path),
        ("provenance-bundle.json", bundle_path),
    ):
        present = path.is_file()
        checks[f"{label}_present"] = present
        checks[f"{label}_sha256"] = sha256_file(path)
        if not present:
            failures.append(f"missing {label}")

    # resolved case/profile may live beside run dir or under control; search nearby.
    resolved_case = list(artifact_dir.rglob("resolved-case.json")) + list(
        run_dir.rglob("resolved-case.json")
    )
    resolved_profile = list(artifact_dir.rglob("resolved-run-profile.json")) + list(
        run_dir.rglob("resolved-run-profile.json")
    )
    # Also accept case/profile written under control temp (not required on disk by test).
    # Test writes case into control tempdir which is deleted — so look under run dir for copies.
    # Many runners embed paths in manifest; check manifest fields instead.
    checks["resolved_case_candidates"] = [str(p) for p in resolved_case[:5]]
    checks["resolved_profile_candidates"] = [str(p) for p in resolved_profile[:5]]

    if manifest_path.is_file():
        try:
            manifest = read_json(manifest_path)
            checks["manifest_status"] = manifest.get("status")
            if str(manifest.get("status", "")).lower() not in {"complete", "completed"}:
                # Accept both spellings if schema uses one of them.
                failures.append(f"manifest.status={manifest.get('status')!r} not complete")
            term = manifest.get("terminations") or {}
            abnormal = term.get("abnormal_count", term.get("abnormal_terminations"))
            checks["manifest_abnormal_count"] = abnormal
            if abnormal not in (0, 0.0):
                failures.append(f"manifest abnormal_count={abnormal}")
            inputs = manifest.get("inputs") or {}
            for key in (
                "dataset_lock_sha256",
                "dataset_profile_sha256",
                "dataset_content_sha256",
            ):
                val = inputs.get(key)
                checks[key] = val
                if not val:
                    failures.append(f"empty {key}")
            ledger = manifest.get("mass_ledger") or []
            checks["mass_ledger_records"] = len(ledger)
            if not ledger:
                failures.append("empty mass_ledger")
            max_frac = 0.0
            ledger_fail = 0
            for rec in ledger:
                imb = abs(float(rec.get("imbalance_kg", 0.0)))
                tol = float(rec.get("tolerance_kg", 0.0))
                if tol == 0.0:
                    frac = 0.0 if imb == 0.0 else float("inf")
                else:
                    frac = imb / tol
                max_frac = max(max_frac, frac)
                if imb > tol:
                    ledger_fail += 1
            checks["mass_ledger_max_fraction"] = max_frac
            checks["mass_ledger_imbalance_violations"] = ledger_fail
            if ledger_fail:
                failures.append(f"mass_ledger violations={ledger_fail}")
            # final mass gate if present
            final_gate = manifest.get("final_mass_gate") or manifest.get("mass_final_gate")
            if final_gate is not None:
                checks["final_mass_gate"] = final_gate
                if isinstance(final_gate, dict) and final_gate.get("passed") is False:
                    failures.append("final_mass_gate.passed=false")
        except Exception as exc:  # noqa: BLE001
            failures.append(f"manifest_parse: {exc}")

    if bundle_path.is_file():
        try:
            bundle = read_json(bundle_path)
            checks["provenance_schema_version"] = bundle.get("schema_version")
            if not bundle.get("schema_version"):
                failures.append("provenance missing schema_version")
            if not bundle.get("run_id"):
                failures.append("provenance missing run_id")
        except Exception as exc:  # noqa: BLE001
            failures.append(f"provenance_parse: {exc}")

    ok, integrity_msg = sqlite_integrity_ok(sqlite_path)
    checks["sqlite_integrity_check"] = integrity_msg
    if not ok:
        failures.append(f"sqlite integrity_check={integrity_msg}")

    if test_exit_code != 0:
        failures.append(f"test_exit_code={test_exit_code}")

    checks["failures"] = failures
    checks["passed"] = not failures
    return checks


def build_cargo_cmd(
    *,
    particles: int,
    family: str,
    direction: str,
    artifact_dir: Path,
    release: bool,
) -> list[str]:
    cmd = ["cargo", "test", "--offline", "-p", TEST_PACKAGE, "--test", TEST_NAME]
    if release:
        cmd.append("--release")
    cmd.extend(
        [
            TEST_FILTER,
            "--",
            "--ignored",
            "--nocapture",
            "--exact",
        ]
    )
    return cmd


def run_windows_cell(
    *,
    family: str,
    direction: str,
    particles: int,
    duration_seconds: int,
    artifact_root: Path,
    release: bool,
) -> dict[str, Any]:
    platform_name = "windows"
    cid = cell_id(platform_name, family, direction, particles)
    out_dir = cell_dir(artifact_root, platform_name, family, direction, particles)
    out_dir.mkdir(parents=True, exist_ok=True)
    stdout_path = out_dir / "stdout.log"
    stderr_path = out_dir / "stderr.log"
    meta_path = out_dir / "cell_meta.json"

    env = os.environ.copy()
    env["TRAJECTA_M4_A2_PARTICLES"] = str(particles)
    env["TRAJECTA_M4_A2_DURATION_SECONDS"] = str(duration_seconds)
    env["TRAJECTA_M4_A2_FAMILY"] = family
    env["TRAJECTA_M4_A2_DIRECTION"] = direction
    env["TRAJECTA_M4_A2_ARTIFACT_DIR"] = str(out_dir.resolve())
    # Ensure real-met fixtures path resolution remains default relative.
    env.setdefault("TRAJECTA_REQUIRE_REAL_MET", "1")

    cmd = build_cargo_cmd(
        particles=particles,
        family=family,
        direction=direction,
        artifact_dir=out_dir,
        release=release,
    )
    identity = collect_host_identity(platform_name)
    started = time.time()
    started_utc = utc_now()
    print(f"[run] {cid}", flush=True)
    print(f"  cmd: {' '.join(cmd)}", flush=True)
    print(f"  artifact: {out_dir}", flush=True)

    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    elapsed = time.time() - started
    stdout_path.write_text(proc.stdout or "", encoding="utf-8", errors="replace")
    stderr_path.write_text(proc.stderr or "", encoding="utf-8", errors="replace")

    validation = validate_cell_artifacts(
        out_dir,
        family=family,
        direction=direction,
        platform_name=platform_name,
        particles=particles,
        duration_seconds=duration_seconds,
        test_exit_code=proc.returncode,
    )
    status = "passed" if validation.get("passed") else "failed"
    result = {
        "cell_id": cid,
        "platform": platform_name,
        "family": family,
        "direction": direction,
        "particles": particles,
        "duration_seconds": duration_seconds,
        "status": status,
        "implemented": True,
        "executed": True,
        "started_utc": started_utc,
        "finished_utc": utc_now(),
        "elapsed_seconds": elapsed,
        "command": cmd,
        "returncode": proc.returncode,
        "artifact_dir": str(out_dir),
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
        "identity": identity,
        "validation": validation,
        "release": release,
    }
    write_json(meta_path, result)
    print(f"  status={status} rc={proc.returncode} elapsed={elapsed:.1f}s", flush=True)
    if not validation.get("passed"):
        print(f"  failures={validation.get('failures')}", flush=True)
    return result


def run_wsl_cell(
    *,
    family: str,
    direction: str,
    particles: int,
    duration_seconds: int,
    artifact_root: Path,
    release: bool,
    wsl_distro: str,
    wsl_target_dir: str,
) -> dict[str, Any]:
    platform_name = "wsl"
    cid = cell_id(platform_name, family, direction, particles)
    # Artifact on Windows repo path so evidence is durable.
    out_dir = cell_dir(artifact_root, platform_name, family, direction, particles)
    out_dir.mkdir(parents=True, exist_ok=True)
    stdout_path = out_dir / "stdout.log"
    stderr_path = out_dir / "stderr.log"
    meta_path = out_dir / "cell_meta.json"
    runner_sh = out_dir / "wsl_run.sh"

    # Linux paths
    linux_root = "/mnt/e/flexpart/trajecta"
    # Convert Windows artifact path to /mnt/e/...
    out_resolved = out_dir.resolve()
    # Expect E:\flexpart\...
    linux_artifact = "/mnt/" + out_resolved.drive[0].lower() + out_resolved.as_posix()[2:]

    cargo_flags = "--release" if release else ""
    script = f"""#!/bin/bash
set -euo pipefail
export PATH="/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_TARGET_DIR="{wsl_target_dir}"
export LIBCLANG_PATH="${{LIBCLANG_PATH:-/usr/lib/llvm-18/lib}}"
export TRAJECTA_M4_A2_PARTICLES="{particles}"
export TRAJECTA_M4_A2_DURATION_SECONDS="{duration_seconds}"
export TRAJECTA_M4_A2_FAMILY="{family}"
export TRAJECTA_M4_A2_DIRECTION="{direction}"
export TRAJECTA_M4_A2_ARTIFACT_DIR="{linux_artifact}"
export TRAJECTA_REQUIRE_REAL_MET=1
cd "{linux_root}"
echo "=== identity ==="
date -u +"utc=%Y-%m-%dT%H:%M:%SZ"
uname -a
rustc -V
cargo -V
git rev-parse HEAD || true
git status -sb || true
echo "CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
echo "ARTIFACT=$TRAJECTA_M4_A2_ARTIFACT_DIR"
mkdir -p "$CARGO_TARGET_DIR"
mkdir -p "$TRAJECTA_M4_A2_ARTIFACT_DIR"
echo "=== cargo test ==="
cargo test --offline {cargo_flags} -p {TEST_PACKAGE} --test {TEST_NAME} \\
  {TEST_FILTER} -- --ignored --nocapture --exact
"""
    # Force Unix LF endings; CRLF breaks `set -o pipefail` under /bin/bash.
    runner_sh.write_bytes(script.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8"))

    started = time.time()
    started_utc = utc_now()
    print(f"[run] {cid}", flush=True)
    print(f"  artifact: {out_dir}", flush=True)
    print(f"  wsl_target: {wsl_target_dir}", flush=True)

    # Invoke via cmd to avoid Git-Bash path mangling.
    wsl_cmd = [
        "cmd.exe",
        "/c",
        "wsl.exe",
        "-d",
        wsl_distro,
        "-e",
        "/bin/bash",
        "--noprofile",
        "--norc",
        linux_artifact + "/wsl_run.sh",
    ]
    try:
        proc = subprocess.run(
            wsl_cmd,
            cwd=str(ROOT),
            text=True,
            capture_output=True,
        )
        external_blocked = False
        block_reason = None
    except Exception as exc:  # noqa: BLE001
        proc = None
        external_blocked = True
        block_reason = f"wsl launch failed: {exc}"

    elapsed = time.time() - started
    if proc is not None:
        stdout_path.write_text(proc.stdout or "", encoding="utf-8", errors="replace")
        stderr_path.write_text(proc.stderr or "", encoding="utf-8", errors="replace")
        rc = proc.returncode
        # Detect toolchain / sandbox style failures.
        combined = (proc.stdout or "") + "\n" + (proc.stderr or "")
        if rc != 0 and any(
            needle in combined
            for needle in (
                "not applicable to the",
                "No such file or directory",
                "permission denied",
                "Failed to start",
                "could not compile",
                "linker `cc` not found",
                "error: linker",
            )
        ) and "assertion" not in combined.lower() and "M4-A2" not in combined:
            # Heuristic: build/tooling failure may be external_blocked rather than science fail.
            # Still mark failed if test actually ran and asserted.
            if "running 1 test" not in combined and "test result:" not in combined:
                external_blocked = True
                block_reason = "WSL toolchain/build failure before/during compile (see logs)"
    else:
        stdout_path.write_text("", encoding="utf-8")
        stderr_path.write_text(block_reason or "", encoding="utf-8")
        rc = 127

    identity = {
        "platform_label": "wsl",
        "wsl_distro": wsl_distro,
        "wsl_target_dir": wsl_target_dir,
        "linux_artifact": linux_artifact,
        "note": "full identity embedded in stdout.log header",
    }

    if external_blocked:
        result = {
            "cell_id": cid,
            "platform": platform_name,
            "family": family,
            "direction": direction,
            "particles": particles,
            "duration_seconds": duration_seconds,
            "status": "external_blocked",
            "implemented": True,
            "executed": False if proc is None else True,
            "started_utc": started_utc,
            "finished_utc": utc_now(),
            "elapsed_seconds": elapsed,
            "command": wsl_cmd,
            "returncode": rc,
            "artifact_dir": str(out_dir),
            "stdout_log": str(stdout_path),
            "stderr_log": str(stderr_path),
            "identity": identity,
            "block_reason": block_reason,
            "release": release,
            "validation": {
                "passed": False,
                "failures": [block_reason or "external_blocked"],
            },
        }
        write_json(meta_path, result)
        print(f"  status=external_blocked rc={rc} elapsed={elapsed:.1f}s", flush=True)
        print(f"  reason={block_reason}", flush=True)
        return result

    validation = validate_cell_artifacts(
        out_dir,
        family=family,
        direction=direction,
        platform_name=platform_name,
        particles=particles,
        duration_seconds=duration_seconds,
        test_exit_code=rc,
    )
    status = "passed" if validation.get("passed") else "failed"
    result = {
        "cell_id": cid,
        "platform": platform_name,
        "family": family,
        "direction": direction,
        "particles": particles,
        "duration_seconds": duration_seconds,
        "status": status,
        "implemented": True,
        "executed": True,
        "started_utc": started_utc,
        "finished_utc": utc_now(),
        "elapsed_seconds": elapsed,
        "command": wsl_cmd,
        "returncode": rc,
        "artifact_dir": str(out_dir),
        "stdout_log": str(stdout_path),
        "stderr_log": str(stderr_path),
        "identity": identity,
        "validation": validation,
        "release": release,
    }
    write_json(meta_path, result)
    print(f"  status={status} rc={rc} elapsed={elapsed:.1f}s", flush=True)
    if not validation.get("passed"):
        print(f"  failures={validation.get('failures')}", flush=True)
    return result


def parse_selection(arg: str | None, allowed: tuple[str, ...], label: str) -> list[str]:
    if not arg or arg.lower() == "all":
        return list(allowed)
    items = [x.strip() for x in arg.split(",") if x.strip()]
    bad = [x for x in items if x not in allowed]
    if bad:
        raise SystemExit(f"unknown {label}: {bad}; allowed={allowed}")
    return items


def load_existing_results(artifact_root: Path) -> list[dict[str, Any]]:
    results = []
    for meta in artifact_root.rglob("cell_meta.json"):
        try:
            results.append(read_json(meta))
        except Exception:
            continue
    return results


def write_phase_summary(artifact_root: Path, phase: str, results: list[dict[str, Any]]) -> Path:
    summary = {
        "schema_version": "trajecta.m4-a2-execution-summary/v1",
        "phase": phase,
        "generated_at_utc": utc_now(),
        "repo_root": str(ROOT),
        "counts": {
            "total": len(results),
            "passed": sum(1 for r in results if r.get("status") == "passed"),
            "failed": sum(1 for r in results if r.get("status") == "failed"),
            "external_blocked": sum(1 for r in results if r.get("status") == "external_blocked"),
            "not_run": sum(1 for r in results if r.get("status") == "not_run"),
        },
        "results": results,
    }
    path = artifact_root / "summary" / f"{phase}_summary.json"
    write_json(path, summary)
    # Also refresh aggregate of all cell_meta.
    all_results = load_existing_results(artifact_root)
    write_json(
        artifact_root / "summary" / "ALL_CELLS_SUMMARY.json",
        {
            "schema_version": "trajecta.m4-a2-execution-summary/v1",
            "phase": "all_cells_on_disk",
            "generated_at_utc": utc_now(),
            "counts": {
                "total": len(all_results),
                "passed": sum(1 for r in all_results if r.get("status") == "passed"),
                "failed": sum(1 for r in all_results if r.get("status") == "failed"),
                "external_blocked": sum(
                    1 for r in all_results if r.get("status") == "external_blocked"
                ),
            },
            "results": all_results,
        },
    )
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--phase",
        required=True,
        choices=(
            "identity",
            "wsl-smoke64",
            "windows-smoke64",
            "wsl-precheck1000",
            "windows-precheck1000",
            "windows-formal10000",
            "wsl-formal10000",
            "formal10000-both",
            "single",
        ),
    )
    parser.add_argument("--platform", choices=("windows", "wsl"), default=None)
    parser.add_argument("--family", default="all")
    parser.add_argument("--direction", default="all")
    parser.add_argument("--particles", type=int, default=None)
    parser.add_argument(
        "--duration-seconds",
        type=int,
        default=600,
        help="Physical duration per cell; formal M4-A2 evidence uses 600",
    )
    parser.add_argument(
        "--artifact-root",
        type=Path,
        default=ROOT / "target" / "m4-a2",
    )
    parser.add_argument("--release", action="store_true", default=False)
    parser.add_argument("--no-release", action="store_true", default=False)
    parser.add_argument("--wsl-distro", default="Ubuntu-24.04")
    parser.add_argument(
        "--wsl-target-dir",
        default="/tmp/trajecta-m4-a2-target",
        help="Linux-only CARGO_TARGET_DIR (must not be Windows target)",
    )
    parser.add_argument(
        "--stop-on-fail",
        action="store_true",
        help="Stop phase after first failed/blocked cell (prior evidence kept)",
    )
    parser.add_argument(
        "--skip-existing-passed",
        action="store_true",
        help="Skip cells whose cell_meta.json already status=passed",
    )
    args = parser.parse_args()
    if args.duration_seconds <= 0:
        raise SystemExit("--duration-seconds must be positive")

    artifact_root = args.artifact_root if args.artifact_root.is_absolute() else (ROOT / args.artifact_root)
    artifact_root.mkdir(parents=True, exist_ok=True)
    (artifact_root / "summary").mkdir(parents=True, exist_ok=True)
    (artifact_root / "logs").mkdir(parents=True, exist_ok=True)

    if args.phase == "identity":
        win = collect_host_identity("windows")
        write_json(artifact_root / "summary" / "windows_identity.json", win)
        print(json.dumps(win, indent=2))
        return 0

    # Phase presets
    if args.phase == "wsl-smoke64":
        platform_name, particles, release = "wsl", 64, False
    elif args.phase == "windows-smoke64":
        platform_name, particles, release = "windows", 64, True
    elif args.phase == "wsl-precheck1000":
        platform_name, particles, release = "wsl", 1000, False
    elif args.phase == "windows-precheck1000":
        platform_name, particles, release = "windows", 1000, True
    elif args.phase == "windows-formal10000":
        platform_name, particles, release = "windows", 10000, True
    elif args.phase == "wsl-formal10000":
        platform_name, particles, release = "wsl", 10000, True
    elif args.phase == "formal10000-both":
        # Handled specially below.
        platform_name, particles, release = None, 10000, True
    elif args.phase == "single":
        if not args.platform or args.particles is None:
            raise SystemExit("single requires --platform and --particles")
        platform_name = args.platform
        particles = args.particles
        release = args.release and not args.no_release
        if args.platform == "windows" and particles >= 1000 and not args.no_release:
            release = True
    else:
        raise SystemExit(f"unhandled phase {args.phase}")

    if args.no_release:
        release = False
    if args.release:
        release = True

    families = parse_selection(args.family, FAMILIES, "family")
    directions = parse_selection(args.direction, DIRECTIONS, "direction")

    plan: list[tuple[str, str, str, int, bool]] = []
    if args.phase == "formal10000-both":
        for plat in ("windows", "wsl"):
            for fam in families:
                for direction in directions:
                    plan.append((plat, fam, direction, 10000, True))
    else:
        assert platform_name is not None and particles is not None
        for fam in families:
            for direction in directions:
                plan.append((platform_name, fam, direction, particles, release))

    results: list[dict[str, Any]] = []
    print(f"phase={args.phase} cells={len(plan)} artifact_root={artifact_root}", flush=True)

    for plat, fam, direction, particles, rel in plan:
        out_dir = cell_dir(artifact_root, plat, fam, direction, particles)
        meta = out_dir / "cell_meta.json"
        if args.skip_existing_passed and meta.is_file():
            try:
                prev = read_json(meta)
                if prev.get("status") == "passed":
                    print(f"[skip-passed] {cell_id(plat, fam, direction, particles)}", flush=True)
                    results.append(prev)
                    continue
            except Exception:
                pass

        if plat == "windows":
            result = run_windows_cell(
                family=fam,
                direction=direction,
                particles=particles,
                duration_seconds=args.duration_seconds,
                artifact_root=artifact_root,
                release=rel,
            )
        else:
            result = run_wsl_cell(
                family=fam,
                direction=direction,
                particles=particles,
                duration_seconds=args.duration_seconds,
                artifact_root=artifact_root,
                release=rel,
                wsl_distro=args.wsl_distro,
                wsl_target_dir=args.wsl_target_dir,
            )
        results.append(result)
        write_phase_summary(artifact_root, args.phase, results)
        if args.stop_on_fail and result.get("status") != "passed":
            print("stop-on-fail triggered", flush=True)
            break

    summary_path = write_phase_summary(artifact_root, args.phase, results)
    print(f"summary: {summary_path}", flush=True)
    failed = [r for r in results if r.get("status") != "passed"]
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
