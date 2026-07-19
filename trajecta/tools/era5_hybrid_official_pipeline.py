#!/usr/bin/env python3
"""Formal empty-cache hybrid-137 pipeline: CDS raw → derived coeffs → ready.

Layout under target/test-data/era5-cds-hybrid137-official/:
  raw/       # service bytes only (hybrid 3D, lnsp, surface base/flux, optional PV grib)
  derived/   # A/B coefficients JSON (NOT raw)
  ready/     # Trajecta-normalized prepared NetCDF
  FETCH_MANIFEST.json / PREPARE_MANIFEST.json

Usage:
  python tools/era5_hybrid_official_pipeline.py
  python tools/era5_hybrid_official_pipeline.py --force-fetch
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import cdsapi

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/test-data/era5-cds-hybrid137-official"
# A-frozen expanded box: N, W, S, E
AREA = [53, 0, 45, 10]
DATE = "2018-12-01"
TIMES = ["00:00", "03:00", "06:00"]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def has_trajecta_stamp(path: Path) -> bool:
    if path.suffix.lower() != ".nc":
        return False
    try:
        from netCDF4 import Dataset

        with Dataset(path) as ds:
            note = str(getattr(ds, "anchor_note", "") or "")
            fam = str(getattr(ds, "dataset_family", "") or "")
            role = str(getattr(ds, "role", "") or "")
            return bool(fam or role or "Trajecta" in note or "Ready" in note or "prepared" in note.lower())
    except Exception:
        return False




def load_expected_from_manifest(root: Path, name: str):
    """Return (size, sha256) from FETCH_MANIFEST if present for relative raw/<name>."""
    manifest = root / "FETCH_MANIFEST.json"
    if not manifest.is_file():
        return None, None
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except Exception as exc:
        raise SystemExit(f"FETCH_MANIFEST.json unreadable at {manifest}: {exc}") from exc
    if not isinstance(data, dict) or "files" not in data:
        raise SystemExit(f"FETCH_MANIFEST.json missing files[] at {manifest}")
    for rec in data.get("files", []):
        rel = rec.get("relative_path") or rec.get("name") or ""
        if rel.endswith(name) or rec.get("name") == name:
            size = rec.get("size")
            sha = rec.get("sha256")
            if size is None or not sha:
                raise SystemExit(
                    f"FETCH_MANIFEST entry for {name} missing size/sha256: {rec}"
                )
            return int(size), str(sha)
    return None, None


def _openable_netcdf(path: Path) -> bool:
    try:
        from netCDF4 import Dataset

        with Dataset(path) as ds:
            return len(ds.variables) > 0
    except Exception as e:
        print("netcdf open failed", path, e, flush=True)
        return False



def _looks_like_grib(path: Path) -> bool:
    """Basic GRIB container check without external decoder dependency.

    Accepts GRIB1/GRIB2 single- or multi-message files whose first non-padding
    magic is b'GRIB'. Rejects empty/text/NetCDF mislabeled downloads.
    """
    try:
        with path.open("rb") as handle:
            head = handle.read(16)
    except OSError as exc:
        print("grib open failed", path, exc, flush=True)
        return False
    if len(head) < 4:
        return False
    if head.startswith(b"GRIB"):
        return True
    # Some downloads prepend a short HTTP/text wrapper; scan first 4 KiB for magic.
    try:
        with path.open("rb") as handle:
            blob = handle.read(4096)
    except OSError:
        return False
    if b"GRIB" in blob:
        # Require GRIB at a 4-byte aligned offset (edition-1/2 message start).
        idx = 0
        while True:
            pos = blob.find(b"GRIB", idx)
            if pos < 0:
                break
            if pos % 4 == 0:
                return True
            idx = pos + 1
    print("grib magic missing in", path.name, flush=True)
    return False


def file_usable(path: Path, *, expected_size, expected_sha) -> bool:
    """Strong integrity check. expected_* may be None only for fresh .part basic gate."""
    if not path.is_file() or path.stat().st_size <= 0:
        return False
    if has_trajecta_stamp(path):
        return False
    size = path.stat().st_size
    if expected_size is not None and size != int(expected_size):
        print("size mismatch for", path.name, size, "!=", expected_size, flush=True)
        return False
    if expected_sha is not None:
        digest = sha256_file(path)
        if digest != expected_sha:
            print("sha256 mismatch for", path.name, flush=True)
            return False
    name = path.name.lower()
    if name.endswith(".nc") or name.endswith(".nc.part"):
        if not _openable_netcdf(path):
            return False
    # GRIB / GRIB.part: require magic even when FETCH size/sha is absent.
    gribish = (
        name.endswith(".grb")
        or name.endswith(".grb2")
        or name.endswith(".grib")
        or name.endswith(".grib2")
        or name.endswith(".grb.part")
        or name.endswith(".grb2.part")
        or name.endswith(".grib.part")
        or name.endswith(".grib2.part")
        or ".grb." in name
        or ".grb2." in name
        or ".grib." in name
        or ".grib2." in name
    )
    if gribish:
        if not _looks_like_grib(path):
            return False
    return True


def promote_part(part: Path, target: Path, *, exp_size, exp_sha) -> None:
    """Verify .part then replace target. Never promote corrupt bytes."""
    if exp_size is None and exp_sha is None:
        if not file_usable(part, expected_size=None, expected_sha=None):
            if part.is_file():
                part.unlink(missing_ok=True)
            raise SystemExit(f"downloaded .part failed basic integrity: {part}")
        print(
            "WARNING: promoting .part without FETCH_MANIFEST expected size/sha:",
            part.name,
            flush=True,
        )
    else:
        if not file_usable(part, expected_size=exp_size, expected_sha=exp_sha):
            if part.is_file():
                part.unlink(missing_ok=True)
            raise SystemExit(
                f"downloaded .part failed expected size/sha verification: {part}"
            )
    if target.is_file():
        target.unlink()
    part.replace(target)
    if has_trajecta_stamp(target):
        target.unlink(missing_ok=True)
        raise SystemExit(
            f"downloaded file unexpectedly contains Trajecta stamp: {target}"
        )

def retrieve(client: cdsapi.Client, dataset: str, request: dict, target: Path, *, force: bool) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    root = OUT
    exp_size, exp_sha = load_expected_from_manifest(root, target.name)
    if force:
        # Expanded area / content change invalidates prior FETCH expectations.
        exp_size, exp_sha = None, None
    if not force:
        if exp_size is not None and exp_sha is not None:
            if file_usable(target, expected_size=exp_size, expected_sha=exp_sha):
                print("reuse raw (verified size+sha)", target.name, target.stat().st_size, flush=True)
                return
        else:
            if (
                os.environ.get("TRAJECTA_ALLOW_UNVERIFIED_RAW") == "1"
                and file_usable(target, expected_size=None, expected_sha=None)
            ):
                print("reuse raw (UNVERIFIED, env override)", target.name, flush=True)
                return
            if target.is_file():
                print("refuse weak reuse without FETCH size/sha:", target.name, flush=True)
    if target.is_file():
        print("discard unusable/forced", target.name, flush=True)
        target.unlink()
    part = target.with_suffix(target.suffix + ".part")
    if part.is_file():
        part.unlink()
    print("requesting", dataset, "->", target.name, flush=True)
    print("area", request.get("area"), flush=True)
    client.retrieve(dataset, request, str(part))
    promote_part(part, target, exp_size=exp_size, exp_sha=exp_sha)

def fetch_all(*, force: bool) -> None:
    raw = OUT / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    client = cdsapi.Client()
    year, month, day = DATE.split("-")

    # Hybrid 3D on model levels 1-137 (t/u/v/q/w). lnsp is separate file (level 1).
    retrieve(
        client,
        "reanalysis-era5-complete",
        {
            "class": "ea",
            "date": DATE,
            "expver": "1",
            "levelist": "/".join(str(i) for i in range(1, 138)),
            "levtype": "ml",
            "param": "130/131/132/133/135",
            "stream": "oper",
            "time": "/".join(t.replace(":00", "") for t in ["00", "03", "06"]),
            "type": "an",
            "area": "/".join(str(x) for x in AREA),
            "grid": "0.25/0.25",
            "format": "netcdf",
        },
        raw / "era5_hybrid137_20181201.nc",
        force=force,
    )

    # lnsp (param 152) on model level 1
    retrieve(
        client,
        "reanalysis-era5-complete",
        {
            "class": "ea",
            "date": DATE,
            "expver": "1",
            "levelist": "1",
            "levtype": "ml",
            "param": "152",
            "stream": "oper",
            "time": "/".join(t.replace(":00", "") for t in ["00", "03", "06"]),
            "type": "an",
            "area": "/".join(str(x) for x in AREA),
            "grid": "0.25/0.25",
            "format": "netcdf",
        },
        raw / "era5_lnsp_20181201.nc",
        force=force,
    )

    # Surface companions aligned to 00/03/06
    retrieve(
        client,
        "reanalysis-era5-single-levels",
        {
            "product_type": "reanalysis",
            "variable": [
                "geopotential",
                "10m_u_component_of_wind",
                "10m_v_component_of_wind",
                "2m_temperature",
                "2m_dewpoint_temperature",
                "forecast_surface_roughness",
                "boundary_layer_height",
                "friction_velocity",
            ],
            "year": year,
            "month": month,
            "day": day,
            "time": TIMES,
            "area": AREA,
            "data_format": "netcdf",
            "download_format": "unarchived",
        },
        raw / "era5_hybrid_surface_base_20181201.nc",
        force=force,
    )
    retrieve(
        client,
        "reanalysis-era5-single-levels",
        {
            "product_type": "reanalysis",
            "variable": [
                "instantaneous_surface_sensible_heat_flux",
                "instantaneous_moisture_flux",
            ],
            "year": year,
            "month": month,
            "day": day,
            "time": TIMES,
            "area": AREA,
            "data_format": "netcdf",
            "download_format": "unarchived",
        },
        raw / "era5_hybrid_surface_flux_20181201.nc",
        force=force,
    )

    # Tiny GRIB PV probe for official A/B coefficients (service bytes).
    retrieve(
        client,
        "reanalysis-era5-complete",
        {
            "class": "ea",
            "date": DATE,
            "expver": "1",
            "levelist": "137",
            "levtype": "ml",
            "param": "130",
            "stream": "oper",
            "time": "00",
            "type": "an",
            "area": "/".join(str(x) for x in AREA),
            "grid": "0.25/0.25",
            "format": "grib",
        },
        raw / "era5_hybrid_pv_probe.grib",
        force=force,
    )

    files = []
    for path in sorted(raw.iterdir()):
        if path.is_file():
            files.append(
                {
                    "name": path.name,
                    "relative_path": f"raw/{path.name}",
                    "size": path.stat().st_size,
                    "sha256": sha256_file(path),
                }
            )
            print(path.name, path.stat().st_size, files[-1]["sha256"], flush=True)
    (OUT / "FETCH_MANIFEST.json").write_text(
        json.dumps(
            {
                "dataset": "era5_cds_hybrid137_raw",
                "date": DATE,
                "times_utc": ["00", "03", "06"],
                "source": "https://cds.climate.copernicus.eu",
                "files": files,
                "notes": [
                    "raw/ is service download bytes only.",
                    "A/B coefficients live under derived/, not raw/.",
                    "No API keys recorded.",
                ],
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def extract_coefficients() -> Path:
    raw_probe = OUT / "raw" / "era5_hybrid_pv_probe.grib"
    derived = OUT / "derived"
    derived.mkdir(parents=True, exist_ok=True)
    coeff = derived / "era5_l137_ab_coefficients.json"
    if not raw_probe.is_file():
        raise SystemExit(f"missing PV probe {raw_probe}")
    cmd = [
        "cargo",
        "run",
        "--offline",
        "-p",
        "trajecta-met",
        "--example",
        "dump_grib_hybrid_pv",
        "--",
        str(raw_probe),
        str(coeff),
    ]
    print("+", " ".join(cmd), flush=True)
    proc = subprocess.run(cmd, cwd=ROOT)
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)
    return coeff


def prepare() -> None:
    # Point prepare script at correct layout (coeff in derived/).
    cmd = [sys.executable, str(ROOT / "tools/prepare_era5_hybrid_anchors.py"), "--root", str(OUT)]
    print("+", " ".join(cmd), flush=True)
    proc = subprocess.run(cmd, cwd=ROOT)
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--force-fetch", action="store_true")
    parser.add_argument("--skip-fetch", action="store_true")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if not args.skip_fetch:
        fetch_all(force=args.force_fetch)
    extract_coefficients()
    prepare()
    print("pipeline complete:", OUT)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
