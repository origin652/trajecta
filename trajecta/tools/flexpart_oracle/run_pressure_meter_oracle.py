#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run the real FLEXPART pressure-meter oracle against official ERA5 data.

Produces the full frozen 75-record matrix for a pressure-level family:
27 native anchors, 18 interpolated common, 12 surface-layer (PBL path),
and 18 modern-difference records. Status is ``complete`` only when every
record is scientifically resolved (``ok`` or explicit vertical
``not_available`` is not expected for the frozen matrix).
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import platform
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import netCDF4  # type: ignore
import numpy as np
from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[2]
FLEXPART_ROOT = ROOT.parent / "flexpart"
FLEXPART_COMMIT = "dace3affa2ba71677f12f3858b04aaf59f8ee51e"
SCHEMA = ROOT / "testdata/M3_FLEXPART_ORACLE.schema.json"
MANIFEST = ROOT / "testdata/REAL_MET_MANIFEST.json"
PROFILE = ROOT / "crates/trajecta-met/profiles/era5_cf_pressure_netcdf_v0.yaml"
DEFAULT_PRESSURE = (
    ROOT
    / "target/test-data/era5-cds-pressure-official/ready/era5_pressure_20181201.nc"
)
DEFAULT_SURFACE = (
    ROOT
    / "target/test-data/era5-cds-pressure-official/ready/era5_surface_20181201.nc"
)
DEFAULT_QUERIES = ROOT / "target/m3-oracle/queries/era5_pressure_queries.json"
DEFAULT_BINARY = (
    ROOT
    / "target/m3-oracle/build/pressure-meter/bin/pressure_meter_oracle_driver"
)
DEFAULT_OUTPUT = ROOT / "target/m3-oracle/artifacts/era5_pressure_oracle.json"
HARNESS_VERSION = "trajecta-flexpart-oracle/0.3.0-pressure-meter-full-matrix"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_sources(paths: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in sorted(paths, key=lambda item: item.as_posix()):
        rel = path.resolve().relative_to(ROOT.resolve()).as_posix().encode("utf-8")
        digest.update(len(rel).to_bytes(4, "big"))
        digest.update(rel)
        data = path.read_bytes()
        digest.update(len(data).to_bytes(8, "big"))
        digest.update(data)
    return digest.hexdigest()


def require_file(path: Path, label: str) -> None:
    if not path.is_file() or path.stat().st_size <= 0:
        raise SystemExit(f"missing {label}: {path}")


def git_head(path: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(path), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def exact_index(values: np.ndarray, wanted: float, label: str, atol: float) -> int:
    matches = np.flatnonzero(np.isclose(values, wanted, rtol=0.0, atol=atol))
    if len(matches) != 1:
        raise SystemExit(f"{label}={wanted} has {len(matches)} matches")
    return int(matches[0])


def build_driver_queries(
    query_doc: dict[str, Any], pressure_path: Path
) -> tuple[list[str], dict[str, dict[str, Any]], dict[str, Any]]:
    with netCDF4.Dataset(pressure_path.as_posix()) as dataset:
        times = np.asarray(dataset.variables["valid_time"][:], dtype=np.int64)
        levels_hpa = np.asarray(dataset.variables["pressure_level"][:], dtype=float)
        latitudes = np.asarray(dataset.variables["latitude"][:], dtype=float)
        longitudes = np.asarray(dataset.variables["longitude"][:], dtype=float)

    if len(times) < 2 or not np.all(np.diff(times) > 0):
        raise SystemExit("pressure valid_time must be strictly increasing")
    if not np.all(np.diff(levels_hpa) < 0):
        raise SystemExit("pressure levels must run surface-to-top")

    row_entries: list[tuple[int, int, int, str, str]] = []
    supported: dict[str, dict[str, Any]] = {}
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
        if any(char.isspace() for char in point_id):
            raise SystemExit(f"point_id contains whitespace: {point_id!r}")
        if scope not in scope_kind:
            raise SystemExit(f"{point_id}: unsupported sample_scope {scope}")
        time_value = int(record["time_unix"])
        time_relative = time_value - base_time
        lon = float(record["longitude_degrees"])
        lat = float(record["latitude_degrees"])

        if scope == "native_anchor":
            time_index = exact_index(times, time_value, "valid_time", 0.0)
            level_pa = float(record["vertical"])
            level_index = exact_index(
                levels_hpa, level_pa / 100.0, "pressure_level_hpa", 1.0e-7
            )
            exact_index(longitudes, lon, "longitude", 1.0e-7)
            exact_index(latitudes, lat, "latitude", 1.0e-7)
            row = (
                f"1 {time_relative} {lon:.12g} {lat:.12g} 0 {level_pa:.12g} "
                f"{time_index + 1} {level_index + 1} 0 0 {point_id}"
            )
            supported[point_id] = {
                "time_index": time_index,
                "level_index": level_index,
                "levels_hpa": levels_hpa,
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
        # Sort: native(0), common(1), surface(2), modern(3)
        sort_scope = {"interpolated_common": 1, "surface_layer": 2, "modern_difference": 3}[
            scope
        ]
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
    metadata = {
        "times": times,
        "levels_hpa": levels_hpa,
        "counts": counts,
    }
    return rows, supported, metadata


def write_driver_query_file(path: Path, rows: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = f"{len(rows)}\n" + "\n".join(rows) + "\n"
    path.write_text(data, encoding="utf-8", newline="\n")


def run_driver(
    binary: Path,
    pressure_path: Path,
    surface_path: Path,
    query_path: Path,
    raw_output: Path,
    log_path: Path,
    *,
    source_family: str,
    vertical_coordinate: str = "pressure",
    pbl_height_mode: str = "official_prescribed",
) -> bool:
    """Run GPL oracle driver with explicit 3-axis identity. Returns True on success."""
    allowed = {
        ("era5", "pressure", "official_prescribed"),
        ("cfsr", "pressure", "official_prescribed"),
        ("era5", "hybrid_eta", "richardson_diagnosed"),
    }
    key = (source_family, vertical_coordinate, pbl_height_mode)
    if key not in allowed:
        raise SystemExit(f"unsupported oracle identity combination: {key}")
    raw_output.parent.mkdir(parents=True, exist_ok=True)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.pop("LD_LIBRARY_PATH", None)
    env["TRAJECTA_ORACLE_SOURCE_FAMILY"] = source_family
    env["TRAJECTA_ORACLE_VERTICAL_COORDINATE"] = vertical_coordinate
    env["TRAJECTA_ORACLE_PBL_HEIGHT_MODE"] = pbl_height_mode
    env["TRAJECTA_ORACLE_RICHARDSON_TOKEN_PATH"] = str(
        log_path.with_name(log_path.stem + ".richardson_fail_count.txt")
    )
    result = subprocess.run(
        [
            str(binary),
            str(pressure_path),
            str(surface_path),
            str(query_path),
            str(raw_output),
        ],
        capture_output=True,
        text=True,
        env=env,
    )
    log_path.write_text(
        f"exit_code={result.returncode}\n"
        f"TRAJECTA_ORACLE_SOURCE_FAMILY={source_family}\n"
        f"TRAJECTA_ORACLE_VERTICAL_COORDINATE={vertical_coordinate}\n"
        f"TRAJECTA_ORACLE_PBL_HEIGHT_MODE={pbl_height_mode}\n"
        f"[stdout]\n{result.stdout}\n[stderr]\n{result.stderr}",
        encoding="utf-8",
        newline="\n",
    )
    build_dir = binary.resolve().parent.parent
    # Per-run token beside the log so multi-family sharing one binary cannot clobber.
    fail_count_path = log_path.with_name(log_path.stem + ".richardson_fail_count.txt")
    os.environ["TRAJECTA_ORACLE_RICHARDSON_TOKEN_PATH"] = str(fail_count_path)
    if result.returncode != 0:
        # No silent scientific fallback: failed FLEXPART path stays failed.
        token = (
            "hard_fail"
            if "richardson failed" in (result.stderr + result.stdout)
            else "driver_fail"
        )
        fail_count_path.write_text(token + "\n", encoding="utf-8", newline="\n")
        return False
    # Successful exit ⇒ no Richardson fallback and no hard fail for this run.
    fail_count_path.write_text("0\n", encoding="utf-8", newline="\n")
    return True


def parse_driver_output(path: Path) -> dict[str, dict[str, Any]]:
    values: dict[str, dict[str, Any]] = {}
    with path.open("r", encoding="utf-8", newline="") as handle:
        for row in csv.DictReader(handle, delimiter="\t"):
            point_id = row["point_id"]
            if point_id in values:
                raise SystemExit(f"duplicate driver output point_id {point_id}")
            if row["status"] == "ok":
                for key in (
                    "eastward_wind",
                    "northward_wind",
                    "air_temperature",
                    "specific_humidity",
                    "air_pressure",
                    "geopotential_height",
                    "legacy_w",
                    "air_density",
                    "geometric_terrain_height",
                    "geometric_vertical_velocity",
                ):
                    if key not in row or row[key] in (None, ""):
                        raise SystemExit(f"{point_id}: driver missing field {key}")
                    row[key] = float(row[key])
            values[point_id] = row
    return values


def sample(value: float, unit: str, semantic_class: str, source: str) -> dict[str, Any]:
    if not np.isfinite(value):
        raise SystemExit(f"non-finite oracle sample from {source}")
    return {
        "valid": True,
        "value": value,
        "unit": unit,
        "precision": "binary32",
        "semantic_class": semantic_class,
        "source": source,
    }


def vertical_selector(
    record: dict[str, Any], supported_meta: dict[str, Any] | None
) -> dict[str, Any]:
    if record["sample_scope"] == "native_anchor":
        if supported_meta is None:
            raise SystemExit(f"native record lacks support metadata: {record['point_id']}")
        levels_hpa = supported_meta["levels_hpa"]
        level_index = int(supported_meta["level_index"])
        return {
            "kind": "native_full_level",
            "level_index_from_top": len(levels_hpa) - 1 - level_index,
            "resolved_coordinate": "pressure_pa",
            "resolved_value": float(levels_hpa[level_index] * 100.0),
        }
    coordinate = {
        "above_ground": "agl",
        "above_sea_level": "asl",
        "pressure": "pressure_pa",
    }[record["vertical_coordinate"]]
    return {
        "kind": "physical",
        "coordinate": coordinate,
        "value": float(record["vertical"]),
    }


def build_records(
    query_doc: dict[str, Any],
    driver_values: dict[str, dict[str, Any]],
    supported: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for query in query_doc["records"]:
        point_id = query["point_id"]
        raw = driver_values.get(point_id)
        support_meta = supported.get(point_id)
        record: dict[str, Any] = {
            "point_id": point_id,
            "sample_scope": query["sample_scope"],
            "time_unix_seconds": int(query["time_unix"]),
            "time_nanosecond": 0,
            "longitude_degrees": float(query["longitude_degrees"]),
            "latitude_degrees": float(query["latitude_degrees"]),
            "vertical_selector": vertical_selector(query, support_meta),
            "fields": {},
        }
        if raw is None:
            record["status"] = "not_available"
            record["diagnostic"] = {
                "code": "oracle.driver_missing_point",
                "message": "FLEXPART driver did not emit this frozen query point_id",
            }
            records.append(record)
            continue
        if raw["status"] != "ok":
            record["status"] = "not_available"
            record["diagnostic"] = {
                "code": "oracle.vertical_not_available",
                "message": "FLEXPART reported the requested vertical point outside its transformed column",
            }
            records.append(record)
            continue

        scope = query["sample_scope"]
        native = scope == "native_anchor"
        if native:
            semantic = "flexpart_native"
            source = "oracle_adapter_netcdf:raw_native_binary32"
        elif scope in ("surface_layer", "modern_difference"):
            # Schema enum: surface/modern FLEXPART quantities feed difference reports.
            semantic = "modern_difference_reference"
            if scope == "surface_layer":
                source = (
                    "flexpart:verttransform_gfs+interpol_pbl+interpol_pbl_short+"
                    "interpol_wind+interpol_partoutput_val"
                )
            else:
                source = (
                    "flexpart:verttransform_gfs+interpol_wind+interpol_partoutput_val"
                )
        else:
            semantic = "common_semantics"
            source = "flexpart:verttransform_gfs+interpol_wind+interpol_partoutput_val"

        record["status"] = "ok"
        if scope == "modern_difference":
            kind = query.get("modern_kind")
            if kind == "density":
                record["fields"] = {
                    "air_density": sample(
                        raw["air_density"], "kg m-3", semantic, source
                    )
                }
            elif kind == "geometric_w":
                # FLEXPART meter-mode W is the legacy vertical velocity quantity;
                # published under the difference-report field name for MIT pairing.
                record["fields"] = {
                    "geometric_vertical_velocity": sample(
                        raw["geometric_vertical_velocity"],
                        "m s-1",
                        semantic,
                        source + "+legacy_w",
                    )
                }
            elif kind == "geometric_terrain":
                record["fields"] = {
                    "geometric_terrain_height": sample(
                        raw["geometric_terrain_height"],
                        "m",
                        semantic,
                        source + "+orography_geometric",
                    )
                }
            else:
                raise SystemExit(f"{point_id}: unknown modern_kind {kind!r}")
        elif scope == "surface_layer":
            record["fields"] = {
                "eastward_wind": sample(raw["eastward_wind"], "m s-1", semantic, source),
                "northward_wind": sample(
                    raw["northward_wind"], "m s-1", semantic, source
                ),
                "air_temperature": sample(
                    raw["air_temperature"], "K", semantic, source
                ),
                "specific_humidity": sample(
                    raw["specific_humidity"], "kg kg-1", semantic, source
                ),
                "air_pressure": sample(raw["air_pressure"], "Pa", semantic, source),
                "geometric_vertical_velocity": sample(
                    raw["geometric_vertical_velocity"], "m s-1", semantic, source
                ),
                "air_density": sample(raw["air_density"], "kg m-3", semantic, source),
                "geometric_terrain_height": sample(
                    raw["geometric_terrain_height"], "m", semantic, source
                ),
            }
        else:
            record["fields"] = {
                "eastward_wind": sample(raw["eastward_wind"], "m s-1", semantic, source),
                "northward_wind": sample(
                    raw["northward_wind"], "m s-1", semantic, source
                ),
                "air_temperature": sample(
                    raw["air_temperature"], "K", semantic, source
                ),
                "specific_humidity": sample(
                    raw["specific_humidity"], "kg kg-1", semantic, source
                ),
                "air_pressure": sample(raw["air_pressure"], "Pa", semantic, source),
                "geopotential_height": sample(
                    raw["geopotential_height"], "m", semantic, source
                ),
            }
        records.append(record)
    return records


def input_file(role: str, path: Path) -> dict[str, Any]:
    return {
        "role": role,
        "name": path.name,
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
    }



def build_dir_from_binary(binary: Path) -> Path:
    """Resolve the oracle build root that produced --binary (…/build/<name>/bin/<exe>)."""
    resolved = binary.resolve()
    # …/build/<name>/bin/<exe> → …/build/<name>
    if resolved.parent.name == "bin":
        return resolved.parent.parent
    return resolved.parent


def load_compile_identity(
    build_dir: Path,
    *,
    required_symbols: list[str] | None = None,
    vertical_transform: str | None = None,
) -> tuple[list[str], list[str], dict[str, str]]:
    """Read compile identity from the actual --binary build directory.

    compile_flags.txt, compile_identity.txt, and nm_symbols.txt are mandatory.
    """
    flags_path = build_dir / "compile_flags.txt"
    identity_path = build_dir / "compile_identity.txt"
    nm_path = build_dir / "nm_symbols.txt"
    missing = [path.name for path in (flags_path, identity_path, nm_path) if not path.is_file()]
    if missing:
        raise SystemExit(f"compile identity evidence missing in {build_dir}: {missing}")

    flags = [
        line.strip()
        for line in flags_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not flags:
        raise SystemExit(f"empty compile_flags.txt: {flags_path}")

    text = identity_path.read_text(encoding="utf-8")
    nm_text = nm_path.read_text(encoding="utf-8", errors="replace")
    required_syms = required_symbols or [
        "verttransform_gfs",
        "interpol_wind",
        "interpol_partoutput_val",
        "interpol_pbl",
        "oracle_calcpar",
    ]
    missing_syms = [sym for sym in required_syms if sym not in nm_text]
    if missing_syms:
        raise SystemExit(f"nm_symbols missing required symbols in {build_dir}: {missing_syms}")

    extras: dict[str, str] = {
        "build_dir": str(build_dir),
        "compile_flags_path": str(flags_path),
        "compile_identity_path": str(identity_path),
        "nm_symbols_path": str(nm_path),
        "compile_identity_text": text.strip(),
        "nm_required_symbols": ",".join(required_syms),
        "vertical_transform": vertical_transform or "verttransform_gfs",
    }
    token_env = os.environ.get("TRAJECTA_ORACLE_RICHARDSON_TOKEN_PATH")
    fail_count_path = Path(token_env) if token_env else (build_dir / "richardson_fail_count.txt")
    if fail_count_path.is_file():
        extras["richardson_fail_count"] = fail_count_path.read_text(encoding="utf-8").strip()
    extras["richardson_fail_count_path"] = str(fail_count_path)

    defs: list[str] = []
    if "USE_NCF=undef" in text:
        defs.append("-UUSE_NCF")
    if "ETA=define" in text:
        defs.append("-DETA")
    elif "ETA=undef" in text:
        defs.append("-UETA")
    if "useomp=define" in text:
        defs.append("-Duseomp")
    for token in flags:
        if token.startswith(("-D", "-U", "-g")) and token not in defs:
            defs.append(token)
    return flags, defs, extras

def compiler_version() -> str:
    result = subprocess.run(
        ["gfortran", "--version"], check=True, capture_output=True, text=True
    )
    return result.stdout.splitlines()[0].strip()



def write_identity_sidecar(oracle_path: Path, document: dict[str, Any]) -> Path:
    """Schema-frozen oracle JSON cannot hold extra identity keys; emit sidecar."""
    hv = document["oracle"]["harness_version"]
    parts = {}
    for token in hv.split(";"):
        if "=" in token:
            k, v = token.split("=", 1)
            parts[k] = v
    payload = {
        "oracle_path": oracle_path.as_posix(),
        "dataset_family": document["input"]["dataset_family"],
        "build_mode_schema": document["oracle"]["build_mode"],
        "adapter": parts.get("adapter"),
        "source_family": parts.get("source_family"),
        "vertical_coordinate": parts.get("vertical_coordinate"),
        "pbl_height_mode": parts.get("pbl_height_mode"),
        "vertical_transform": parts.get("vertical_transform"),
        "comparison_variant_hint": (
            "trajecta-vs-flexpart-v11.1"
            if parts.get("vertical_transform") == "verttransform_ecmwf"
            else "trajecta-vs-flexpart-pressure-adapter-v1"
        ),
        "harness_version": hv,
        "binary_sha256": document["oracle"]["binary_sha256"],
        "harness_sha256": document["oracle"]["harness_sha256"],
        "query_sha256": document["input"]["query_sha256"],
    }
    side = oracle_path.with_name(oracle_path.stem + "_identity.json")
    side.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + chr(10),
        encoding="utf-8",
        newline=chr(10),
    )
    return side


def build_document(
    pressure_path: Path,
    surface_path: Path,
    query_path: Path,
    binary: Path,
    query_doc: dict[str, Any],
    records: list[dict[str, Any]],
    *,
    source_family: str,
    dataset_family: str,
    required_symbols: list[str] | None = None,
    vertical_transform: str | None = None,
    build_mode: str | None = None,
    pbl_height_mode: str | None = None,
    vertical_coordinate: str | None = None,
) -> dict[str, Any]:
    driver = ROOT / "tools/flexpart_oracle/src/pressure_meter_oracle_driver.f90"
    build_script = ROOT / "tools/flexpart_oracle/build_pressure_meter_driver.sh"
    runner = Path(__file__).resolve()
    libc_name, libc_version = platform.libc_ver()
    ok_count = sum(record["status"] == "ok" for record in records)
    build_dir = build_dir_from_binary(binary)
    # If the latest driver log token exists next to standard work logs, prefer it.
    # Callers may also export TRAJECTA_ORACLE_RICHARDSON_TOKEN_PATH.
    vt = vertical_transform or "verttransform_gfs"
    compile_flags, preprocessor_definitions, compile_extras = load_compile_identity(
        build_dir,
        required_symbols=required_symbols,
        vertical_transform=vt,
    )
    complete = ok_count == len(records) and len(records) == 75
    richardson_token = compile_extras.get("richardson_fail_count", "missing")
    if complete and richardson_token != "0":
        raise SystemExit(
            "refusing complete oracle while richardson_fail_count != 0: "
            f"{richardson_token}"
        )
    # Driver hard-fail (e.g. frozen FLEXPART richardson path) ⇒ failed, not complete.
    status = "complete" if complete else ("failed" if not records or ok_count == 0 else "partial")
    return {
        "schema_version": "trajecta.m3.flexpart_oracle/v1",
        "status": status,
        "generated_at_utc": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "oracle": {
            "flexpart_repository": "https://github.com/flexpart/flexpart",
            "flexpart_commit": FLEXPART_COMMIT,
            "compiler": "gfortran",
            "compiler_version": compiler_version(),
            "compile_flags": compile_flags,
            "preprocessor_definitions": preprocessor_definitions,
            "binary_sha256": sha256_file(binary),
            "default_real_bits": 32,
            "build_mode": (
                "eta" if (build_mode or "").startswith("eta") or vt == "verttransform_ecmwf"
                else "pressure_meter"
            ),
            "loader_kind": "oracle_adapter_netcdf",
            "loader_sha256": sha256_file(driver),
            "harness_version": (
                HARNESS_VERSION
                + ";adapter="
                + (
                    "eta_hybrid"
                    if vt == "verttransform_ecmwf"
                    else "pressure_meter_adapter"
                )
                + f";source_family={source_family}"
                + ";vertical_coordinate="
                + (
                    vertical_coordinate
                    or ("hybrid_eta" if vt == "verttransform_ecmwf" else "pressure")
                )
                + ";pbl_height_mode="
                + (
                    pbl_height_mode
                    or (
                        "richardson_diagnosed"
                        if vt == "verttransform_ecmwf"
                        else "official_prescribed"
                    )
                )
                + f";vertical_transform={vt}"
            ),
            "harness_sha256": sha256_sources([driver, build_script, runner, ROOT / "tools/flexpart_oracle/src/oracle_calcpar_mod.f90"]),
            "host": {
                "os": platform.system(),
                "architecture": platform.machine(),
                **(
                    {"libc": f"{libc_name} {libc_version}"}
                    if libc_name and libc_version
                    else {}
                ),
            },
        },
        "input": {
            "dataset_family": dataset_family,
            "dataset_manifest_sha256": sha256_file(MANIFEST),
            "profile_sha256": sha256_file(PROFILE),
            "query_sha256": sha256_file(query_path),
            "files": [
                input_file("pressure", pressure_path),
                input_file("surface", surface_path),
            ],
        },
        "records": records,
        "failures": []
        if complete
        else [
            {
                "code": "oracle.coverage_incomplete"
                if status != "failed"
                else "oracle.flexpart_path_failed",
                "message": (
                    f"real FLEXPART pressure-meter path produced {ok_count}/"
                    f"{len(query_doc['records'])} ok records; "
                    f"richardson_fail_count={richardson_token}"
                ),
            }
        ],
    }


def validate(doc: dict[str, Any]) -> None:
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    errors = sorted(Draft202012Validator(schema).iter_errors(doc), key=lambda e: list(e.path))
    if errors:
        for error in errors[:20]:
            print(f"schema {list(error.path)}: {error.message}", file=sys.stderr)
        raise SystemExit(2)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pressure", type=Path, default=DEFAULT_PRESSURE)
    parser.add_argument("--surface", type=Path, default=DEFAULT_SURFACE)
    parser.add_argument("--queries", type=Path, default=DEFAULT_QUERIES)
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()

    for path, label in (
        (args.pressure, "pressure input"),
        (args.surface, "surface input"),
        (args.queries, "formal queries"),
        (args.binary, "oracle driver binary"),
        (SCHEMA, "oracle schema"),
        (MANIFEST, "real-met manifest"),
        (PROFILE, "ERA5 pressure profile"),
    ):
        require_file(path, label)
    if git_head(FLEXPART_ROOT) != FLEXPART_COMMIT:
        raise SystemExit("FLEXPART checkout is not at the frozen oracle commit")

    query_doc = json.loads(args.queries.read_text(encoding="utf-8"))
    if query_doc.get("dataset_family") != "era5_pressure":
        raise SystemExit("query document is not era5_pressure")
    if query_doc.get("flexpart_commit") != FLEXPART_COMMIT:
        raise SystemExit("query document FLEXPART commit drift")
    if query_doc.get("status") != "frozen_terrain_selected":
        raise SystemExit("only formal terrain-selected queries are accepted")

    rows, supported, _metadata = build_driver_queries(query_doc, args.pressure)
    work = ROOT / "target/m3-oracle/work/era5-pressure"
    driver_queries = work / "driver_queries.txt"
    raw_output = work / "driver_output.tsv"
    driver_log = work / "driver.log"
    write_driver_query_file(driver_queries, rows)
    driver_ok = run_driver(
        args.binary,
        args.pressure,
        args.surface,
        driver_queries,
        raw_output,
        driver_log,
        source_family="era5",
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
        # Frozen FLEXPART path aborted (e.g. richardson). Emit schema-valid failed oracle.
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
        args.pressure,
        args.surface,
        args.queries,
        args.binary,
        query_doc,
        records,
        source_family="era5",
        dataset_family="era5_pressure",
    )
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
