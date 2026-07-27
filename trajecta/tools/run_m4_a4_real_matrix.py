#!/usr/bin/env python3
"""Sequential, artifact-preserving M4-A4 real-data performance matrix runner.

This program runs only the frozen ERA5-hybrid A4 production harness.  It never
changes numerical settings, never retries into the same artifact directory, and
writes a phase summary after every cell so a later failure cannot hide evidence
from an earlier one.

Phases:
  preflight  : one Windows/WSL 1,000-particle forward, four-worker cell
  diagnostic : one WSL 10,000-particle forward, four-worker cell
  attribution: three 1,000-particle forward repetitions at one/four workers
  formal     : 50,000 and 100,000 particles × forward/backward × 1/4 workers
  single     : one explicitly stated frozen cell (for recovery / review)
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform as host_platform
import re
import sqlite3
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
TEST_PACKAGE = "trajecta-core"
TEST_NAME = "m4_a4_real_perf"
TEST_FILTER = "real_air_mass_domain_fill_hybrid_performance_cell"
FAMILY = "era5-hybrid"
DIRECTIONS = ("forward", "backward")
WORKERS = (1, 4)
FORMAL_CELLS = (
    ("forward", 50_000, 4),  # S50-F
    ("forward", 100_000, 4),  # S100-F
    ("backward", 50_000, 4),  # S50-B
    ("backward", 100_000, 4),  # S100-B
    ("forward", 100_000, 1),  # D100-F
    ("backward", 100_000, 1),  # D100-B
)
DURATION_SECONDS = 3_600
TIME_STEP_SECONDS = 600
OUTPUT_INTERVAL_SECONDS = 600
GNU_TIME_FILE_NAME = "gnu-time-v.txt"
CONCURRENT_READER_FILE_NAME = "M4_A4_CONCURRENT_READER.json"
CELL_SUMMARY_FILE_NAME = "M4_A4_CELL_SUMMARY.json"
PEAK_RSS_LIMIT_BYTES = 2 * 1_024 * 1_024 * 1_024
SQLITE_LIMIT_BYTES = 512 * 1_024 * 1_024
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256(path: Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size if path.is_file() else 0
    except OSError:
        return 0


def directory_size(path: Path) -> int:
    total = 0
    if not path.is_dir():
        return total
    for root, _, files in os.walk(path):
        for name in files:
            total += file_size(Path(root) / name)
    return total


def parse_gnu_time(path: Path) -> dict[str, Any]:
    evidence: dict[str, Any] = {
        "method": "GNU /usr/bin/time -v Maximum resident set size",
        "path": str(path),
        "present": path.is_file(),
        "peak_rss_bytes": None,
        "elapsed_wall_clock": None,
        "exit_status": None,
    }
    if not path.is_file():
        return evidence
    text = path.read_text(encoding="utf-8", errors="replace")
    rss = re.search(r"^\s*Maximum resident set size \(kbytes\):\s*(\d+)\s*$", text, re.MULTILINE)
    elapsed = re.search(
        r"^\s*Elapsed \(wall clock\) time \(h:mm:ss or m:ss\):\s*(.+?)\s*$",
        text,
        re.MULTILINE,
    )
    status = re.search(r"^\s*Exit status:\s*(-?\d+)\s*$", text, re.MULTILINE)
    if rss:
        evidence["peak_rss_bytes"] = int(rss.group(1)) * 1_024
    if elapsed:
        evidence["elapsed_wall_clock"] = elapsed.group(1)
    if status:
        evidence["exit_status"] = int(status.group(1))
    evidence["sha256"] = sha256(path)
    return evidence


def required_active_snapshots(mode: str, particles: int) -> int:
    if mode in {"preflight", "diagnostic"}:
        return 1
    if mode == "formal" and particles == 100_000:
        return 3
    if mode == "formal" and particles == 50_000:
        return 1
    return 0


def host_identity(label: str) -> dict[str, Any]:
    result: dict[str, Any] = {
        "collected_at_utc": utc_now(),
        "platform_label": label,
        "python": sys.version,
        "host_platform": host_platform.platform(),
        "host_system": host_platform.system(),
        "host_release": host_platform.release(),
        "host_machine": host_platform.machine(),
        "cwd": str(Path.cwd()),
        "repo_root": str(ROOT),
    }
    for command, key in (
        (["rustc", "-V"], "rustc"),
        (["cargo", "-V"], "cargo"),
        (["git", "rev-parse", "HEAD"], "git_head"),
        (["git", "rev-parse", "--abbrev-ref", "HEAD"], "git_branch"),
        (["git", "status", "--short"], "git_status_short"),
    ):
        try:
            result[key] = subprocess.check_output(
                command, cwd=ROOT, text=True, stderr=subprocess.STDOUT
            ).strip()
        except Exception as exc:  # evidence must include probe failures
            result[key] = f"ERROR: {exc}"
    status = result.get("git_status_short")
    if isinstance(status, str) and not status.startswith("ERROR:"):
        result["git_dirty_paths"] = [
            line[3:].strip() for line in status.splitlines() if len(line) >= 4
        ]
    return result


def source_tree_identity() -> dict[str, Any]:
    """Hash tracked and nonignored untracked files below the Trajecta root."""
    head = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
    ).strip()
    listing = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "."],
        cwd=ROOT,
    )
    paths = [os.fsdecode(raw) for raw in listing.split(b"\0") if raw]
    digest = hashlib.sha256()
    included: list[str] = []
    for relative in sorted(paths):
        path = (ROOT / relative).resolve()
        try:
            path.relative_to(ROOT.resolve())
        except ValueError as exc:
            raise RuntimeError(f"source identity escaped repository root: {relative}") from exc
        if not path.is_file():
            continue
        encoded = relative.replace("\\", "/").encode("utf-8", errors="surrogateescape")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
        digest.update(path.stat().st_size.to_bytes(8, "big"))
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        included.append(relative.replace("\\", "/"))
    status = subprocess.check_output(
        ["git", "status", "--short", "--", "."], cwd=ROOT, text=True
    )
    return {
        "git_head": head,
        "source_tree_sha256": digest.hexdigest(),
        "file_count": len(included),
        "dirty_paths": [line[3:].strip() for line in status.splitlines() if len(line) >= 4],
    }


def cell_id(platform_name: str, direction: str, particles: int, workers: int) -> str:
    return f"{platform_name}__{FAMILY}__{direction}__p{particles}__w{workers}"


def next_attempt_directory(root: Path, cid: str) -> Path:
    """Allocate a fresh evidence attempt; never overwrite a prior attempt."""
    cell_root = root / "cells" / cid
    attempts = [
        int(path.name.removeprefix("attempt-"))
        for path in cell_root.glob("attempt-*")
        if path.is_dir() and path.name.removeprefix("attempt-").isdigit()
    ]
    return cell_root / f"attempt-{max(attempts, default=0) + 1}"


def direct_artifact_files(run_dir: Path) -> dict[str, Path]:
    return {
        "run-manifest.json": run_dir / "run-manifest.json",
        "particles.sqlite": run_dir / "particles.sqlite",
        "provenance-bundle.json": run_dir / "provenance-bundle.json",
        "resolved-case.json": run_dir / "resolved-case.json",
        "resolved-run-profile.json": run_dir / "resolved-run-profile.json",
    }


def find_one_run_dir(cell_dir: Path, failures: list[str]) -> Path | None:
    candidates = sorted(path.parent for path in cell_dir.rglob("run-manifest.json"))
    if len(candidates) != 1:
        failures.append(f"expected exactly one run-manifest.json, found {len(candidates)}")
        return candidates[0] if candidates else None
    return candidates[0]


def sqlite_integrity(path: Path) -> tuple[bool, str]:
    if not path.is_file():
        return False, "missing particles.sqlite"
    try:
        connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
        try:
            row = connection.execute("PRAGMA integrity_check").fetchone()
            result = str(row[0]) if row else "empty"
            return result.lower() == "ok", result
        finally:
            connection.close()
    except Exception as exc:
        return False, f"sqlite exception: {exc}"


def sqlite_lifecycle_audit(path: Path, direction: str) -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    try:
        # The production schema is indexed for public particle/time access, not
        # for every acceptance-only lifecycle join. Materialize only the eight
        # columns used below into a local TEMP database and build audit indexes
        # once, avoiding O(output_events * particle_states) scans on /mnt drives.
        connection.execute("PRAGMA temp_store=FILE")
        connection.executescript(
            """
            CREATE TEMP TABLE particle_state AS
            SELECT run_id, particle_id, sample_sequence, event_sequence,
                   physical_seconds, physical_nanosecond,
                   particle_status, termination_reason
            FROM main.particle_state;
            CREATE UNIQUE INDEX temp.audit_state_by_particle_sample
                ON particle_state (run_id, particle_id, sample_sequence);
            CREATE INDEX temp.audit_state_by_particle_time
                ON particle_state (
                    run_id, particle_id, physical_seconds, physical_nanosecond
                );
            CREATE INDEX temp.audit_state_by_event
                ON particle_state (run_id, event_sequence);
            """
        )
        first_event = connection.execute(
            """SELECT physical_seconds, physical_nanosecond
               FROM output_event WHERE event_sequence = 0"""
        ).fetchone()
        if first_event is None:
            raise ValueError("missing output event_sequence=0")
        direction_sign = 1 if direction == "forward" else -1
        schedule = [
            (int(first_event[0]) + direction_sign * step * OUTPUT_INTERVAL_SECONDS,
             int(first_event[1]))
            for step in range(DURATION_SECONDS // OUTPUT_INTERVAL_SECONDS + 1)
        ]
        schedule_values = ",".join(f"({seconds},{nanosecond})" for seconds, nanosecond in schedule)
        schedule_cte = (
            "schedule(physical_seconds, physical_nanosecond) AS "
            f"(VALUES {schedule_values})"
        )
        if direction == "forward":
            after_birth = """(
                sc.physical_seconds > p.birth_seconds OR
                (sc.physical_seconds = p.birth_seconds AND
                 sc.physical_nanosecond >= p.birth_nanosecond)
            )"""
            before_termination = """(
                t.particle_id IS NULL OR
                sc.physical_seconds < t.physical_seconds OR
                (sc.physical_seconds = t.physical_seconds AND
                 sc.physical_nanosecond <= t.physical_nanosecond)
            )"""
        elif direction == "backward":
            after_birth = """(
                sc.physical_seconds < p.birth_seconds OR
                (sc.physical_seconds = p.birth_seconds AND
                 sc.physical_nanosecond <= p.birth_nanosecond)
            )"""
            before_termination = """(
                t.particle_id IS NULL OR
                sc.physical_seconds > t.physical_seconds OR
                (sc.physical_seconds = t.physical_seconds AND
                 sc.physical_nanosecond >= t.physical_nanosecond)
            )"""
        else:
            raise ValueError(f"invalid direction {direction!r}")
        expected_schedule_cte = f"""{schedule_cte}, expected_schedule AS (
            SELECT p.run_id, p.particle_id,
                   sc.physical_seconds, sc.physical_nanosecond
            FROM particle p
            CROSS JOIN schedule sc
            LEFT JOIN termination t
              ON t.run_id = p.run_id AND t.particle_id = p.particle_id
            WHERE {after_birth} AND {before_termination}
        )"""
        actual_rows = int(connection.execute("SELECT COUNT(*) FROM particle_state").fetchone()[0])
        missing_scheduled, duplicate_scheduled = map(int, connection.execute(
            f"""WITH {schedule_cte}, event_counts AS (
                    SELECT sc.physical_seconds, sc.physical_nanosecond,
                           COUNT(e.event_sequence) AS event_count
                    FROM schedule sc
                    LEFT JOIN output_event e
                      ON e.physical_seconds = sc.physical_seconds
                     AND e.physical_nanosecond = sc.physical_nanosecond
                    GROUP BY sc.physical_seconds, sc.physical_nanosecond
                )
                SELECT COALESCE(SUM(event_count = 0), 0),
                       COALESCE(SUM(event_count > 1), 0)
                FROM event_counts"""
        ).fetchone())
        extra_events = int(connection.execute(
            f"""WITH {schedule_cte}
                SELECT COUNT(*) FROM output_event e
                WHERE NOT EXISTS (
                    SELECT 1 FROM schedule sc
                    WHERE sc.physical_seconds = e.physical_seconds
                      AND sc.physical_nanosecond = e.physical_nanosecond
                )"""
        ).fetchone()[0])
        invalid_extra_kinds = int(connection.execute(
            f"""WITH {schedule_cte}
                SELECT COUNT(*) FROM output_event e
                WHERE NOT EXISTS (
                    SELECT 1 FROM schedule sc
                    WHERE sc.physical_seconds = e.physical_seconds
                      AND sc.physical_nanosecond = e.physical_nanosecond
                )
                  AND e.event_kind NOT IN ('birth', 'termination')"""
        ).fetchone()[0])
        expected_scheduled_rows = int(connection.execute(
            f"WITH {expected_schedule_cte} SELECT COUNT(*) FROM expected_schedule"
        ).fetchone()[0])
        expected_rows = int(connection.execute(
            f"""WITH {expected_schedule_cte}, expected_state AS (
                    SELECT run_id, particle_id, physical_seconds, physical_nanosecond
                    FROM expected_schedule
                    UNION
                    SELECT run_id, particle_id, birth_seconds, birth_nanosecond
                    FROM particle
                    UNION
                    SELECT run_id, particle_id, physical_seconds, physical_nanosecond
                    FROM termination
                )
                SELECT COUNT(*) FROM expected_state"""
        ).fetchone()[0])
        invalid_contiguous = int(connection.execute(
            """WITH per_particle AS (
                   SELECT run_id, particle_id,
                          MIN(sample_sequence) first_sample,
                          MAX(sample_sequence) last_sample,
                          COUNT(*) state_count
                   FROM particle_state GROUP BY run_id, particle_id
               )
               SELECT COUNT(*) FROM particle p
               LEFT JOIN per_particle s
                 ON s.run_id = p.run_id AND s.particle_id = p.particle_id
               WHERE s.particle_id IS NULL OR s.first_sample != 0
                  OR s.state_count != s.last_sample + 1 OR s.last_sample < 0"""
        ).fetchone()[0])
        nonmonotonic_samples = int(connection.execute(
            """SELECT COUNT(DISTINCT current.particle_id)
               FROM particle_state current
               JOIN particle_state previous
                 ON previous.run_id = current.run_id
                AND previous.particle_id = current.particle_id
                AND previous.sample_sequence = current.sample_sequence - 1
               WHERE current.event_sequence <= previous.event_sequence"""
        ).fetchone()[0])
        birth_mismatches = int(connection.execute(
            """SELECT COUNT(*) FROM particle p
               WHERE (
                   SELECT COUNT(*) FROM particle_state s
                   WHERE s.run_id = p.run_id AND s.particle_id = p.particle_id
                     AND s.sample_sequence = 0
                     AND s.physical_seconds = p.birth_seconds
                     AND s.physical_nanosecond = p.birth_nanosecond
               ) != 1"""
        ).fetchone()[0])
        termination_mismatches = int(connection.execute(
            """WITH last_sample AS (
                   SELECT run_id, particle_id, MAX(sample_sequence) sample_sequence
                   FROM particle_state GROUP BY run_id, particle_id
               )
               SELECT COUNT(*) FROM termination t
               LEFT JOIN last_sample last
                 ON last.run_id = t.run_id AND last.particle_id = t.particle_id
               LEFT JOIN particle_state s
                 ON s.run_id = t.run_id AND s.particle_id = t.particle_id
                AND s.sample_sequence = last.sample_sequence
               WHERE s.particle_id IS NULL
                  OR s.physical_seconds != t.physical_seconds
                  OR s.physical_nanosecond != t.physical_nanosecond
                  OR s.particle_status != 'terminated'
                  OR s.termination_reason != t.reason"""
        ).fetchone()[0])
        run_end_seconds, run_end_nanosecond = schedule[-1]
        missing_final = int(connection.execute(
            """WITH last_sample AS (
                   SELECT run_id, particle_id, MAX(sample_sequence) sample_sequence
                   FROM particle_state GROUP BY run_id, particle_id
               )
               SELECT COUNT(*) FROM particle p
               LEFT JOIN termination t
                 ON t.run_id = p.run_id AND t.particle_id = p.particle_id
               LEFT JOIN last_sample last
                 ON last.run_id = p.run_id AND last.particle_id = p.particle_id
               LEFT JOIN particle_state s
                 ON s.run_id = p.run_id AND s.particle_id = p.particle_id
                AND s.sample_sequence = last.sample_sequence
               WHERE t.particle_id IS NULL
                 AND (s.particle_id IS NULL OR s.physical_seconds != ?
                      OR s.physical_nanosecond != ? OR s.particle_status != 'alive')""",
            (run_end_seconds, run_end_nanosecond),
        ).fetchone()[0])
        scheduled_state_mismatches = int(connection.execute(
            f"""WITH {expected_schedule_cte}
                SELECT COUNT(*) FROM (
                    SELECT expected.run_id, expected.particle_id,
                           expected.physical_seconds, expected.physical_nanosecond
                    FROM expected_schedule expected
                    LEFT JOIN particle_state actual
                      ON actual.run_id = expected.run_id
                     AND actual.particle_id = expected.particle_id
                     AND actual.physical_seconds = expected.physical_seconds
                     AND actual.physical_nanosecond = expected.physical_nanosecond
                    GROUP BY expected.run_id, expected.particle_id,
                             expected.physical_seconds, expected.physical_nanosecond
                    HAVING COUNT(actual.sample_sequence) != 1
                )"""
        ).fetchone()[0])
        unrelated_lifecycle_rows = int(connection.execute(
            f"""WITH {schedule_cte}
                SELECT COUNT(*) FROM particle_state s
                JOIN particle p
                  ON p.run_id = s.run_id AND p.particle_id = s.particle_id
                LEFT JOIN termination t
                  ON t.run_id = s.run_id AND t.particle_id = s.particle_id
                WHERE NOT EXISTS (
                    SELECT 1 FROM schedule sc
                    WHERE sc.physical_seconds = s.physical_seconds
                      AND sc.physical_nanosecond = s.physical_nanosecond
                )
                  AND NOT (s.physical_seconds = p.birth_seconds
                           AND s.physical_nanosecond = p.birth_nanosecond)
                  AND NOT (t.particle_id IS NOT NULL
                           AND s.physical_seconds = t.physical_seconds
                           AND s.physical_nanosecond = t.physical_nanosecond)"""
        ).fetchone()[0])
        state_event_time_mismatches = int(connection.execute(
            """SELECT COUNT(*) FROM particle_state s
               JOIN output_event e
                 ON e.run_id = s.run_id AND e.event_sequence = s.event_sequence
               WHERE s.physical_seconds != e.physical_seconds
                  OR s.physical_nanosecond != e.physical_nanosecond"""
        ).fetchone()[0])
        empty_output_events = int(connection.execute(
            """SELECT COUNT(*) FROM (
                   SELECT e.run_id, e.event_sequence
                   FROM output_event e
                   LEFT JOIN particle_state s
                     ON s.run_id = e.run_id AND s.event_sequence = e.event_sequence
                   GROUP BY e.run_id, e.event_sequence
                   HAVING COUNT(s.particle_id) = 0
               )"""
        ).fetchone()[0])
        invalid_event_sequence = int(connection.execute(
            """SELECT CASE WHEN COUNT(*) > 0 AND MIN(event_sequence) = 0
                                  AND COUNT(*) = MAX(event_sequence) + 1
                            THEN 0 ELSE 1 END
               FROM output_event"""
        ).fetchone()[0])
        result = {
            "scheduled_output_times_expected": len(schedule),
            "scheduled_output_times_present": len(schedule) - missing_scheduled,
            "missing_scheduled_output_times": missing_scheduled,
            "duplicate_scheduled_output_times": duplicate_scheduled,
            "extra_lifecycle_output_events": extra_events,
            "invalid_extra_event_kinds": invalid_extra_kinds,
            "expected_scheduled_state_rows": expected_scheduled_rows,
            "expected_lifecycle_only_state_rows": expected_rows - expected_scheduled_rows,
            "expected_rows_from_lifecycle": expected_rows,
            "actual_particle_state_rows": actual_rows,
            "invalid_sample_sequence_particles": invalid_contiguous,
            "nonmonotonic_sample_event_particles": nonmonotonic_samples,
            "birth_state_mismatches": birth_mismatches,
            "termination_state_mismatches": termination_mismatches,
            "unterminated_missing_final_state": missing_final,
            "scheduled_state_mismatches": scheduled_state_mismatches,
            "unrelated_lifecycle_state_rows": unrelated_lifecycle_rows,
            "state_event_time_mismatches": state_event_time_mismatches,
            "empty_output_events": empty_output_events,
            "invalid_output_event_sequence": invalid_event_sequence,
        }
        result["valid"] = (
            actual_rows == expected_rows
            and missing_scheduled == 0
            and duplicate_scheduled == 0
            and invalid_extra_kinds == 0
            and invalid_contiguous == 0
            and nonmonotonic_samples == 0
            and birth_mismatches == 0
            and termination_mismatches == 0
            and missing_final == 0
            and scheduled_state_mismatches == 0
            and unrelated_lifecycle_rows == 0
            and state_event_time_mismatches == 0
            and empty_output_events == 0
            and invalid_event_sequence == 0
        )
        return result
    finally:
        connection.close()


def sqlite_finite_audit(path: Path) -> dict[str, Any]:
    """Scan every REAL output column once per table for NaN/infinity."""
    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    try:
        by_table: dict[str, dict[str, int]] = {}
        total_nonfinite = 0
        for table in ("particle", "particle_mass", "particle_state", "termination"):
            columns = [
                str(row[1])
                for row in connection.execute(f'PRAGMA table_info("{table}")')
                if str(row[2]).upper() == "REAL"
            ]
            if not columns:
                continue
            expressions = [
                "SUM(CASE WHEN \"{column}\" IS NOT NULL AND "
                "(\"{column}\" != \"{column}\" OR "
                "abs(\"{column}\") > 1.7976931348623157e308) "
                "THEN 1 ELSE 0 END)".format(column=column)
                for column in columns
            ]
            row = connection.execute(
                f'SELECT {", ".join(expressions)} FROM "{table}"'
            ).fetchone()
            counts = {
                column: int(value or 0)
                for column, value in zip(columns, row or (), strict=True)
            }
            by_table[table] = counts
            total_nonfinite += sum(counts.values())
        return {
            "method": "single table scan over every REAL column",
            "nonfinite_by_table_and_column": by_table,
            "total_nonfinite": total_nonfinite,
            "valid": total_nonfinite == 0,
        }
    finally:
        connection.close()


def sqlite_interior_birth_time_count(
    path: Path,
    *,
    run_start_seconds: int,
    run_start_nanosecond: int,
    macro_step_seconds: int,
) -> int:
    """Count distinct births strictly inside the frozen macro-step lattice."""
    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    try:
        macro_step_ns = macro_step_seconds * 1_000_000_000
        row = connection.execute(
            """
            SELECT COUNT(*)
            FROM (
                SELECT birth_seconds, birth_nanosecond
                FROM particle
                WHERE (
                    ((birth_seconds - ?) * 1000000000)
                    + (birth_nanosecond - ?)
                ) % ? != 0
                GROUP BY birth_seconds, birth_nanosecond
            )
            """,
            (run_start_seconds, run_start_nanosecond, macro_step_ns),
        ).fetchone()
        return int(row[0])
    finally:
        connection.close()


def sqlite_population_audit(
    path: Path,
    *,
    direction: str,
    run_start_seconds: int,
    run_start_nanosecond: int,
    target_particle_count: int,
) -> dict[str, Any]:
    """Distinguish the initial domain-fill target from legitimate inflow births."""
    if direction == "forward":
        in_run_direction = """(
            p.birth_seconds > :start_seconds OR
            (p.birth_seconds = :start_seconds AND
             p.birth_nanosecond > :start_nanosecond)
        )"""
    elif direction == "backward":
        in_run_direction = """(
            p.birth_seconds < :start_seconds OR
            (p.birth_seconds = :start_seconds AND
             p.birth_nanosecond < :start_nanosecond)
        )"""
    else:
        raise ValueError(f"invalid direction {direction!r}")

    connection = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    try:
        parameters = {
            "start_seconds": run_start_seconds,
            "start_nanosecond": run_start_nanosecond,
        }
        counts = connection.execute(
            f"""
            SELECT
                COUNT(*) AS total_particles,
                COALESCE(SUM(
                    p.birth_seconds = :start_seconds AND
                    p.birth_nanosecond = :start_nanosecond
                ), 0) AS initial_particles,
                COALESCE(SUM(NOT (
                    p.birth_seconds = :start_seconds AND
                    p.birth_nanosecond = :start_nanosecond
                )), 0) AS inflow_particles,
                COALESCE(SUM(
                    p.birth_seconds = :start_seconds AND
                    p.birth_nanosecond = :start_nanosecond AND
                    p.origin_kind != 'domain_initial'
                ), 0) AS invalid_initial_origins,
                COALESCE(SUM(
                    NOT (
                        p.birth_seconds = :start_seconds AND
                        p.birth_nanosecond = :start_nanosecond
                    ) AND p.origin_kind != 'domain_boundary'
                ), 0) AS invalid_inflow_origins,
                COALESCE(SUM(
                    NOT (
                        p.birth_seconds = :start_seconds AND
                        p.birth_nanosecond = :start_nanosecond
                    ) AND NOT {in_run_direction}
                ), 0) AS invalid_inflow_birth_direction
            FROM particle p
            """,
            parameters,
        ).fetchone()
        if counts is None:
            raise ValueError("failed to count SQLite particles")
        (
            total_particles,
            initial_particles,
            inflow_particles,
            invalid_initial_origins,
            invalid_inflow_origins,
            invalid_inflow_birth_direction,
        ) = map(int, counts)

        birth_timestamp_counts = connection.execute(
            """
            WITH particle_birth_times AS (
                SELECT run_id, birth_seconds, birth_nanosecond
                FROM particle
                GROUP BY run_id, birth_seconds, birth_nanosecond
            ), birth_event_times AS (
                SELECT run_id, physical_seconds, physical_nanosecond,
                       COUNT(*) AS event_count
                FROM output_event
                WHERE event_kind = 'birth'
                GROUP BY run_id, physical_seconds, physical_nanosecond
            )
            SELECT
                (SELECT COUNT(*) FROM particle_birth_times),
                (SELECT COUNT(*) FROM output_event WHERE event_kind = 'birth'),
                (SELECT COUNT(*)
                 FROM particle_birth_times p
                 LEFT JOIN birth_event_times e
                   ON e.run_id = p.run_id
                  AND e.physical_seconds = p.birth_seconds
                  AND e.physical_nanosecond = p.birth_nanosecond
                 WHERE COALESCE(e.event_count, 0) != 1),
                (SELECT COUNT(*)
                 FROM birth_event_times e
                 WHERE NOT EXISTS (
                     SELECT 1 FROM particle_birth_times p
                     WHERE p.run_id = e.run_id
                       AND p.birth_seconds = e.physical_seconds
                       AND p.birth_nanosecond = e.physical_nanosecond
                 ))
            """
        ).fetchone()
        if birth_timestamp_counts is None:
            raise ValueError("failed to audit SQLite birth events")
        (
            distinct_birth_timestamps,
            birth_output_events,
            invalid_birth_event_multiplicity,
            orphan_birth_events,
        ) = map(int, birth_timestamp_counts)

        birth_state_event_mismatches = int(connection.execute(
            """
            SELECT COUNT(*)
            FROM particle p
            LEFT JOIN particle_state s
              ON s.run_id = p.run_id
             AND s.particle_id = p.particle_id
             AND s.sample_sequence = 0
            LEFT JOIN output_event e
              ON e.run_id = s.run_id
             AND e.event_sequence = s.event_sequence
            WHERE s.particle_id IS NULL
               OR e.event_sequence IS NULL
               OR e.event_kind != 'birth'
               OR e.physical_seconds != p.birth_seconds
               OR e.physical_nanosecond != p.birth_nanosecond
               OR s.physical_seconds != p.birth_seconds
               OR s.physical_nanosecond != p.birth_nanosecond
            """
        ).fetchone()[0])

        result = {
            "target_initial_particles": target_particle_count,
            "total_particles": total_particles,
            "initial_particles": initial_particles,
            "inflow_particles": inflow_particles,
            "invalid_initial_origins": invalid_initial_origins,
            "invalid_inflow_origins": invalid_inflow_origins,
            "invalid_inflow_birth_direction": invalid_inflow_birth_direction,
            "distinct_birth_timestamps": distinct_birth_timestamps,
            "birth_output_events": birth_output_events,
            "invalid_birth_event_multiplicity": invalid_birth_event_multiplicity,
            "orphan_birth_events": orphan_birth_events,
            "birth_state_event_mismatches": birth_state_event_mismatches,
        }
        result["valid"] = (
            initial_particles == target_particle_count
            and total_particles == initial_particles + inflow_particles
            and invalid_initial_origins == 0
            and invalid_inflow_origins == 0
            and invalid_inflow_birth_direction == 0
            and invalid_birth_event_multiplicity == 0
            and orphan_birth_events == 0
            and birth_state_event_mismatches == 0
        )
        return result
    finally:
        connection.close()


def validate_cell(
    cell_dir: Path,
    *,
    platform_name: str,
    direction: str,
    particles: int,
    workers: int,
    mode: str,
    returncode: int,
    require_performance_attribution: bool,
) -> dict[str, Any]:
    """Validate evidence without changing it; every requirement becomes a check."""
    failures: list[str] = []
    checks: dict[str, Any] = {
        "test_returncode": returncode,
        "family": FAMILY,
        "direction": direction,
        "particles": particles,
        "workers": workers,
        "mode": mode,
        "platform": platform_name,
    }
    command_path = cell_dir / "command.json"
    checks["command_artifact_present"] = command_path.is_file()
    checks["command_artifact_sha256"] = sha256(command_path)
    if not command_path.is_file():
        failures.append("missing command.json")
    else:
        try:
            checks["command_artifact"] = read_json(command_path)
        except Exception as exc:
            failures.append(f"command.json invalid JSON: {exc}")

    binary_path = cell_dir / "binary-identity.json"
    checks["binary_identity_present"] = binary_path.is_file()
    if not binary_path.is_file():
        failures.append("missing binary-identity.json")
    else:
        try:
            binary_identity = read_json(binary_path)
            checks["binary_identity"] = binary_identity
            if not isinstance(binary_identity.get("size"), int) or binary_identity["size"] <= 0:
                failures.append("binary identity has invalid size")
            if not isinstance(binary_identity.get("sha256"), str) \
                    or not SHA256_PATTERN.fullmatch(binary_identity["sha256"]):
                failures.append("binary identity has invalid SHA-256")
        except Exception as exc:
            failures.append(f"binary-identity.json invalid JSON: {exc}")

    peak_rss = parse_gnu_time(cell_dir / GNU_TIME_FILE_NAME)
    checks["peak_rss"] = peak_rss
    if platform_name == "wsl":
        if not peak_rss["present"] or not isinstance(peak_rss["peak_rss_bytes"], int) \
                or peak_rss["peak_rss_bytes"] <= 0:
            failures.append("missing/invalid GNU time peak RSS evidence")
        if mode == "formal" and particles == 100_000 \
                and isinstance(peak_rss["peak_rss_bytes"], int) \
                and peak_rss["peak_rss_bytes"] > PEAK_RSS_LIMIT_BYTES:
            failures.append(
                f"100k peak RSS={peak_rss['peak_rss_bytes']} > {PEAK_RSS_LIMIT_BYTES}"
            )

    reader_path = cell_dir / CONCURRENT_READER_FILE_NAME
    required_snapshots = required_active_snapshots(mode, particles)
    checks["concurrent_reader_required_active_snapshots"] = required_snapshots
    checks["concurrent_reader_present"] = reader_path.is_file()
    if platform_name == "wsl" and not reader_path.is_file():
        failures.append(f"missing {CONCURRENT_READER_FILE_NAME}")
    elif reader_path.is_file():
        try:
            reader = read_json(reader_path)
            checks["concurrent_reader"] = reader
            if reader.get("schema_version") != "trajecta.m4-a4-concurrent-reader/v1":
                failures.append("invalid concurrent-reader schema_version")
            observed = reader.get("successful_active_snapshots")
            if not isinstance(observed, int) or observed < required_snapshots:
                failures.append(
                    f"writer-active SQLite snapshots={observed!r}, required {required_snapshots}"
                )
            if reader.get("passed") is not True:
                failures.append("concurrent-reader monitor did not pass")
        except Exception as exc:
            failures.append(f"{CONCURRENT_READER_FILE_NAME} invalid JSON: {exc}")
    summary_path = cell_dir / "M4_A4_REAL_MATRIX_SUMMARY.json"
    if not summary_path.is_file():
        failures.append("missing M4_A4_REAL_MATRIX_SUMMARY.json")
        summary: dict[str, Any] = {}
    else:
        try:
            summary = read_json(summary_path)
            checks["summary_sha256"] = sha256(summary_path)
        except Exception as exc:
            summary = {}
            failures.append(f"summary invalid JSON: {exc}")
    for key, expected in (
        ("schema_version", "trajecta.m4-a4-real-matrix/v1"),
        ("mode", mode),
        ("target_particle_count", particles),
        ("worker_threads", workers),
        ("duration_seconds", DURATION_SECONDS),
        ("time_step_seconds", TIME_STEP_SECONDS),
        ("output_interval_seconds", OUTPUT_INTERVAL_SECONDS),
    ):
        actual = summary.get(key)
        checks[f"summary_{key}"] = actual
        if actual != expected:
            failures.append(f"summary {key}={actual!r}, expected {expected!r}")
    runs = summary.get("runs") if isinstance(summary, dict) else None
    if not isinstance(runs, list) or len(runs) != 1:
        failures.append("summary must contain exactly one run")
        run: dict[str, Any] = {}
    else:
        run = runs[0]
    for key, expected in (
        ("family", FAMILY),
        ("direction", direction),
        ("status", "complete"),
        ("numerical_steps", 6),
        ("abnormal_terminations", 0),
    ):
        actual = run.get(key)
        checks[f"run_{key}"] = actual
        if actual != expected:
            failures.append(f"run {key}={actual!r}, expected {expected!r}")
    seeded_particles = run.get("seeded_particles")
    final_particle_rows = run.get("final_particle_rows")
    checks["run_seeded_particles"] = seeded_particles
    checks["run_final_particle_rows"] = final_particle_rows
    if not isinstance(seeded_particles, int) or seeded_particles < particles:
        failures.append(
            f"run seeded_particles={seeded_particles!r}, expected at least {particles}"
        )
    if not isinstance(final_particle_rows, int) or final_particle_rows != seeded_particles:
        failures.append(
            "run final_particle_rows must equal seeded_particles: "
            f"{final_particle_rows!r}/{seeded_particles!r}"
        )
    output_event_rows = run.get("output_event_rows")
    checks["run_output_event_rows"] = output_event_rows
    if not isinstance(output_event_rows, int) or output_event_rows < 7:
        failures.append(
            f"run output_event_rows={output_event_rows!r}, expected at least 7"
        )
    state_rows = run.get("particle_state_rows")
    checks["particle_state_rows"] = state_rows
    timings = {
        "lock_build_preload_milliseconds": run.get("lock_build_preload_milliseconds"),
        "runner_run_milliseconds": run.get("run_milliseconds"),
        "test_process_elapsed_milliseconds": run.get("process_elapsed_milliseconds"),
    }
    checks["timings"] = timings
    if any(not isinstance(value, int) or value <= 0 for value in timings.values()):
        failures.append(f"missing/nonpositive run timing evidence: {timings}")

    output_identity = run.get("output_identity") if isinstance(run, dict) else None
    checks["output_identity"] = output_identity
    if not isinstance(output_identity, dict) or output_identity.get("valid") is not True:
        failures.append("manifest/bundle/SQLite normalized identity did not recompute from disk")
    else:
        normalized = {
            "content_sha256": output_identity.get("recomputed_content_sha256"),
            "sqlite_sql_sha256": output_identity.get("recomputed_sqlite_sql_sha256"),
            "canonical_output_sha256": output_identity.get(
                "recomputed_canonical_output_sha256"
            ),
        }
        checks["normalized_output_digests"] = normalized
        for name, value in normalized.items():
            if not isinstance(value, str) or not SHA256_PATTERN.fullmatch(value):
                failures.append(f"invalid recomputed normalized digest {name}={value!r}")

    performance = run.get("performance_attribution") if isinstance(run, dict) else None
    checks["performance_attribution_present"] = isinstance(performance, dict)
    if require_performance_attribution:
        if not isinstance(performance, dict):
            failures.append("missing A4.5 performance attribution")
        else:
            if performance.get("schema_version") != "trajecta.m4-a4.5-performance-attribution/v1":
                failures.append("invalid A4.5 performance schema_version")
            stages = performance.get("stages") or {}
            distributions = performance.get("distributions") or {}
            for name in ("runner_total", "runner_boundary", "boundary_query_total"):
                observation = stages.get(name)
                if not isinstance(observation, dict) or observation.get("observations", 0) <= 0:
                    failures.append(f"missing/non-empty A4.5 stage {name}")
            observation = distributions.get("boundary_segments_per_path")
            if not isinstance(observation, dict) or observation.get("observations", 0) <= 0:
                failures.append("missing/non-empty A4.5 boundary path distribution")
            for group_name, group in (("stage", stages), ("distribution", distributions)):
                for name, observation in group.items():
                    histogram = observation.get("log2_histogram") if isinstance(observation, dict) else None
                    count = observation.get("observations") if isinstance(observation, dict) else None
                    if not isinstance(histogram, list) or len(histogram) != 64 \
                            or not isinstance(count, int) or sum(histogram) != count:
                        failures.append(f"invalid A4.5 {group_name} histogram {name}")

    io = run.get("io") if isinstance(run, dict) else None
    execute_delta = io.get("execute_delta") if isinstance(io, dict) else None
    checks["execute_io_delta"] = execute_delta
    if not isinstance(execute_delta, dict) or any(value != 0 for value in execute_delta.values()):
        failures.append("execute-phase reader/provider I/O is nonzero or unmeasured")
    reported_gate = run.get("particle_loop_query_gate") if isinstance(run, dict) else None
    queries = run.get("queries") if isinstance(run, dict) else None
    post_queries = queries.get("after_run") if isinstance(queries, dict) else None
    checks["query_counts"] = post_queries
    if not isinstance(post_queries, dict):
        failures.append("missing exact query-key metrics")
    else:
        logical = post_queries.get("logical_requests")
        executed = post_queries.get("executed_batches")
        reused = post_queries.get("exact_key_reuses")
        if not all(isinstance(value, int) for value in (logical, executed, reused)) \
                or logical <= 0 or logical != executed + reused:
            failures.append(f"query logical/executed/reused invalid: {logical!r}/{executed!r}/{reused!r}")
        origins = post_queries.get("by_origin") or {}
        integrator = origins.get("integrator") or {}
        output = origins.get("output") or {}
        bulk_logical = integrator.get("logical_requests", 0) + output.get("logical_requests", 0)
        bulk_executed = integrator.get("executed_batches", 0) + output.get("executed_batches", 0)
        bulk_reused = integrator.get("exact_key_reuses", 0) + output.get("exact_key_reuses", 0)
        exact = post_queries.get("particle_loop_exact") or {}
        checks["particle_loop_query_gate"] = reported_gate
        if not isinstance(reported_gate, dict) or reported_gate.get("valid") is not True:
            failures.append("particle-loop scale-aware query gate is missing or failed")
        else:
            interior_birth_times = reported_gate.get("interior_birth_time_count")
            expected_integrator = reported_gate.get(
                "expected_integrator_logical_requests"
            )
            observed_integrator = reported_gate.get(
                "observed_integrator_logical_requests"
            )
            expected_output = reported_gate.get("expected_output_logical_requests")
            observed_output = reported_gate.get("observed_output_logical_requests")
            if not isinstance(interior_birth_times, int) or interior_birth_times < 0:
                failures.append(
                    f"invalid interior birth-time count={interior_birth_times!r}"
                )
            elif expected_integrator != 2 * (6 + interior_birth_times):
                failures.append(
                    "integrator query expectation does not equal "
                    f"2*(6+interior_birth_times): {expected_integrator!r}"
                )
            if expected_output != 7:
                failures.append(f"expected output logical requests={expected_output!r}, expected 7")
            if observed_integrator != integrator.get("logical_requests"):
                failures.append("reported/observed integrator logical requests differ")
            if observed_output != output.get("logical_requests"):
                failures.append("reported/observed output logical requests differ")
            if reported_gate.get("logical_requests") != bulk_logical \
                    or reported_gate.get("executed_batches") != bulk_executed \
                    or reported_gate.get("exact_key_reuses") != bulk_reused:
                failures.append("reported particle-loop totals differ from origin counters")
            if bulk_logical != bulk_executed + bulk_reused:
                failures.append(
                    f"particle-loop logical/executed/reused={bulk_logical}/"
                    f"{bulk_executed}/{bulk_reused}"
                )
            if exact.get("unique_exact_keys") != bulk_executed:
                failures.append(
                    "particle-loop unique executed keys="
                    f"{exact.get('unique_exact_keys')!r}, executed={bulk_executed}"
                )
            if exact.get("repeated_exact_executions") != 0:
                failures.append(
                    "particle-loop repeated exact executions="
                    f"{exact.get('repeated_exact_executions')!r}"
                )

    run_dir = find_one_run_dir(cell_dir, failures)
    checks["run_dir"] = str(run_dir) if run_dir else None
    if run_dir:
        artifact_files = direct_artifact_files(run_dir)
        for name, path in artifact_files.items():
            checks[f"{name}_present"] = path.is_file()
            checks[f"{name}_sha256"] = sha256(path)
            if not path.is_file():
                failures.append(f"missing {name}")
        sqlite_path = run_dir / "particles.sqlite"
        wal_path = run_dir / "particles.sqlite-wal"
        shm_path = run_dir / "particles.sqlite-shm"
        bundle_path = run_dir / "provenance-bundle.json"
        sizes = {
            "particles_sqlite_bytes": file_size(sqlite_path),
            "particles_sqlite_wal_bytes": file_size(wal_path),
            "particles_sqlite_shm_bytes": file_size(shm_path),
            "provenance_bundle_bytes": file_size(bundle_path),
            "run_directory_bytes": directory_size(run_dir),
        }
        checks["artifact_sizes"] = sizes
        if sizes["particles_sqlite_wal_bytes"] != 0:
            failures.append(
                "terminal particles.sqlite-wal is nonempty: "
                f"{sizes['particles_sqlite_wal_bytes']} bytes"
            )
        if mode == "formal" and particles == 100_000 \
                and sizes["particles_sqlite_bytes"] > SQLITE_LIMIT_BYTES:
            failures.append(
                f"100k particles.sqlite={sizes['particles_sqlite_bytes']} > {SQLITE_LIMIT_BYTES}"
            )

        sqlite_ok, sqlite_message = sqlite_integrity(sqlite_path)
        checks["sqlite_integrity_check"] = sqlite_message
        if not sqlite_ok:
            failures.append(f"sqlite integrity_check={sqlite_message!r}")
        if sqlite_path.is_file():
            try:
                lifecycle = sqlite_lifecycle_audit(sqlite_path, direction)
                checks["sqlite_lifecycle_coverage"] = lifecycle
                if not lifecycle["valid"]:
                    failures.append(f"invalid SQLite lifecycle coverage: {lifecycle}")
            except Exception as exc:
                failures.append(f"SQLite lifecycle audit failed: {exc}")
            try:
                finite = sqlite_finite_audit(sqlite_path)
                checks["sqlite_finite_audit"] = finite
                if not finite["valid"]:
                    failures.append(f"nonfinite SQLite output values: {finite}")
            except Exception as exc:
                failures.append(f"SQLite finite audit failed: {exc}")
        performance_path = run_dir / "A4_5_STAGE_TIMINGS.json"
        checks["A4_5_STAGE_TIMINGS.json_present"] = performance_path.is_file()
        checks["A4_5_STAGE_TIMINGS.json_sha256"] = sha256(performance_path)
        if require_performance_attribution and not performance_path.is_file():
            failures.append("missing A4_5_STAGE_TIMINGS.json")

        manifest_payload: dict[str, Any] | None = None
        resolved_case_payload: dict[str, Any] | None = None
        for name in ("run-manifest.json", "resolved-case.json", "resolved-run-profile.json"):
            path = run_dir / name
            if path.is_file():
                try:
                    payload = read_json(path)
                    if not isinstance(payload, dict):
                        raise ValueError("top-level JSON must be an object")
                    checks[f"{name}_valid_json"] = True
                    if name == "run-manifest.json":
                        manifest_payload = payload
                        if str(payload.get("status", "")).lower() != "complete":
                            failures.append("manifest.status not complete")
                        terms = payload.get("terminations") or {}
                        if terms.get("abnormal_count") != 0:
                            failures.append("manifest abnormal_count nonzero")
                        ledger = payload.get("mass_ledger") or []
                        if not ledger:
                            failures.append("manifest mass_ledger empty")
                        else:
                            violations = sum(
                                abs(float(row.get("imbalance_kg", 0)))
                                > float(row.get("tolerance_kg", 0))
                                for row in ledger
                            )
                            checks["mass_ledger_violations"] = violations
                            if violations:
                                failures.append(f"mass ledger tolerance violations={violations}")
                    elif name == "resolved-case.json":
                        resolved_case_payload = payload
                except Exception as exc:
                    checks[f"{name}_valid_json"] = False
                    failures.append(f"{name} invalid JSON: {exc}")
        population_audit: dict[str, Any] | None = None
        if resolved_case_payload is not None and sqlite_path.is_file():
            try:
                start = resolved_case_payload["time"]["start"]
                population_audit = sqlite_population_audit(
                    sqlite_path,
                    direction=direction,
                    run_start_seconds=int(start["seconds_since_unix_epoch"]),
                    run_start_nanosecond=int(start["nanosecond"]),
                    target_particle_count=particles,
                )
                checks["sqlite_population_audit"] = population_audit
                if not population_audit["valid"]:
                    failures.append(
                        f"invalid SQLite population lifecycle: {population_audit}"
                    )
                if population_audit["total_particles"] != seeded_particles:
                    failures.append(
                        "SQLite total particle rows differ from run seeded_particles: "
                        f"{population_audit['total_particles']}/{seeded_particles!r}"
                    )
                if population_audit["total_particles"] != final_particle_rows:
                    failures.append(
                        "SQLite total particle rows differ from run final_particle_rows: "
                        f"{population_audit['total_particles']}/{final_particle_rows!r}"
                    )
            except Exception as exc:
                failures.append(f"SQLite population audit failed: {exc}")
        if resolved_case_payload is not None and isinstance(reported_gate, dict) \
                and sqlite_path.is_file():
            try:
                start = resolved_case_payload["time"]["start"]
                sqlite_interior = sqlite_interior_birth_time_count(
                    sqlite_path,
                    run_start_seconds=int(start["seconds_since_unix_epoch"]),
                    run_start_nanosecond=int(start["nanosecond"]),
                    macro_step_seconds=TIME_STEP_SECONDS,
                )
                checks["sqlite_interior_birth_time_count"] = sqlite_interior
                if sqlite_interior != reported_gate.get("interior_birth_time_count"):
                    failures.append(
                        "SQLite/reported interior birth-time count differs: "
                        f"{sqlite_interior}/{reported_gate.get('interior_birth_time_count')!r}"
                    )
            except Exception as exc:
                failures.append(f"SQLite interior birth-time audit failed: {exc}")
        checks["provenance-bundle.json_streaming_valid"] = (
            isinstance(output_identity, dict) and output_identity.get("valid") is True
        )
        if manifest_payload is not None:
            provenance = manifest_payload.get("provenance")
            checks["manifest_provenance"] = provenance
            if not isinstance(provenance, dict):
                failures.append("manifest provenance identity missing")
            else:
                exact_bundle_sha = checks.get("provenance-bundle.json_sha256")
                exact_sqlite_sha = checks.get("particles.sqlite_sha256")
                if provenance.get("sha256") != exact_bundle_sha:
                    failures.append("manifest provenance.sha256 != on-disk bundle SHA")
                if provenance.get("sqlite_sha256") != exact_sqlite_sha:
                    failures.append("manifest provenance.sqlite_sha256 != on-disk SQLite SHA")
                for key in (
                    "content_sha256",
                    "sqlite_sql_sha256",
                    "canonical_output_sha256",
                ):
                    value = provenance.get(key)
                    if not isinstance(value, str) or not SHA256_PATTERN.fullmatch(value):
                        failures.append(f"manifest provenance {key} invalid")
                if isinstance(output_identity, dict):
                    comparisons = {
                        "sha256": "disk_bundle_sha256",
                        "sqlite_sha256": "disk_sqlite_sha256",
                        "content_sha256": "recomputed_content_sha256",
                        "sqlite_sql_sha256": "recomputed_sqlite_sql_sha256",
                        "canonical_output_sha256": "recomputed_canonical_output_sha256",
                    }
                    for manifest_key, evidence_key in comparisons.items():
                        if provenance.get(manifest_key) != output_identity.get(evidence_key):
                            failures.append(
                                f"manifest provenance {manifest_key} != independent {evidence_key}"
                            )
                manifest_rows = (manifest_payload.get("sqlite") or {}).get("row_counts")
                checks["manifest_sqlite_row_counts"] = manifest_rows
                if isinstance(manifest_rows, dict):
                    expected_particle_rows = (
                        population_audit["total_particles"]
                        if isinstance(population_audit, dict)
                        else final_particle_rows
                    )
                    if manifest_rows.get("particle") != expected_particle_rows:
                        failures.append(
                            "manifest SQLite particle row count mismatch: "
                            f"{manifest_rows.get('particle')!r}/{expected_particle_rows!r}"
                        )
                    if manifest_rows.get("output_event") != output_event_rows:
                        failures.append("manifest SQLite output_event row count mismatch")
                    if manifest_rows.get("particle_state") != state_rows:
                        failures.append("manifest SQLite particle_state row count mismatch")
                else:
                    failures.append("manifest SQLite row_counts missing")
    if returncode != 0:
        failures.append(f"test returncode={returncode}")
    checks["failures"] = failures
    checks["passed"] = not failures
    return checks


def cargo_command() -> list[str]:
    return [
        "cargo", "test", "--offline", "--release", "-p", TEST_PACKAGE,
        "--test", TEST_NAME, TEST_FILTER, "--", "--ignored", "--nocapture", "--exact",
    ]


def environment(
    base: dict[str, str], *, mode: str, direction: str, particles: int, workers: int,
    artifact_dir: str, observe_performance: bool,
) -> dict[str, str]:
    env = base.copy()
    env.update({
        "TRAJECTA_M4_A4_MODE": mode,
        "TRAJECTA_M4_A4_PARTICLES": str(particles),
        "TRAJECTA_M4_A4_DURATION_SECONDS": str(DURATION_SECONDS),
        "TRAJECTA_M4_A4_TIME_STEP_SECONDS": str(TIME_STEP_SECONDS),
        "TRAJECTA_M4_A4_OUTPUT_INTERVAL_SECONDS": str(OUTPUT_INTERVAL_SECONDS),
        "TRAJECTA_M4_A4_WORKERS": str(workers),
        "TRAJECTA_M4_A4_FAMILY": FAMILY,
        "TRAJECTA_M4_A4_DIRECTION": direction,
        "TRAJECTA_M4_A4_ARTIFACT_DIR": artifact_dir,
        "TRAJECTA_REQUIRE_REAL_MET": "1",
        "TRAJECTA_M4_A4_5_OBSERVE": "1" if observe_performance else "0",
    })
    return env


def to_wsl_path(path: Path) -> str:
    resolved = path.resolve()
    if not resolved.drive:
        raise RuntimeError(f"Windows path required for WSL artifact: {resolved}")
    return "/mnt/" + resolved.drive[0].lower() + resolved.as_posix()[2:]


def run_windows(
    cell_dir: Path, *, mode: str, direction: str, particles: int, workers: int,
    observe_performance: bool,
) -> tuple[int, str, str]:
    compile_cmd = ["cargo", "test", "--offline", "--release", "-p", TEST_PACKAGE,
                   "--test", TEST_NAME, "--no-run"]
    compiled = subprocess.run(compile_cmd, cwd=ROOT, text=True, capture_output=True)
    compile_stdout, compile_stderr = compiled.stdout or "", compiled.stderr or ""
    if compiled.returncode != 0:
        return compiled.returncode, compile_stdout, compile_stderr
    candidates = sorted((ROOT / "target" / "release" / "deps").glob("m4_a4_real_perf-*.exe"),
                        key=lambda path: path.stat().st_mtime)
    if not candidates:
        return 1, compile_stdout, compile_stderr + "\nno direct Windows A4 test binary found\n"
    binary = candidates[-1]
    write_json(cell_dir / "binary-identity.json", {"path": str(binary.resolve()), "size": binary.stat().st_size,
                                                     "sha256": sha256(binary), "compile_command": compile_cmd})
    proc = subprocess.run(
        [str(binary), TEST_FILTER, "--ignored", "--nocapture", "--exact"], cwd=ROOT,
        env=environment(os.environ, mode=mode, direction=direction, particles=particles, workers=workers,
                        artifact_dir=str(cell_dir.resolve()), observe_performance=observe_performance),
        text=True, capture_output=True,
        encoding="utf-8", errors="replace",
    )
    return proc.returncode, "=== compile --no-run ===\n" + compile_stdout + "\n=== direct test binary ===\n" + (proc.stdout or ""), compile_stderr + "\n" + (proc.stderr or "")


def run_wsl(
    cell_dir: Path, *, mode: str, direction: str, particles: int, workers: int,
    target_dir: str, distro: str | None, observe_performance: bool,
) -> tuple[int, str, str]:
    linux_root = "/mnt/e/flexpart/trajecta"
    artifact = to_wsl_path(cell_dir)
    monitor_required = required_active_snapshots(mode, particles)
    values = environment({}, mode=mode, direction=direction, particles=particles, workers=workers,
                         artifact_dir=artifact, observe_performance=observe_performance)
    exports = "\n".join(f"export {key}={json.dumps(value)}" for key, value in values.items())
    script = f"""set -euo pipefail
export PATH="/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
export LC_ALL=C
export CARGO_TARGET_DIR={json.dumps(target_dir)}
{exports}
cd {json.dumps(linux_root)}
uname -a
rustc -V
cargo -V
git rev-parse HEAD
git status --short
test -x /usr/bin/time
test -f tools/monitor_m4_a4_wsl_cell.py
cargo test --offline --release -p {TEST_PACKAGE} --test {TEST_NAME} --no-run
binary=$(find "$CARGO_TARGET_DIR/release/deps" -maxdepth 1 -type f -name 'm4_a4_real_perf-*' -perm -111 | sort | tail -n 1)
test -n "$binary"
printf 'direct_binary=%s\\n' "$binary"
printf 'direct_binary_size=%s\\n' "$(stat -c %s "$binary")"
binary_sha=$(sha256sum "$binary")
binary_sha=${{binary_sha%% *}}
printf 'direct_binary_sha256=%s\\n' "$binary_sha"
set +e
/usr/bin/time -v -o "$TRAJECTA_M4_A4_ARTIFACT_DIR/{GNU_TIME_FILE_NAME}" \\
  "$binary" {TEST_FILTER} --ignored --nocapture --exact &
measured_pid=$!
python3 tools/monitor_m4_a4_wsl_cell.py \\
  --artifact-dir "$TRAJECTA_M4_A4_ARTIFACT_DIR" \\
  --process-pid "$measured_pid" \\
  --required-active-snapshots {monitor_required} \\
  --output "$TRAJECTA_M4_A4_ARTIFACT_DIR/{CONCURRENT_READER_FILE_NAME}" \\
  > "$TRAJECTA_M4_A4_ARTIFACT_DIR/concurrent-reader.stdout.log" \\
  2> "$TRAJECTA_M4_A4_ARTIFACT_DIR/concurrent-reader.stderr.log" &
monitor_pid=$!
wait "$measured_pid"
test_rc=$?
wait "$monitor_pid"
monitor_rc=$?
set -e
printf 'direct_test_returncode=%s\\n' "$test_rc"
printf 'concurrent_reader_returncode=%s\\n' "$monitor_rc"
if [ "$test_rc" -ne 0 ]; then
  exit "$test_rc"
fi
exit "$monitor_rc"
"""
    # Pass a durable LF shell file rather than a `bash -lc` string: WSL's
    # Windows argv conversion otherwise rewrites /tmp target paths and corrupts
    # the direct binary's LD_LIBRARY_PATH.
    runner = cell_dir / "wsl-run.sh"
    runner.write_bytes(script.replace("\r\n", "\n").replace("\r", "\n").encode("utf-8"))
    command = ["wsl.exe"] + (["-d", distro] if distro else []) + ["--", "bash", to_wsl_path(runner)]
    proc = subprocess.run(command, cwd=ROOT, text=True, capture_output=True,
                          encoding="utf-8", errors="replace")
    stdout = proc.stdout or ""
    markers: dict[str, str] = {}
    for line in stdout.splitlines():
        for key in (
            "direct_binary",
            "direct_binary_size",
            "direct_binary_sha256",
            "direct_test_returncode",
            "concurrent_reader_returncode",
        ):
            prefix = key + "="
            if line.startswith(prefix):
                markers[key] = line[len(prefix):]
    try:
        binary_size = int(markers["direct_binary_size"])
    except (KeyError, ValueError):
        binary_size = None
    write_json(
        cell_dir / "binary-identity.json",
        {
            "platform": "wsl",
            "path": markers.get("direct_binary"),
            "size": binary_size,
            "sha256": markers.get("direct_binary_sha256"),
            "compile_command": [
                "cargo", "test", "--offline", "--release", "-p", TEST_PACKAGE,
                "--test", TEST_NAME, "--no-run",
            ],
            "direct_arguments": [TEST_FILTER, "--ignored", "--nocapture", "--exact"],
            "direct_test_returncode": markers.get("direct_test_returncode"),
            "concurrent_reader_returncode": markers.get("concurrent_reader_returncode"),
            "sha256_method": "sha256sum inside the selected WSL distro",
        },
    )
    return proc.returncode, stdout, proc.stderr or ""


def execute_cell(
    artifact_root: Path, *, platform_name: str, mode: str, direction: str, particles: int, workers: int,
    wsl_target_dir: str, wsl_distro: str | None, observe_performance: bool,
    source_identity: dict[str, Any],
) -> dict[str, Any]:
    cid = cell_id(platform_name, direction, particles, workers)
    cell_dir = next_attempt_directory(artifact_root, cid)
    cell_dir.mkdir(parents=True, exist_ok=False)
    identity = host_identity(platform_name)
    identity["source_tree"] = source_identity
    write_json(cell_dir / "host-identity.json", identity)
    artifact_environment = environment(
        {},
        mode=mode,
        direction=direction,
        particles=particles,
        workers=workers,
        artifact_dir=(
            to_wsl_path(cell_dir) if platform_name == "wsl" else str(cell_dir.resolve())
        ),
        observe_performance=observe_performance,
    )
    command_record = {
        "schema_version": "trajecta.m4-a4-command/v1",
        "platform": platform_name,
        "compile_command": [
            "cargo", "test", "--offline", "--release", "-p", TEST_PACKAGE,
            "--test", TEST_NAME, "--no-run",
        ],
        "direct_test_arguments": [TEST_FILTER, "--ignored", "--nocapture", "--exact"],
        "environment": artifact_environment,
        "wsl_target_dir": wsl_target_dir if platform_name == "wsl" else None,
        "wsl_distro": wsl_distro if platform_name == "wsl" else None,
        "required_active_sqlite_snapshots": required_active_snapshots(mode, particles),
        "source_tree": source_identity,
    }
    write_json(cell_dir / "command.json", command_record)
    started = time.monotonic()
    started_utc = utc_now()
    print(f"[run] {cid}", flush=True)
    if platform_name == "windows":
        returncode, stdout, stderr = run_windows(
            cell_dir, mode=mode, direction=direction, particles=particles, workers=workers,
            observe_performance=observe_performance,
        )
    else:
        returncode, stdout, stderr = run_wsl(
            cell_dir, mode=mode, direction=direction, particles=particles, workers=workers,
            target_dir=wsl_target_dir, distro=wsl_distro,
            observe_performance=observe_performance,
        )
    (cell_dir / "stdout.log").write_text(stdout, encoding="utf-8", errors="replace")
    (cell_dir / "stderr.log").write_text(stderr, encoding="utf-8", errors="replace")
    validation = validate_cell(cell_dir, platform_name=platform_name,
                               direction=direction, particles=particles, workers=workers,
                               mode=mode, returncode=returncode,
                               require_performance_attribution=observe_performance)
    result = {
        "schema_version": "trajecta.m4-a4-cell-summary/v1",
        "cell_id": cid, "platform": platform_name, "family": FAMILY, "direction": direction,
        "particles": particles, "workers": workers, "mode": mode, "implemented": True,
        "executed": True, "status": "passed" if validation["passed"] else "failed",
        "attempt": cell_dir.name,
        "source_tree": source_identity,
        "started_utc": started_utc, "finished_utc": utc_now(), "elapsed_seconds": time.monotonic() - started,
        "returncode": returncode, "command": command_record, "artifact_dir": str(cell_dir),
        "checks": {key: value for key, value in validation.items()
                   if key not in {"failures", "passed"}},
        "failures": validation["failures"],
        "validation": validation,
    }
    write_json(cell_dir / CELL_SUMMARY_FILE_NAME, result)
    write_json(cell_dir / "cell-result.json", result)
    print(f"  status={result['status']} returncode={returncode} elapsed={result['elapsed_seconds']:.1f}s", flush=True)
    return result


def resume_candidate(
    artifact_root: Path,
    *,
    platform_name: str,
    mode: str,
    direction: str,
    particles: int,
    workers: int,
    source_identity: dict[str, Any],
) -> dict[str, Any] | None:
    cid = cell_id(platform_name, direction, particles, workers)
    def attempt_number(path: Path) -> int:
        suffix = path.parent.name.removeprefix("attempt-")
        return int(suffix) if suffix.isdigit() else -1

    candidates = sorted(
        (artifact_root / "cells" / cid).glob(f"attempt-*/{CELL_SUMMARY_FILE_NAME}"),
        key=attempt_number,
        reverse=True,
    )
    for path in candidates:
        try:
            payload = read_json(path)
        except Exception:
            continue
        if not isinstance(payload, dict) or payload.get("status") != "passed":
            continue
        if payload.get("schema_version") != "trajecta.m4-a4-cell-summary/v1":
            continue
        expected = {
            "platform": platform_name,
            "mode": mode,
            "direction": direction,
            "particles": particles,
            "workers": workers,
            "family": FAMILY,
        }
        if any(payload.get(key) != value for key, value in expected.items()):
            continue
        prior_source = payload.get("source_tree")
        if not isinstance(prior_source, dict) or prior_source.get("source_tree_sha256") \
                != source_identity.get("source_tree_sha256"):
            continue
        validation = payload.get("validation")
        if not isinstance(validation, dict) or validation.get("passed") is not True:
            continue
        payload["resumed"] = True
        payload["resumed_summary_path"] = str(path)
        payload["resumed_summary_sha256"] = sha256(path)
        return payload
    return None


def formal_phase_audit(results: list[dict[str, Any]]) -> dict[str, Any]:
    failures: list[str] = []
    expected = {(direction, particles, workers) for direction, particles, workers in FORMAL_CELLS}
    by_cell = {
        (result.get("direction"), result.get("particles"), result.get("workers")): result
        for result in results
    }
    missing = sorted(expected - set(by_cell))
    if missing:
        failures.append(f"formal matrix incomplete; missing={missing}")
    failed = [result.get("cell_id") for result in results if result.get("status") != "passed"]
    if failed:
        failures.append(f"formal cells failed={failed}")

    scaling: dict[str, Any] = {}
    determinism: dict[str, Any] = {}
    for direction in DIRECTIONS:
        small = by_cell.get((direction, 50_000, 4))
        large = by_cell.get((direction, 100_000, 4))
        if small and large:
            small_ms = ((small.get("checks") or {}).get("timings") or {}).get(
                "runner_run_milliseconds"
            )
            large_ms = ((large.get("checks") or {}).get("timings") or {}).get(
                "runner_run_milliseconds"
            )
            ratio = (
                float(large_ms) / float(small_ms)
                if isinstance(small_ms, int) and small_ms > 0 and isinstance(large_ms, int)
                else None
            )
            scaling[direction] = {
                "particles_50k_runner_milliseconds": small_ms,
                "particles_100k_runner_milliseconds": large_ms,
                "ratio": ratio,
                "limit": 2.4,
                "passed": ratio is not None and ratio <= 2.4,
            }
            if ratio is None or ratio > 2.4:
                failures.append(f"{direction} 50k->100k scaling ratio={ratio!r} > 2.4")

        w4 = by_cell.get((direction, 100_000, 4))
        w1 = by_cell.get((direction, 100_000, 1))
        if w4 and w1:
            digests_w4 = (w4.get("checks") or {}).get("normalized_output_digests")
            digests_w1 = (w1.get("checks") or {}).get("normalized_output_digests")
            equal = isinstance(digests_w4, dict) and digests_w4 == digests_w1
            determinism[direction] = {
                "workers_4": digests_w4,
                "workers_1": digests_w1,
                "passed": equal,
            }
            if not equal:
                failures.append(f"{direction} 1-worker/4-worker normalized digests differ")

    return {
        "schema_version": "trajecta.m4-a4-formal-audit/v1",
        "expected_cell_count": len(FORMAL_CELLS),
        "observed_cell_count": len(results),
        "scaling": scaling,
        "determinism": determinism,
        "failures": failures,
        "passed": not failures and len(results) == len(FORMAL_CELLS),
    }


def phase_summary(root: Path, phase: str, results: list[dict[str, Any]]) -> tuple[Path, dict[str, Any] | None]:
    path = root / "summary" / f"{phase}-{utc_now().replace(':', '').replace('-', '')}.json"
    aggregate = formal_phase_audit(results) if phase == "formal" else None
    write_json(path, {"schema_version": "trajecta.m4-a4-execution-summary/v1", "phase": phase,
                      "generated_at_utc": utc_now(), "results": results,
                      "aggregate": aggregate})
    return path, aggregate


def plan(args: argparse.Namespace) -> list[tuple[str, str, int, int]]:
    platform_name = args.platform
    if args.phase == "preflight":
        return [(platform_name, "forward", 1_000, 4)]
    if args.phase == "diagnostic":
        return [(platform_name, "forward", 10_000, 4)]
    if args.phase == "attribution":
        attribution_workers = (args.workers,) if args.workers is not None else (1, 4)
        if any(workers not in (1, 4) for workers in attribution_workers):
            raise SystemExit("attribution --workers must be 1 or 4")
        return [
            (platform_name, "forward", 1_000, workers)
            for _ in range(args.repetitions)
            for workers in attribution_workers
        ]
    if args.phase == "formal":
        return [(platform_name, direction, particles, workers)
                for direction, particles, workers in FORMAL_CELLS]
    if args.phase == "single":
        candidate = (args.direction, args.particles, args.workers)
        if candidate not in FORMAL_CELLS:
            raise SystemExit("single requires one of the frozen formal cells")
        return [(platform_name, args.direction, args.particles, args.workers)]
    raise AssertionError(args.phase)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "phase", choices=("preflight", "diagnostic", "attribution", "formal", "single")
    )
    parser.add_argument("--platform", choices=("windows", "wsl"), required=True)
    parser.add_argument("--direction")
    parser.add_argument("--particles", type=int)
    parser.add_argument("--workers", type=int)
    parser.add_argument("--artifact-root", type=Path)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--wsl-target-dir", default="/tmp/trajecta-m4-a4-target")
    parser.add_argument("--wsl-distro")
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--stop-on-fail", dest="stop_on_fail", action="store_true", default=True)
    parser.add_argument("--continue-on-fail", dest="stop_on_fail", action="store_false")
    args = parser.parse_args()
    if args.repetitions < 1 or args.repetitions > 10:
        raise SystemExit("--repetitions must be in 1..10")
    if args.artifact_root is None:
        args.artifact_root = ROOT / "target" / (
            "m4-a4.5" if args.phase == "attribution" else "m4-a4"
        )
    if args.platform == "windows" and os.name != "nt":
        raise SystemExit("--platform windows must run under Windows; do not mislabel a WSL result")
    if args.phase in {"preflight", "diagnostic", "attribution", "formal"} \
            and args.platform != "wsl":
        raise SystemExit(
            "A4 frozen preflight/attribution/formal matrix is WSL-only; "
            "Windows may run only an explicit tool dry-run"
        )
    source_identity = source_tree_identity()
    results: list[dict[str, Any]] = []
    final_aggregate: dict[str, Any] | None = None
    for platform_name, direction, particles, workers in plan(args):
        mode = (
            "preflight" if args.phase == "preflight"
            else "diagnostic" if args.phase == "diagnostic"
            else "attribution" if args.phase == "attribution"
            else "formal"
        )
        result = resume_candidate(
            args.artifact_root,
            platform_name=platform_name,
            mode=mode,
            direction=direction,
            particles=particles,
            workers=workers,
            source_identity=source_identity,
        ) if args.resume else None
        if result is None:
            result = execute_cell(args.artifact_root, platform_name=platform_name,
                                  mode=mode,
                                  direction=direction, particles=particles, workers=workers,
                                  wsl_target_dir=args.wsl_target_dir, wsl_distro=args.wsl_distro,
                                  observe_performance=args.phase == "attribution",
                                  source_identity=source_identity)
        else:
            print(
                f"[resume] {result['cell_id']} from {result['resumed_summary_path']}",
                flush=True,
            )
        results.append(result)
        summary, final_aggregate = phase_summary(args.artifact_root, args.phase, results)
        print(f"  phase summary: {summary}", flush=True)
        if result["status"] != "passed" and args.stop_on_fail:
            break
    cells_passed = all(result["status"] == "passed" for result in results)
    aggregate_passed = args.phase != "formal" or (
        isinstance(final_aggregate, dict) and final_aggregate.get("passed") is True
    )
    return 0 if cells_passed and aggregate_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
