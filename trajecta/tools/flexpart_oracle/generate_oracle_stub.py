#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Emit schema-valid v1 oracle documents with status=failed (environment probe).

Never returns 0. Never claims complete. If frozen input files are missing, writes
only STATUS notes and exits 2 without forging oracle JSON (empty files[] forbidden).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target" / "m3-oracle"
SCHEMA = ROOT / "testdata" / "M3_FLEXPART_ORACLE.schema.json"
COMMIT = "dace3affa2ba71677f12f3858b04aaf59f8ee51e"
ZERO = "0" * 64


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def validate(doc: dict) -> list[str]:
    try:
        from jsonschema import Draft202012Validator
    except ImportError:
        print(
            "INCOMPLETE: jsonschema required (Draft 2020-12); no weak fallback",
            file=sys.stderr,
        )
        raise SystemExit(2)
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    v = Draft202012Validator(schema)
    return [f"{list(e.path)}: {e.message}" for e in v.iter_errors(doc)]


def family_inputs(family: str) -> list[dict] | None:
    mapping = {
        "era5_pressure": [
            (
                "pressure",
                ROOT
                / "target/test-data/era5-cds-pressure-official/ready/era5_pressure_20181201.nc",
            ),
            (
                "surface",
                ROOT
                / "target/test-data/era5-cds-pressure-official/ready/era5_surface_20181201.nc",
            ),
        ],
        "era5_hybrid": [
            (
                "hybrid",
                ROOT
                / "target/test-data/era5-cds-hybrid137-official/ready/era5_hybrid137_prepared_20181201.nc",
            ),
            (
                "surface",
                ROOT
                / "target/test-data/era5-cds-hybrid137-official/ready/era5_surface_20181201.nc",
            ),
        ],
        "cfsr_pressure": [
            (
                "pgbl00",
                ROOT / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010100.grb2",
            ),
            (
                "pgbl06",
                ROOT / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010106.grb2",
            ),
            (
                "pgbl12",
                ROOT / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010112.grb2",
            ),
        ],
    }
    items = mapping[family]
    files = []
    for role, path in items:
        if not path.is_file():
            return None
        files.append(
            {
                "role": role,
                "name": path.name,
                "size": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
    return files


def query_sha(family: str) -> str:
    # Prefer formal only; do not silently pick skeleton SHA for formal identity.
    formal = ROOT / "target/m3-oracle/queries" / f"{family}_queries.json"
    if formal.is_file():
        # Reject skeleton residue mistaken for formal.
        try:
            doc = json.loads(formal.read_text(encoding="utf-8"))
            if doc.get("status") == "skeleton_sites_not_terrain_selected":
                return ZERO
        except Exception:
            return ZERO
        return sha256_file(formal)
    return ZERO


def build_failed(family: str, reason: str, files: list[dict]) -> dict:
    host_os = sys.platform
    arch = "unknown"
    try:
        import platform

        arch = platform.machine() or "unknown"
    except Exception:
        pass
    return {
        "schema_version": "trajecta.m3.flexpart_oracle/v1",
        "status": "failed",
        "generated_at_utc": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "oracle": {
            "flexpart_repository": "https://github.com/flexpart/flexpart",
            "flexpart_commit": COMMIT,
            "compiler": "gfortran",
            "compiler_version": "unavailable",
            "compile_flags": [],
            "preprocessor_definitions": [],
            "binary_sha256": ZERO,
            "default_real_bits": 32,
            "build_mode": "eta" if "hybrid" in family else "pressure_meter",
            "loader_kind": "oracle_adapter_netcdf"
            if "era5" in family
            else "oracle_adapter_eccodes",
            "loader_sha256": ZERO,
            "harness_version": "trajecta-flexpart-oracle/0.0.0-probe",
            "harness_sha256": ZERO,
            "host": {
                "os": host_os,
                "architecture": arch,
            },
        },
        "input": {
            "dataset_family": family,
            "dataset_manifest_sha256": ZERO,
            "profile_sha256": ZERO,
            "query_sha256": query_sha(family),
            "files": files,
        },
        "records": [],
        "failures": [
            {
                "code": reason,
                "message": f"environment probe failed: {reason}; refused complete oracle",
                "point_id": "probe:environment",
            }
        ],
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--reason", default="environment_probe_failed")
    args = ap.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "artifacts").mkdir(parents=True, exist_ok=True)

    wrote = 0
    for family in ("era5_pressure", "era5_hybrid", "cfsr_pressure"):
        files = family_inputs(family)
        if files is None:
            note = OUT / "artifacts" / f"{family}_ORACLE_SKIPPED.txt"
            note.write_text(
                f"skipped schema oracle JSON: real input files missing for {family}\n",
                encoding="utf-8",
            )
            print("skip (no inputs)", family)
            continue
        doc = build_failed(family, args.reason, files)
        errs = validate(doc)
        if errs:
            print("schema errors", family, errs[:5], file=sys.stderr)
            return 2
        path = OUT / "artifacts" / f"{family}_oracle.json"
        raw = (json.dumps(doc, indent=2, sort_keys=True) + "\n").encode("utf-8")
        path.write_bytes(raw)
        # re-validate on-disk
        on_disk = json.loads(path.read_text(encoding="utf-8"))
        errs = validate(on_disk)
        if errs:
            print("on-disk schema errors", errs[:5], file=sys.stderr)
            return 2
        print("wrote failed oracle", path, "sha", sha256_bytes(raw)[:16])
        wrote += 1

    if wrote == 0:
        (OUT / "STATUS_PROBE.txt").write_text(
            "no oracle JSON written: missing all family inputs\n", encoding="utf-8"
        )
        return 2
    # Probe always non-zero
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
