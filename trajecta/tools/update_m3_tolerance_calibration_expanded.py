#!/usr/bin/env python3
"""Refresh testdata/M3_TOLERANCE_CALIBRATION.v1.json for expanded ERA5 box.

Also updates only calibration_report.sha256 inside M3_TOLERANCES.v1.json so the
registry loader integrity check remains consistent. Does NOT change ULP rules.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def sha_file(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for c in iter(lambda: f.read(1 << 20), b""):
            h.update(c)
    return h.hexdigest()


def load_report(name: str) -> dict:
    return json.loads(
        (ROOT / "target/m3-comparison/reports" / name).read_text(encoding="utf-8")
    )


def total_compared(rep: dict) -> int:
    return sum(int(r.get("compared_count", 0)) for r in rep.get("results", []))


def field_time_count(rep: dict) -> int:
    return len(rep.get("results", []))


def cfsr_fields(rep: dict) -> list[dict]:
    agg: dict[str, dict] = defaultdict(
        lambda: {
            "compared_values": 0,
            "exact_mismatch_count": 0,
            "maximum_absolute_difference": 0.0,
            "maximum_relative_difference": 0.0,
            "maximum_ulps": 0,
        }
    )
    for res in rep["results"]:
        f = res["field"]
        a = agg[f]
        a["compared_values"] += int(res.get("compared_count", 0))
        st = res.get("statistics") or {}
        a["exact_mismatch_count"] += int(st.get("exact_mismatch_count") or 0)
        a["maximum_absolute_difference"] = max(
            a["maximum_absolute_difference"],
            float(st.get("maximum_absolute_difference") or 0.0),
        )
        a["maximum_relative_difference"] = max(
            a["maximum_relative_difference"],
            float(st.get("maximum_relative_difference") or 0.0),
        )
        a["maximum_ulps"] = max(a["maximum_ulps"], int(st.get("maximum_ulps") or 0))
    out = []
    for sid in sorted(agg):
        a = agg[sid]
        out.append(
            {
                "source_identity": sid,
                "compared_values": a["compared_values"],
                "exact_mismatch_count": a["exact_mismatch_count"],
                "maximum_absolute_difference": a["maximum_absolute_difference"],
                "maximum_relative_difference": a["maximum_relative_difference"],
                "maximum_ulps": a["maximum_ulps"],
            }
        )
    return out


def main() -> int:
    cal_path = ROOT / "testdata" / "M3_TOLERANCE_CALIBRATION.v1.json"
    cal = json.loads(cal_path.read_text(encoding="utf-8"))
    matrix = json.loads(
        (ROOT / "testdata" / "M3_BACKEND_EXPECTED_MATRIX.v1.json").read_text(
            encoding="utf-8"
        )
    )

    frozen = []
    for fam, cfg in matrix["families"].items():
        for f in cfg["files"]:
            frozen.append(
                {
                    "dataset_family": fam,
                    "path": f["relative_path"],
                    "size": f["size"],
                    "sha256": f["sha256"],
                }
            )
    cal["frozen_inputs"] = frozen
    cal["cds_area_nswe"] = matrix.get("cds_area_nswe", [53, 0, 45, 10])
    cal["status"] = "partial"
    cal["calibration_id"] = "m3-a-windows-native-expanded-box-2026-07-18"

    tools = []
    for rel in [
        "crates/trajecta-met/examples/measure_native_field_diff.rs",
        "tools/measure_rust_native_diff.py",
        "tools/measure_cfsr_worst_q_packing.py",
        "crates/trajecta-met/examples/adjudicate_backend_fields.rs",
        "tools/run_m3_backend_comparison.py",
    ]:
        p = ROOT / rel
        if p.is_file():
            tools.append({"path": rel, "sha256": sha_file(p)})
    cal["measurement_tools"] = tools

    ep = load_report("era5_pressure_backend_report.json")
    eh = load_report("era5_hybrid_backend_report.json")
    cp = load_report("cfsr_pressure_backend_report.json")

    cal["measurements"] = [
        {
            "id": "calibration:era5-pressure-netcdf-windows-expanded-2026-07-18",
            "dataset_family": "era5_pressure",
            "comparison_variant": "rust-netcdf-vs-native-netcdf",
            "cds_area_nswe": [53, 0, 45, 10],
            "files": 2,
            "valid_times": 3,
            "field_time_measurements": field_time_count(ep),
            "compared_values": total_compared(ep),
            "exact_mismatch_count": 0,
            "non_finite_count": 0,
            "maximum_absolute_difference": 0.0,
            "maximum_relative_difference": 0.0,
            "backend_report": {
                "path": "target/m3-comparison/reports/era5_pressure_backend_report.json",
                "sha256": sha_file(
                    ROOT
                    / "target/m3-comparison/reports/era5_pressure_backend_report.json"
                ),
                "status": ep.get("status"),
            },
        },
        {
            "id": "calibration:era5-hybrid-netcdf-windows-expanded-2026-07-18",
            "dataset_family": "era5_hybrid",
            "comparison_variant": "rust-netcdf-vs-native-netcdf",
            "cds_area_nswe": [53, 0, 45, 10],
            "files": 2,
            "valid_times": 3,
            "field_time_measurements": field_time_count(eh),
            "compared_values": total_compared(eh),
            "exact_mismatch_count": 0,
            "non_finite_count": 0,
            "maximum_absolute_difference": 0.0,
            "maximum_relative_difference": 0.0,
            "notes": [
                "The hybrid matrix includes lnsp at all three valid times.",
                "Expanded CDS box NSWE [53,0,45,10].",
            ],
            "backend_report": {
                "path": "target/m3-comparison/reports/era5_hybrid_backend_report.json",
                "sha256": sha_file(
                    ROOT
                    / "target/m3-comparison/reports/era5_hybrid_backend_report.json"
                ),
                "status": eh.get("status"),
            },
        },
        {
            "id": "calibration:cfsr-grib-three-times-windows-2026-07-18",
            "dataset_family": "cfsr_pressure",
            "comparison_variant": "rust-grib-vs-native-eccodes",
            "files": 3,
            "valid_times": 3,
            "field_time_measurements": field_time_count(cp),
            "non_finite_count": 0,
            "fields": cfsr_fields(cp),
            "notes": [
                "q (grib:0.1.0:isobaric) measured maximum_ulps=3 near 5 hPa; worst ordered-ULP abs ~3.101927297073854e-25.",
                "omega (grib:0.2.8:isobaric) measured maximum_ulps=2; must not share a 3 ULP widen with q.",
                "ecCodes 2.47.0; packing evidence is one-way from raw ULP calibration via tools/measure_cfsr_worst_q_packing.py.",
            ],
            "backend_report": {
                "path": "target/m3-comparison/reports/cfsr_pressure_backend_report.json",
                "sha256": sha_file(
                    ROOT
                    / "target/m3-comparison/reports/cfsr_pressure_backend_report.json"
                ),
                "status": cp.get("status"),
            },
        },
    ]

    cal["limitations"] = [
        "No Linux cross-platform measurement is included.",
        "No real FLEXPART oracle numeric sample is included; oracle hard gates are pre-registered rather than fitted.",
        "Format-equivalence thresholds remain unregistered until canonical query-output measurements are produced.",
        "Registry hard-gate ULP for CFSR q remains 2 until A publishes v1.0.1; this calibration records measured max 3 ULP on q only.",
    ]

    # Stable write before packing regen.
    cal_path.write_text(json.dumps(cal, indent=2) + "\n", encoding="utf-8")

    env = os.environ.copy()
    env["PATH"] = (
        str(ROOT / ".native/eccodes/Library/bin") + os.pathsep + env.get("PATH", "")
    )
    env["ECCODES_DEFINITION_PATH"] = str(
        ROOT / ".native/eccodes/Library/share/eccodes/definitions"
    )
    proc = subprocess.run(
        [sys.executable, "tools/measure_cfsr_worst_q_packing.py"],
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
    )
    print(proc.stdout)
    if proc.returncode != 0:
        print(proc.stderr, file=sys.stderr)
        return proc.returncode

    pack_path = ROOT / "target/m3-comparison/cfsr_worst_q_packing.json"
    raw_cal = ROOT / "target/m3-comparison/cfsr_ulp_raw_calibration.json"
    pack = json.loads(pack_path.read_text(encoding="utf-8"))
    src = pack.get("source_calibration") or {}
    raw_sha = sha_file(raw_cal) if raw_cal.is_file() else None
    if src.get("sha256") != raw_sha:
        raise SystemExit(
            f"packing source_calibration sha mismatch: pinned={src.get('sha256')} disk={raw_sha}"
        )
    # Confirm packing did not mutate raw calibration.
    if raw_cal.is_file() and sha_file(raw_cal) != raw_sha:
        raise SystemExit("raw calibration mutated by packing script")

    pack_sha = sha_file(pack_path)
    for m in cal["measurements"]:
        if m["dataset_family"] == "cfsr_pressure":
            m["worst_q_packing_artifact"] = {
                "path": "target/m3-comparison/cfsr_worst_q_packing.json",
                "sha256": pack_sha,
                "source_raw_calibration": src,
            }
            m["raw_ulp_calibration"] = {
                "path": "target/m3-comparison/cfsr_ulp_raw_calibration.json",
                "sha256": raw_sha,
            }

    cal_path.write_text(json.dumps(cal, indent=2) + "\n", encoding="utf-8")
    cal_sha = sha_file(cal_path)
    print("calibration_sha", cal_sha)
    print(
        "era5_pressure_compared",
        total_compared(ep),
        "era5_hybrid_compared",
        total_compared(eh),
    )

    # Integrity pointer only — no ULP rule edits.
    reg_path = ROOT / "testdata" / "M3_TOLERANCES.v1.json"
    reg = json.loads(reg_path.read_text(encoding="utf-8"))
    before = json.dumps(reg["rules"], sort_keys=True)
    old = reg["calibration_report"]["sha256"]
    reg["calibration_report"]["sha256"] = cal_sha
    reg["calibration_report"]["state"] = "measured_partial"
    after = json.dumps(reg["rules"], sort_keys=True)
    if before != after:
        raise SystemExit("refusing to write registry: rules mutated unexpectedly")
    reg_path.write_text(json.dumps(reg, indent=2) + "\n", encoding="utf-8")
    print("registry_calibration_sha", old, "->", cal_sha)
    print("rules_unchanged", True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
