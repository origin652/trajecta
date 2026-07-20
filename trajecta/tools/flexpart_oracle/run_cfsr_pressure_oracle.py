#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run FLEXPART pressure-meter oracle for frozen CFSR PGBl GRIB inputs."""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from run_pressure_meter_oracle import (
    write_identity_sidecar,  # noqa: E402
    FLEXPART_COMMIT,
    SCHEMA,
    MANIFEST,
    HARNESS_VERSION,
    build_document,
    build_driver_queries,
    build_records,
    git_head,
    parse_driver_output,
    require_file,
    run_driver,
    sha256_file,
    validate,
    write_driver_query_file,
)

FLEXPART_ROOT = ROOT.parent / "flexpart"
DEFAULT_QUERY = ROOT / "target/m3-oracle/queries/cfsr_pressure_queries.json"
DEFAULT_BINARY = (
    ROOT / "target/m3-oracle/build/pressure-meter/bin/pressure_meter_oracle_driver"
)
DEFAULT_OUTPUT = ROOT / "target/m3-oracle/artifacts/cfsr_pressure_oracle.json"
PROFILE = ROOT / "crates/trajecta-met/profiles/cfsr_pgbl_pressure_v0.yaml"
GRIB_DIR = ROOT / "target/test-data/cfsr-ncei-pgbl-official"
GRIBS = [
    GRIB_DIR / "pgbl00.gdas.2009010100.grb2",
    GRIB_DIR / "pgbl00.gdas.2009010106.grb2",
    GRIB_DIR / "pgbl00.gdas.2009010112.grb2",
]
ADAPTER = ROOT / "tools/flexpart_oracle/cfsr_grib_to_pressure_netcdf.py"
WORK = ROOT / "target/m3-oracle/work/cfsr-pressure"
PRESSURE_NC = WORK / "cfsr_pressure_oracle_adapter.nc"
SURFACE_NC = WORK / "cfsr_surface_oracle_adapter.nc"


def convert_grib() -> None:
    env = os.environ.copy()
    native = ROOT / ".native/eccodes/Library/bin"
    env["PATH"] = str(native) + os.pathsep + env.get("PATH", "")
    env["ECCODES_DEFINITION_PATH"] = str(
        ROOT / ".native/eccodes/Library/share/eccodes/definitions"
    )
    # Prefer env with eccodes python bindings when available.
    pythons = [
        Path(r"C:/Users/dell/miniforge3/envs/wrf_xesmf_env/python.exe"),
        Path(sys.executable),
    ]
    python = next((p for p in pythons if p.is_file()), Path(sys.executable))
    cmd = [
        str(python),
        str(ADAPTER),
        "--pressure-out",
        str(PRESSURE_NC),
        "--surface-out",
        str(SURFACE_NC),
    ]
    for grib in GRIBS:
        cmd.extend(["--grib", str(grib)])
    print("+", " ".join(cmd), flush=True)
    result = subprocess.run(cmd, cwd=ROOT, env=env, text=True, capture_output=True)
    (WORK / "cfsr_convert.log").parent.mkdir(parents=True, exist_ok=True)
    (WORK / "cfsr_convert.log").write_text(
        f"exit={result.returncode}\n[stdout]\n{result.stdout}\n[stderr]\n{result.stderr}\n",
        encoding="utf-8",
        newline="\n",
    )
    if result.returncode != 0:
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        raise SystemExit(f"CFSR GRIB adapter failed; see {WORK / 'cfsr_convert.log'}")
    print(result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--queries", type=Path, default=DEFAULT_QUERY)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--skip-convert", action="store_true")
    args = parser.parse_args()

    for path, label in (
        (args.queries, "formal queries"),
        (args.binary, "oracle driver binary"),
        (SCHEMA, "oracle schema"),
        (MANIFEST, "real-met manifest"),
        (PROFILE, "CFSR profile"),
        (ADAPTER, "grib adapter"),
    ):
        require_file(path, label)
    for grib in GRIBS:
        require_file(grib, "CFSR grib")
    if git_head(FLEXPART_ROOT) != FLEXPART_COMMIT:
        raise SystemExit("FLEXPART checkout is not at the frozen oracle commit")

    if not args.skip_convert:
        convert_grib()
    require_file(PRESSURE_NC, "adapter pressure nc")
    require_file(SURFACE_NC, "adapter surface nc")

    query_doc = json.loads(args.queries.read_text(encoding="utf-8"))
    if query_doc.get("dataset_family") != "cfsr_pressure":
        raise SystemExit("query document is not cfsr_pressure")
    if query_doc.get("flexpart_commit") != FLEXPART_COMMIT:
        raise SystemExit("query document FLEXPART commit drift")
    if query_doc.get("status") != "frozen_terrain_selected":
        raise SystemExit("only formal terrain-selected queries are accepted")

    rows, supported, _metadata = build_driver_queries(query_doc, PRESSURE_NC)
    driver_queries = WORK / "driver_queries.txt"
    raw_output = WORK / "driver_output.tsv"
    driver_log = WORK / "driver.log"
    write_driver_query_file(driver_queries, rows)
    driver_ok = run_driver(
        args.binary,
        PRESSURE_NC,
        SURFACE_NC,
        driver_queries,
        raw_output,
        driver_log,
        source_family="cfsr",
        vertical_coordinate="pressure",
        pbl_height_mode="official_prescribed",
    )
    if driver_ok:
        driver_values = parse_driver_output(raw_output)
        if set(driver_values) != set(supported):
            missing = sorted(set(supported) - set(driver_values))
            extra = sorted(set(driver_values) - set(supported))
            raise SystemExit(
                f"driver coverage mismatch missing={missing[:3]} extra={extra[:3]}"
            )
        records = build_records(query_doc, driver_values, supported)
    else:
        from run_pressure_meter_oracle import vertical_selector

        records = []
        for query in query_doc["records"]:
            support_meta = supported.get(query["point_id"])
            records.append(
                {
                    "point_id": query["point_id"],
                    "sample_scope": query["sample_scope"],
                    "time_unix_seconds": int(query["time_unix"]),
                    "time_nanosecond": 0,
                    "longitude_degrees": float(query["longitude_degrees"]),
                    "latitude_degrees": float(query["latitude_degrees"]),
                    "vertical_selector": vertical_selector(query, support_meta),
                    "status": "oracle_failure",
                    "fields": {},
                    "diagnostic": {
                        "code": "oracle.flexpart_path_failed",
                        "message": f"driver failed; see {driver_log.as_posix()}",
                    },
                }
            )
    document = build_document(
        PRESSURE_NC,
        SURFACE_NC,
        args.queries,
        args.binary,
        query_doc,
        records,
        source_family="cfsr",
        dataset_family="cfsr_pressure",
    )
    # Override family identity: GRIB sources + profile + adapter provenance.
    document["input"]["dataset_family"] = "cfsr_pressure"
    document["input"]["profile_sha256"] = sha256_file(PROFILE)
    # MIT adjudicator verifies these names under the CFSR data root.
    document["input"]["files"] = [
        {
            "role": "grib",
            "name": path.name,
            "size": path.stat().st_size,
            "sha256": sha256_file(path),
        }
        for path in GRIBS
    ]
    document["oracle"]["loader_kind"] = "oracle_adapter_eccodes"
    document["oracle"]["loader_sha256"] = sha256_file(ADAPTER)
    document["oracle"]["build_mode"] = "pressure_meter"
    # Preserve machine-readable identity tokens from build_document harness_version.
    base_hv = document["oracle"]["harness_version"]
    tokens = base_hv.split(";", 1)
    identity_suffix = (";" + tokens[1]) if len(tokens) > 1 else (
        ";adapter=pressure_meter_adapter"
        ";source_family=cfsr;vertical_coordinate=pressure"
        ";pbl_height_mode=official_prescribed;vertical_transform=verttransform_gfs"
    )
    document["oracle"]["harness_version"] = HARNESS_VERSION + "+cfsr-eccodes" + identity_suffix
    # Rebuild harness hash to include adapter.
    from run_pressure_meter_oracle import sha256_sources

    document["oracle"]["harness_sha256"] = sha256_sources(
        [
            ROOT / "tools/flexpart_oracle/src/pressure_meter_oracle_driver.f90",
            ROOT / "tools/flexpart_oracle/src/oracle_calcpar_mod.f90",
            ROOT / "tools/flexpart_oracle/build_pressure_meter_driver.sh",
            Path(__file__).resolve(),
            ADAPTER,
        ]
    )
    if not driver_ok:
        document["status"] = "failed"
        document["failures"] = [
            {
                "code": "oracle.flexpart_path_failed",
                "message": f"CFSR driver failed; see {driver_log.as_posix()}; no silent richardson fallback",
            }
        ]
    validate(document)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    part = args.output.with_suffix(args.output.suffix + ".part")
    part.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    validate(json.loads(part.read_text(encoding="utf-8")))
    os.replace(part, args.output)
    write_identity_sidecar(args.output, document)
    ok_count = sum(record["status"] == "ok" for record in records)
    print(
        json.dumps(
            {
                "status": document["status"],
                "records": len(records),
                "ok": ok_count,
                "not_available": len(records) - ok_count,
                "output": str(args.output),
                "sha256": sha256_file(args.output),
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
