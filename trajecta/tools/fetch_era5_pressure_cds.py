#!/usr/bin/env python3
"""Fetch official CDS ERA5 pressure-level raw bytes into raw/ (never stamp).

raw/ holds only service download bytes. prepare_era5_pressure_anchors.py builds ready/.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

# A-frozen expanded box: N, W, S, E (CDS area order)
DEFAULT_AREA_NSWE = [53.0, 0.0, 45.0, 10.0]
PRESSURE_LEVELS = [
    "1", "2", "3", "5", "7", "10", "20", "30", "50", "70",
    "100", "125", "150", "175", "200", "225", "250", "300", "350", "400",
    "450", "500", "550", "600", "650", "700", "750", "775", "800", "825",
    "850", "875", "900", "925", "950", "975", "1000",
]
PRESSURE_VARS = [
    "temperature",
    "u_component_of_wind",
    "v_component_of_wind",
    "specific_humidity",
    "vertical_velocity",
    "geopotential",
]
SURFACE_BASE_VARS = [
    "surface_pressure",
    "geopotential",
    "10m_u_component_of_wind",
    "10m_v_component_of_wind",
    "2m_temperature",
    "2m_dewpoint_temperature",
    "forecast_surface_roughness",
    "boundary_layer_height",
    "friction_velocity",
]
SURFACE_FLUX_VARS = [
    "instantaneous_surface_sensible_heat_flux",
    "instantaneous_moisture_flux",
]
DEFAULT_TIMES = ["00:00", "06:00", "12:00"]


def request_plan(date: str, times: list[str], area: list[float]) -> list[dict]:
    """Return the exact CDS requests without reading credentials or using the network."""
    year, month, day = date.split("-")
    stamp = f"{year}{month}{day}"
    common = {
        "product_type": "reanalysis",
        "year": year,
        "month": month,
        "day": day,
        "time": times,
        "area": area,
        "data_format": "netcdf",
        "download_format": "unarchived",
    }
    return [
        {
            "dataset": "reanalysis-era5-pressure-levels",
            "request": common
            | {"variable": PRESSURE_VARS, "pressure_level": PRESSURE_LEVELS},
            "target": f"raw/era5_pressure_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-single-levels",
            "request": common | {"variable": SURFACE_BASE_VARS},
            "target": f"raw/era5_surface_base_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-single-levels",
            "request": common | {"variable": SURFACE_FLUX_VARS},
            "target": f"raw/era5_surface_flux_{stamp}.nc",
        },
    ]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def has_trajecta_stamp(path: Path) -> bool:
    try:
        from netCDF4 import Dataset

        with Dataset(path) as ds:
            note = str(getattr(ds, "anchor_note", "") or "")
            fam = str(getattr(ds, "dataset_family", "") or "")
            if "Trajecta" in note or "Ready copy" in note or fam:
                # dataset_family alone is our stamp (CDS does not set it).
                if fam or "Trajecta" in note or "Ready" in note or "Official CDS NetCDF4 with Trajecta" in note:
                    return True
    except Exception:
        return False
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

def retrieve(client: cdsapi.Client, dataset: str, request: dict, target: Path, *, force: bool, root=None) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    root = root or (target.parent.parent if target.parent.name == "raw" else target.parent)
    exp_size, exp_sha = load_expected_from_manifest(root, target.name)
    if force:
        # Area/content change invalidates prior FETCH_MANIFEST expectations.
        exp_size, exp_sha = None, None
    if not force:
        if exp_size is not None and exp_sha is not None:
            if file_usable(target, expected_size=exp_size, expected_sha=exp_sha):
                print("reuse raw (verified size+sha)", target, target.stat().st_size, flush=True)
                return
        else:
            # No FETCH expectation: do not weakly reuse unless explicitly allowed.
            if (
                os.environ.get("TRAJECTA_ALLOW_UNVERIFIED_RAW") == "1"
                and file_usable(target, expected_size=None, expected_sha=None)
            ):
                print("reuse raw (UNVERIFIED, env override)", target, flush=True)
                return
            if target.is_file():
                print("refuse weak reuse without FETCH size/sha:", target.name, flush=True)
    if target.is_file():
        print("discard unusable/forced", target, flush=True)
        target.unlink()
    part = target.with_suffix(target.suffix + ".part")
    if part.is_file():
        part.unlink()
    print("requesting", dataset, "->", target, flush=True)
    print("area", request.get("area"), flush=True)
    client.retrieve(dataset, request, str(part))
    promote_part(part, target, exp_size=exp_size, exp_sha=exp_sha)

def main() -> int:
    import cdsapi

    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-official"),
        help="Dataset root; raw files go to <out-dir>/raw/",
    )
    parser.add_argument("--date", default="2018-12-01")
    parser.add_argument("--times", nargs="+", default=DEFAULT_TIMES)
    parser.add_argument(
        "--area",
        nargs=4,
        type=float,
        metavar=("NORTH", "WEST", "SOUTH", "EAST"),
        default=DEFAULT_AREA_NSWE,
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Re-download even if raw exists (also auto-redownloads stamped files)",
    )
    args = parser.parse_args()
    area = list(args.area)
    if area[0] <= area[2] or area[1] >= area[3]:
        raise SystemExit("--area must satisfy north > south and west < east")
    raw = args.out_dir / "raw"
    raw.mkdir(parents=True, exist_ok=True)

    client = cdsapi.Client()
    targets = []
    for item in request_plan(args.date, args.times, area):
        target = args.out_dir / item["target"]
        retrieve(
            client,
            item["dataset"],
            item["request"],
            target,
            force=args.force,
            root=args.out_dir,
        )
        targets.append(target)

    records = []
    for path in targets:
        rec = {
            "name": path.name,
            "relative_path": str(path.relative_to(args.out_dir).as_posix()),
            "size": path.stat().st_size,
            "sha256": sha256_file(path),
        }
        records.append(rec)
        print(rec["relative_path"], rec["size"], rec["sha256"], flush=True)

    manifest = {
        "dataset": "era5_cds_pressure_raw",
        "source": "https://cds.climate.copernicus.eu",
        "datasets": [
            "reanalysis-era5-pressure-levels",
            "reanalysis-era5-single-levels",
        ],
        "date": args.date,
        "times": args.times,
        "area_nswe": area,
        "pressure_levels_hpa": PRESSURE_LEVELS,
        "license": "Copernicus CDS licence (see CDS terms)",
        "attribution": "Generated using Copernicus Climate Change Service information",
        "fetch_tool": "tools/fetch_era5_pressure_cds.py",
        "notes": [
            "raw/ holds service download bytes only; never stamp or rewrite.",
            "No API keys recorded.",
        ],
        "files": records,
    }
    out = args.out_dir / "FETCH_MANIFEST.json"
    out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("wrote", out, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
