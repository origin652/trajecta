#!/usr/bin/env python3
"""Collect independent read-only SQLite evidence while an A4 WSL cell runs.

The monitored PID is the GNU ``time`` wrapper used by the A4 orchestrator.  A
snapshot counts only when that process is alive and the production manifest is
``running`` both before and after one short SQLite read transaction.  The
monitor never checkpoints the WAL or writes to the production database.  Its
active-run probes use indexed high-water marks; exact row counts belong to the
single terminal postflight audit, not a repeated benchmark-side table scan.
"""
from __future__ import annotations

import argparse
import json
import os
import sqlite3
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import quote

SCHEMA_VERSION = "trajecta.m4-a4-concurrent-reader/v1"
MAX_RECORDED_SAMPLES = 32
MAX_RECORDED_ERRORS = 32

OUTPUT_EVENT_HIGH_WATER_SQL = """
SELECT event_sequence
FROM output_event
WHERE run_id = ?
ORDER BY event_sequence DESC
LIMIT 1
"""

PARTICLE_HIGH_WATER_SQL = """
SELECT particle_id
FROM particle
WHERE run_id = ?
ORDER BY particle_id DESC
LIMIT 1
"""

PARTICLE_STATE_HIGH_WATER_SQL = """
SELECT physical_seconds, physical_nanosecond, particle_id, sample_sequence
FROM particle_state
WHERE run_id = ?
ORDER BY physical_seconds DESC, physical_nanosecond DESC,
         particle_id DESC, sample_sequence DESC
LIMIT 1
"""


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def write_json_atomic(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(
        json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    os.replace(temporary, path)


def process_state(pid: int) -> tuple[bool, str | None]:
    """Return whether a Linux process is live (zombies do not count)."""
    stat_path = Path("/proc") / str(pid) / "stat"
    try:
        raw = stat_path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return False, None
    except OSError as exc:
        return False, f"error:{exc}"
    closing = raw.rfind(")")
    if closing < 0:
        return False, "malformed"
    fields = raw[closing + 2 :].split()
    if not fields:
        return False, "malformed"
    state = fields[0]
    return state not in {"Z", "X"}, state


def read_manifest_status(path: Path) -> str | None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    status = payload.get("status") if isinstance(payload, dict) else None
    return str(status).lower() if status is not None else None


def discover_run_dir(artifact_dir: Path, selected: Path | None) -> Path | None:
    if selected is not None and (selected / "run-manifest.json").is_file():
        return selected
    running: list[Path] = []
    terminal: list[Path] = []
    for manifest in artifact_dir.rglob("run-manifest.json"):
        status = read_manifest_status(manifest)
        if status == "running":
            running.append(manifest.parent)
        elif status is not None:
            terminal.append(manifest.parent)
    if len(running) == 1:
        return running[0]
    if not running and len(terminal) == 1:
        return terminal[0]
    return None


def sqlite_snapshot(path: Path) -> dict[str, Any]:
    uri = f"file:{quote(path.as_posix(), safe='/')}?mode=ro"
    connection = sqlite3.connect(
        uri,
        uri=True,
        timeout=0.05,
        isolation_level=None,
    )
    try:
        # Connection-local only: this cannot alter the database or WAL.
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        try:
            user_version = int(connection.execute("PRAGMA user_version").fetchone()[0])
            run_rows = connection.execute("SELECT run_id FROM run LIMIT 2").fetchall()
            if len(run_rows) != 1:
                raise sqlite3.DatabaseError(
                    f"expected exactly one run row, found {len(run_rows)}"
                )
            run_id = str(run_rows[0][0])
            output_event = connection.execute(
                OUTPUT_EVENT_HIGH_WATER_SQL, (run_id,)
            ).fetchone()
            particle = connection.execute(
                PARTICLE_HIGH_WATER_SQL, (run_id,)
            ).fetchone()
            particle_state = connection.execute(
                PARTICLE_STATE_HIGH_WATER_SQL, (run_id,)
            ).fetchone()
            page_count = int(connection.execute("PRAGMA page_count").fetchone()[0])
        finally:
            connection.execute("ROLLBACK")
        return {
            "user_version": user_version,
            "run_id": run_id,
            "database_page_count": page_count,
            "output_event_max_sequence": (
                int(output_event[0]) if output_event is not None else None
            ),
            "particle_max_id": int(particle[0]) if particle is not None else None,
            "particle_state_high_water": (
                {
                    "physical_seconds": int(particle_state[0]),
                    "physical_nanosecond": int(particle_state[1]),
                    "particle_id": int(particle_state[2]),
                    "sample_sequence": int(particle_state[3]),
                }
                if particle_state is not None
                else None
            ),
        }
    finally:
        connection.close()


def append_bounded(items: list[dict[str, Any]], value: dict[str, Any], limit: int) -> None:
    if len(items) < limit:
        items.append(value)


def monitor(args: argparse.Namespace) -> dict[str, Any]:
    artifact_dir = args.artifact_dir.resolve()
    started_monotonic = time.monotonic()
    started_utc = utc_now()
    selected_run_dir: Path | None = None
    attempts = 0
    successful_active_snapshots = 0
    busy_or_locked_errors = 0
    other_errors = 0
    recorded_samples: list[dict[str, Any]] = []
    recorded_errors: list[dict[str, Any]] = []
    last_process_state: str | None = None

    while True:
        elapsed = time.monotonic() - started_monotonic
        if elapsed > args.timeout_seconds:
            append_bounded(
                recorded_errors,
                {"at_utc": utc_now(), "kind": "timeout", "elapsed_seconds": elapsed},
                MAX_RECORDED_ERRORS,
            )
            other_errors += 1
            break

        active_before, state_before = process_state(args.process_pid)
        last_process_state = state_before
        if not active_before:
            break

        attempts += 1
        selected_run_dir = discover_run_dir(artifact_dir, selected_run_dir)
        if selected_run_dir is not None:
            manifest_path = selected_run_dir / "run-manifest.json"
            sqlite_path = selected_run_dir / "particles.sqlite"
            status_before = read_manifest_status(manifest_path)
            if status_before == "running" and sqlite_path.is_file():
                try:
                    snapshot = sqlite_snapshot(sqlite_path)
                    active_after, state_after = process_state(args.process_pid)
                    status_after = read_manifest_status(manifest_path)
                    if active_after and status_after == "running":
                        successful_active_snapshots += 1
                        append_bounded(
                            recorded_samples,
                            {
                                "at_utc": utc_now(),
                                "process_state_before": state_before,
                                "process_state_after": state_after,
                                "manifest_status_before": status_before,
                                "manifest_status_after": status_after,
                                **snapshot,
                            },
                            MAX_RECORDED_SAMPLES,
                        )
                except sqlite3.OperationalError as exc:
                    message = str(exc)
                    lowered = message.lower()
                    kind = "busy_or_locked" if "busy" in lowered or "locked" in lowered else "other"
                    if kind == "busy_or_locked":
                        busy_or_locked_errors += 1
                    else:
                        other_errors += 1
                    append_bounded(
                        recorded_errors,
                        {"at_utc": utc_now(), "kind": kind, "message": message},
                        MAX_RECORDED_ERRORS,
                    )
                except (OSError, sqlite3.DatabaseError, ValueError) as exc:
                    other_errors += 1
                    append_bounded(
                        recorded_errors,
                        {"at_utc": utc_now(), "kind": "other", "message": str(exc)},
                        MAX_RECORDED_ERRORS,
                    )
        time.sleep(args.interval_seconds)

    failures: list[str] = []
    if successful_active_snapshots < args.required_active_snapshots:
        failures.append(
            "writer-active SQLite snapshots "
            f"{successful_active_snapshots} < required {args.required_active_snapshots}"
        )
    return {
        "schema_version": SCHEMA_VERSION,
        "started_at_utc": started_utc,
        "finished_at_utc": utc_now(),
        "elapsed_seconds": time.monotonic() - started_monotonic,
        "artifact_dir": str(artifact_dir),
        "monitored_process_pid": args.process_pid,
        "last_process_state": last_process_state,
        "connection_contract": "SQLite URI mode=ro plus connection-local query_only",
        "transaction_contract": (
            "one short indexed read transaction per attempt; no checkpoint or writes"
        ),
        "snapshot_contract": (
            "indexed_high_water_marks/v1; exact terminal row counts are postflight-only"
        ),
        "interval_seconds": args.interval_seconds,
        "required_active_snapshots": args.required_active_snapshots,
        "attempts": attempts,
        "successful_active_snapshots": successful_active_snapshots,
        "busy_or_locked_errors": busy_or_locked_errors,
        "other_errors": other_errors,
        "selected_run_dir": str(selected_run_dir) if selected_run_dir else None,
        "recorded_samples": recorded_samples,
        "recorded_errors": recorded_errors,
        "failures": failures,
        "passed": not failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--process-pid", type=int, required=True)
    parser.add_argument("--required-active-snapshots", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--interval-seconds", type=float, default=0.10)
    parser.add_argument("--timeout-seconds", type=float, default=21_600.0)
    args = parser.parse_args()
    if args.process_pid <= 0:
        raise SystemExit("--process-pid must be positive")
    if args.required_active_snapshots < 0:
        raise SystemExit("--required-active-snapshots must be nonnegative")
    if not 0.02 <= args.interval_seconds <= 10.0:
        raise SystemExit("--interval-seconds must be in 0.02..10")
    if args.timeout_seconds <= 0:
        raise SystemExit("--timeout-seconds must be positive")
    result = monitor(args)
    write_json_atomic(args.output, result)
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
