"""Validate one frozen M4-A4.5 stage-timing artifact."""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "testdata" / "M4_A4_5_PERFORMANCE_ATTRIBUTION.schema.json"


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    args = parser.parse_args()
    try:
        import jsonschema
    except ImportError as exc:
        print(f"jsonschema is required: {exc}", file=sys.stderr)
        return 2

    try:
        payload = load_json(args.artifact)
        schema = load_json(SCHEMA)
        jsonschema.Draft202012Validator(schema).validate(payload)
    except Exception as exc:
        print(f"invalid A4.5 performance artifact: {exc}", file=sys.stderr)
        return 1

    failures: list[str] = []
    for group_name in ("stages", "distributions"):
        for name, observation in payload[group_name].items():
            if sum(observation["log2_histogram"]) != observation["observations"]:
                failures.append(f"{group_name}.{name}: histogram count mismatch")
            if observation["observations"] == 0 and (
                observation["total"] != 0 or observation["maximum"] != 0
            ):
                failures.append(f"{group_name}.{name}: empty observation has nonzero values")
            if observation["maximum"] > observation["total"]:
                failures.append(f"{group_name}.{name}: maximum exceeds total")

    runner_total = payload["stages"]["runner_total"]
    if runner_total["observations"] != 1 or runner_total["total"] <= 0:
        failures.append("stages.runner_total must contain exactly one positive observation")
    for name in ("runner_boundary", "boundary_query_total"):
        if payload["stages"][name]["observations"] <= 0:
            failures.append(f"stages.{name} must be non-empty")
    if payload["distributions"]["boundary_segments_per_path"]["observations"] <= 0:
        failures.append("distributions.boundary_segments_per_path must be non-empty")

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    print(json.dumps({
        "status": "valid",
        "schema_version": payload["schema_version"],
        "runner_total_nanoseconds": runner_total["total"],
        "boundary_paths": payload["distributions"]["boundary_segments_per_path"]["observations"],
    }, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
