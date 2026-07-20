#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run FLEXPART ETA hybrid-137 oracle for frozen ERA5 CDS hybrid inputs."""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(Path(__file__).resolve().parent))

from run_pressure_meter_oracle import (  # noqa: E402
    FLEXPART_COMMIT,
    SCHEMA,
    MANIFEST,
    HARNESS_VERSION,
    build_dir_from_binary,
    build_document,
    build_records,
    exact_index,
    git_head,
    load_compile_identity,
    write_identity_sidecar,
    parse_driver_output,
    require_file,
    run_driver,
    sha256_file,
    sha256_sources,
    validate,
    write_driver_query_file,
)

FLEXPART_ROOT = ROOT.parent / "flexpart"
DEFAULT_HYBRID = (
    ROOT
    / "target/test-data/era5-cds-hybrid137-official/ready/era5_hybrid137_prepared_20181201.nc"
)
DEFAULT_SURFACE = (
    ROOT
    / "target/test-data/era5-cds-hybrid137-official/ready/era5_surface_20181201.nc"
)
DEFAULT_QUERIES = ROOT / "target/m3-oracle/queries/era5_hybrid_queries.json"
DEFAULT_BINARY = ROOT / "target/m3-oracle/build/eta-hybrid/bin/eta_hybrid_oracle_driver"
DEFAULT_OUTPUT = ROOT / "target/m3-oracle/artifacts/era5_hybrid_oracle.json"
PROFILE = ROOT / "crates/trajecta-met/profiles/era5_cds_hybrid137_v0.yaml"


def build_hybrid_driver_queries(
    query_doc: dict, hybrid_path: Path
) -> tuple[list[str], dict[str, dict], dict]:
    with netCDF4.Dataset(hybrid_path.as_posix()) as dataset:
        times = np.asarray(dataset.variables["valid_time"][:], dtype=np.int64)
        hybrid = np.asarray(dataset.variables["hybrid"][:], dtype=float)
        latitudes = np.asarray(dataset.variables["latitude"][:], dtype=float)
        longitudes = np.asarray(dataset.variables["longitude"][:], dtype=float)

    if len(times) < 2 or not np.all(np.diff(times) > 0):
        raise SystemExit("hybrid valid_time must be strictly increasing")
    # hybrid levels are 1..137 from top.
    if not np.allclose(hybrid, np.arange(1, len(hybrid) + 1)):
        raise SystemExit("hybrid coordinate must be 1..N from top")

    row_entries: list[tuple[int, int, int, str, str]] = []
    supported: dict[str, dict] = {}
    base_time = int(times[0])
    vertical_codes = {
        "above_ground": 1,
        "above_sea_level": 2,
        "pressure": 3,
    }
    scope_kind = {
        "native_anchor": 1,
        "interpolated_common": 2,
        "surface_layer": 3,
        "modern_difference": 4,
    }

    def bracket_time(point_id: str, time_value: int) -> tuple[int, int]:
        insertion = int(np.searchsorted(times, time_value, side="right"))
        left = insertion - 1
        right = insertion
        if left < 0 or right >= len(times) or not (
            int(times[left]) < time_value < int(times[right])
        ):
            raise SystemExit(f"{point_id}: time is not strictly bracketed")
        return left, right

    for record in query_doc["records"]:
        scope = record["sample_scope"]
        point_id = record["point_id"]
        if scope not in scope_kind:
            raise SystemExit(f"{point_id}: unsupported sample_scope {scope}")
        time_value = int(record["time_unix"])
        time_relative = time_value - base_time
        lon = float(record["longitude_degrees"])
        lat = float(record["latitude_degrees"])

        if scope == "native_anchor":
            time_index = exact_index(times, time_value, "valid_time", 0.0)
            # vertical is 1-based native full level from top.
            level_from_top = int(record["vertical"])
            if level_from_top < 1 or level_from_top > len(hybrid):
                raise SystemExit(f"{point_id}: native level {level_from_top} out of range")
            exact_index(longitudes, lon, "longitude", 1.0e-7)
            exact_index(latitudes, lat, "latitude", 1.0e-7)
            row = (
                f"1 {time_relative} {lon:.12g} {lat:.12g} 0 {float(level_from_top):.12g} "
                f"{time_index + 1} {level_from_top} 0 0 {point_id}"
            )
            supported[point_id] = {
                "time_index": time_index,
                "level_index": level_from_top - 1,
                "levels_hpa": hybrid,  # reused only for length in vertical_selector
                "native_level_from_top": level_from_top,
                "scope": scope,
            }
            row_entries.append((0, time_index, time_value, point_id, row))
            continue

        left, right = bracket_time(point_id, time_value)
        coordinate = record["vertical_coordinate"]
        if coordinate not in vertical_codes:
            raise SystemExit(f"{point_id}: unsupported coordinate {coordinate}")
        vertical_value = float(record["vertical"])
        kind = scope_kind[scope]
        row = (
            f"{kind} {time_relative} {lon:.12g} {lat:.12g} "
            f"{vertical_codes[coordinate]} {vertical_value:.12g} 0 0 "
            f"{left + 1} {right + 1} {point_id}"
        )
        supported[point_id] = {
            "left": left,
            "right": right,
            "scope": scope,
            "modern_kind": record.get("modern_kind"),
        }
        sort_scope = {
            "interpolated_common": 1,
            "surface_layer": 2,
            "modern_difference": 3,
        }[scope]
        row_entries.append((sort_scope, left, time_value, point_id, row))

    counts = {
        scope: sum(r["sample_scope"] == scope for r in query_doc["records"])
        for scope in scope_kind
    }
    rows = [entry[-1] for entry in sorted(row_entries)]
    if (
        counts["native_anchor"] != 27
        or counts["interpolated_common"] != 18
        or counts["surface_layer"] != 12
        or counts["modern_difference"] != 18
        or len(rows) != 75
    ):
        raise SystemExit(f"frozen coverage drift: counts={counts} rows={len(rows)}")
    return rows, supported, {"times": times, "counts": counts}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hybrid", type=Path, default=DEFAULT_HYBRID)
    parser.add_argument("--surface", type=Path, default=DEFAULT_SURFACE)
    parser.add_argument("--queries", type=Path, default=DEFAULT_QUERIES)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()

    for path, label in (
        (args.hybrid, "hybrid input"),
        (args.surface, "surface input"),
        (args.queries, "formal queries"),
        (args.binary, "eta oracle driver binary"),
        (SCHEMA, "oracle schema"),
        (MANIFEST, "real-met manifest"),
        (PROFILE, "ERA5 hybrid profile"),
    ):
        require_file(path, label)
    if git_head(FLEXPART_ROOT) != FLEXPART_COMMIT:
        raise SystemExit("FLEXPART checkout is not at the frozen oracle commit")

    query_doc = json.loads(args.queries.read_text(encoding="utf-8"))
    if query_doc.get("dataset_family") != "era5_hybrid":
        raise SystemExit("query document is not era5_hybrid")
    if query_doc.get("flexpart_commit") != FLEXPART_COMMIT:
        raise SystemExit("query document FLEXPART commit drift")
    if query_doc.get("status") != "frozen_terrain_selected":
        raise SystemExit("only formal terrain-selected queries are accepted")

    rows, supported, _ = build_hybrid_driver_queries(query_doc, args.hybrid)
    work = ROOT / "target/m3-oracle/work/era5-hybrid"
    driver_queries = work / "driver_queries.txt"
    raw_output = work / "driver_output.tsv"
    driver_log = work / "driver.log"
    write_driver_query_file(driver_queries, rows)
    driver_ok = run_driver(
        args.binary,
        args.hybrid,
        args.surface,
        driver_queries,
        raw_output,
        driver_log,
        source_family="era5",
        vertical_coordinate="hybrid_eta",
        pbl_height_mode="richardson_diagnosed",
    )
    if driver_ok:
        driver_values = parse_driver_output(raw_output)
        if set(driver_values) != set(supported):
            missing = sorted(set(supported) - set(driver_values))
            extra = sorted(set(driver_values) - set(supported))
            raise SystemExit(
                f"driver coverage mismatch missing={missing[:3]} extra={extra[:3]}"
            )
        # Patch vertical selectors for native hybrid levels.
        records = build_records(query_doc, driver_values, supported)
        for record, query in zip(records, query_doc["records"]):
            if query["sample_scope"] != "native_anchor":
                continue
            level_from_top = int(query["vertical"])
            record["vertical_selector"] = {
                "kind": "native_full_level",
                "level_index_from_top": level_from_top - 1,
                "resolved_coordinate": "pressure_pa",
                "resolved_value": float(
                    (driver_values[query["point_id"]].get("air_pressure") or 0.0)
                ),
            }
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
        args.hybrid,
        args.surface,
        args.queries,
        args.binary,
        query_doc,
        records,
        source_family="era5",
        dataset_family="era5_hybrid",
        required_symbols=[
            "verttransform_ecmwf",
            "interpol_wind",
            "interpol_partoutput_val",
            "interpol_pbl",
            "oracle_calcpar",
        ],
        vertical_transform="verttransform_ecmwf",
        build_mode="eta_hybrid",
        pbl_height_mode="richardson_diagnosed",
        vertical_coordinate="hybrid_eta",
    )
    document["input"]["dataset_family"] = "era5_hybrid"
    document["input"]["profile_sha256"] = sha256_file(PROFILE)
    document["oracle"]["build_mode"] = "eta"
    document["oracle"]["loader_kind"] = "oracle_adapter_netcdf"
    document["oracle"]["loader_sha256"] = sha256_file(
        ROOT / "tools/flexpart_oracle/src/eta_hybrid_oracle_driver.f90"
    )
    # build_records provenance defaults to pressure/gfs; rewrite to ETA path.
    for record in document["records"]:
        fields = record.get("fields") or {}
        for field in fields.values():
            src = str(field.get("source", ""))
            if "verttransform_gfs" in src:
                field["source"] = src.replace("verttransform_gfs", "verttransform_ecmwf")
    base_hv = document["oracle"]["harness_version"]
    tokens = base_hv.split(";", 1)
    identity_suffix = (";" + tokens[1]) if len(tokens) > 1 else (
        ";adapter=eta_hybrid;source_family=era5;vertical_coordinate=hybrid_eta"
        ";pbl_height_mode=richardson_diagnosed;vertical_transform=verttransform_ecmwf"
    )
    document["oracle"]["harness_version"] = HARNESS_VERSION + "+eta-hybrid137" + identity_suffix
    document["oracle"]["harness_sha256"] = sha256_sources(
        [
            ROOT / "tools/flexpart_oracle/src/eta_hybrid_oracle_driver.f90",
            ROOT / "tools/flexpart_oracle/src/oracle_calcpar_mod.f90",
            ROOT / "tools/flexpart_oracle/build_eta_hybrid_driver.sh",
            Path(__file__).resolve(),
        ]
    )
    compile_flags, preprocessor_definitions, compile_extras = load_compile_identity(
        build_dir_from_binary(args.binary),
        required_symbols=[
            "verttransform_ecmwf",
            "interpol_wind",
            "interpol_partoutput_val",
            "interpol_pbl",
            "oracle_calcpar",
        ],
        vertical_transform="verttransform_ecmwf",
    )
    document["oracle"]["compile_flags"] = compile_flags
    document["oracle"]["preprocessor_definitions"] = preprocessor_definitions
    if document["status"] == "complete" and compile_extras.get("richardson_fail_count", "0") != "0":
        raise SystemExit(
            "refusing complete hybrid oracle while richardson_fail_count != 0: "
            f"{compile_extras.get('richardson_fail_count')}"
        )
    if not driver_ok:
        document["status"] = "failed"
        document["failures"] = [
            {
                "code": "oracle.flexpart_path_failed",
                "message": f"ETA driver failed; see {driver_log.as_posix()}; no silent richardson fallback",
            }
        ]
    # nm evidence lives next to the actual --binary build dir.
    nm_path = build_dir_from_binary(args.binary) / "nm_symbols.txt"

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
                "nm_symbols": str(nm_path) if nm_path.is_file() else None,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
