"""Aggregate frozen M4-A4.5 timing/scaling attempts without judging algorithms."""
from __future__ import annotations

import argparse
import hashlib
import json
import statistics
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "testdata" / "M4_A4_5_PERFORMANCE_ATTRIBUTION.schema.json"
TOP_LEVEL_STAGES = (
    "runner_population",
    "runner_integrator",
    "runner_boundary",
    "runner_output",
)


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def median(values: list[float]) -> float | None:
    return statistics.median(values) if values else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-root", type=Path, default=ROOT / "target" / "m4-a4.5")
    parser.add_argument(
        "--baseline-root",
        type=Path,
        help="optional instrumentation-off preflight artifact root",
    )
    parser.add_argument("--required-repetitions", type=int, default=3)
    args = parser.parse_args()
    if args.required_repetitions < 1:
        raise SystemExit("--required-repetitions must be positive")
    try:
        import jsonschema
    except ImportError as exc:
        raise SystemExit(f"jsonschema is required: {exc}") from exc

    schema = load_json(SCHEMA)
    attempts: list[dict[str, Any]] = []
    blockers: list[str] = []
    pattern = "cells/wsl__era5-hybrid__forward__p1000__w*/attempt-*/M4_A4_REAL_MATRIX_SUMMARY.json"
    for summary_path in sorted(args.artifact_root.glob(pattern)):
        summary = load_json(summary_path)
        runs = summary.get("runs") or []
        if len(runs) != 1:
            blockers.append(f"{summary_path}: expected one run")
            continue
        run = runs[0]
        performance = run.get("performance_attribution")
        try:
            jsonschema.Draft202012Validator(schema).validate(performance)
        except Exception as exc:
            blockers.append(f"{summary_path}: invalid performance artifact: {exc}")
            continue
        worker_threads = int(summary["worker_threads"])
        runner_total_ns = int(performance["stages"]["runner_total"]["total"])
        top_level_ns = sum(int(performance["stages"][name]["total"]) for name in TOP_LEVEL_STAGES)
        attempt = {
            "summary_path": str(summary_path.relative_to(args.artifact_root)),
            "summary_sha256": sha256(summary_path),
            "worker_threads": worker_threads,
            "run_id": run["run_id"],
            "run_milliseconds": int(run["run_milliseconds"]),
            "runner_total_nanoseconds": runner_total_ns,
            "top_level_accounted_fraction": (
                top_level_ns / runner_total_ns if runner_total_ns else None
            ),
            "stages": {
                name: {
                    "observations": value["observations"],
                    "total_nanoseconds": value["total"],
                    "maximum_nanoseconds": value["maximum"],
                    "runner_fraction": value["total"] / runner_total_ns if runner_total_ns else None,
                }
                for name, value in performance["stages"].items()
            },
            "distributions": {
                name: {
                    "observations": value["observations"],
                    "total": value["total"],
                    "maximum": value["maximum"],
                }
                for name, value in performance["distributions"].items()
            },
        }
        attempts.append(attempt)

    groups: dict[str, Any] = {}
    for workers in (1, 4):
        selected = [attempt for attempt in attempts if attempt["worker_threads"] == workers]
        if len(selected) < args.required_repetitions:
            blockers.append(
                f"workers={workers}: {len(selected)} valid attempts, "
                f"required {args.required_repetitions}"
            )
        stage_names = sorted(selected[0]["stages"]) if selected else []
        groups[str(workers)] = {
            "valid_attempts": len(selected),
            "median_run_milliseconds": median(
                [float(attempt["run_milliseconds"]) for attempt in selected]
            ),
            "median_top_level_accounted_fraction": median(
                [float(attempt["top_level_accounted_fraction"]) for attempt in selected]
            ),
            "median_stage_nanoseconds": {
                name: median(
                    [float(attempt["stages"][name]["total_nanoseconds"]) for attempt in selected]
                )
                for name in stage_names
            },
        }

    one = groups["1"]["median_run_milliseconds"]
    four = groups["4"]["median_run_milliseconds"]
    baseline_attempts: list[dict[str, Any]] = []
    instrumentation_overhead = None
    if args.baseline_root is not None:
        baseline_pattern = (
            "cells/wsl__era5-hybrid__forward__p1000__w4/"
            "attempt-*/M4_A4_REAL_MATRIX_SUMMARY.json"
        )
        for summary_path in sorted(args.baseline_root.glob(baseline_pattern)):
            summary = load_json(summary_path)
            runs = summary.get("runs") or []
            if summary.get("mode") != "preflight" or summary.get("worker_threads") != 4:
                blockers.append(f"{summary_path}: expected preflight workers=4")
                continue
            if len(runs) != 1:
                blockers.append(f"{summary_path}: expected one baseline run")
                continue
            run = runs[0]
            if run.get("status") != "complete":
                blockers.append(f"{summary_path}: baseline run is not complete")
                continue
            if run.get("performance_attribution") is not None:
                blockers.append(f"{summary_path}: baseline unexpectedly enabled attribution")
                continue
            run_milliseconds = int(run.get("run_milliseconds") or 0)
            if run_milliseconds <= 0:
                blockers.append(f"{summary_path}: invalid baseline run_milliseconds")
                continue
            baseline_attempts.append(
                {
                    "summary_path": str(summary_path.relative_to(args.baseline_root)),
                    "summary_sha256": sha256(summary_path),
                    "run_id": run["run_id"],
                    "run_milliseconds": run_milliseconds,
                }
            )
        if len(baseline_attempts) < args.required_repetitions:
            blockers.append(
                f"instrumentation-off baseline: {len(baseline_attempts)} valid attempts, "
                f"required {args.required_repetitions}"
            )
        baseline_median = median(
            [float(attempt["run_milliseconds"]) for attempt in baseline_attempts]
        )
        overhead_fraction = (
            four / baseline_median - 1.0 if four and baseline_median else None
        )
        instrumentation_overhead = {
            "baseline_root": str(args.baseline_root),
            "valid_attempts": len(baseline_attempts),
            "median_baseline_run_milliseconds": baseline_median,
            "median_observed_run_milliseconds": four,
            "overhead_fraction": overhead_fraction,
            "goal_maximum_fraction": 0.03,
            "within_goal": (
                overhead_fraction <= 0.03 if overhead_fraction is not None else None
            ),
        }
    output = {
        "schema_version": "trajecta.m4-a4.5-performance-summary/v1",
        "status": "measured" if not blockers else "incomplete",
        "required_repetitions": args.required_repetitions,
        "attempts": attempts,
        "groups": groups,
        "worker_1_to_4_speedup": one / four if one and four else None,
        "baseline_attempts": baseline_attempts,
        "instrumentation_overhead": instrumentation_overhead,
        "blockers": blockers,
        "interpretation_owner": "A",
        "completion_claim": "none",
    }
    output_path = args.artifact_root / "summary" / "M4_A4_5_PERFORMANCE_ATTRIBUTION.json"
    write_json(output_path, output)
    print(json.dumps({"status": output["status"], "path": str(output_path)}))
    return 0 if output["status"] == "measured" else 2


if __name__ == "__main__":
    raise SystemExit(main())
