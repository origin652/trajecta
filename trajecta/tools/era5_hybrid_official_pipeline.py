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
import math
import os
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "target/test-data/era5-cds-hybrid137-official"
# A-frozen expanded box: N, W, S, E
DEFAULT_AREA = [53.0, 0.0, 45.0, 10.0]
DEFAULT_DATE = "2018-12-01"
TIMES = ["00:00", "03:00", "06:00"]


def request_plan(date: str, times: list[str], area: list[float]) -> list[dict]:
    """Return exact CDS requests without reading credentials or using the network."""
    year, month, day = date.split("-")
    stamp = f"{year}{month}{day}"
    mars_common = {
        "class": "ea",
        "date": date,
        "expver": "1",
        "stream": "oper",
        "type": "an",
        "area": "/".join(str(x) for x in area),
        "grid": "0.25/0.25",
    }
    single_common = {
        "product_type": "reanalysis",
        "year": year,
        "month": month,
        "day": day,
        "time": times,
        "area": area,
        "data_format": "netcdf",
        "download_format": "unarchived",
    }
    mars_times = "/".join(time.removesuffix(":00") for time in times)
    return [
        {
            "dataset": "reanalysis-era5-complete",
            "request": mars_common
            | {
                "levelist": "/".join(str(level) for level in range(1, 138)),
                "levtype": "ml",
                "param": "130/131/132/133/135",
                "time": mars_times,
                "format": "netcdf",
            },
            "target": f"raw/era5_hybrid137_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-complete",
            "request": mars_common
            | {
                "levelist": "1",
                "levtype": "ml",
                "param": "152",
                "time": mars_times,
                "format": "netcdf",
            },
            "target": f"raw/era5_lnsp_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-single-levels",
            "request": single_common
            | {
                "variable": [
                    "geopotential",
                    "10m_u_component_of_wind",
                    "10m_v_component_of_wind",
                    "2m_temperature",
                    "2m_dewpoint_temperature",
                    "forecast_surface_roughness",
                    "boundary_layer_height",
                    "friction_velocity",
                ]
            },
            "target": f"raw/era5_hybrid_surface_base_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-single-levels",
            "request": single_common
            | {
                "variable": [
                    "instantaneous_surface_sensible_heat_flux",
                    "instantaneous_moisture_flux",
                ]
            },
            "target": f"raw/era5_hybrid_surface_flux_{stamp}.nc",
        },
        {
            "dataset": "reanalysis-era5-complete",
            "request": mars_common
            | {
                "levelist": "137",
                "levtype": "ml",
                "param": "130",
                "time": times[0].removesuffix(":00"),
                "format": "grib",
            },
            "target": "raw/era5_hybrid_pv_probe.grib",
        },
    ]


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

def retrieve(
    client: cdsapi.Client,
    dataset: str,
    request: dict,
    target: Path,
    *,
    root: Path,
    force: bool,
) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
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

def fetch_all(
    out: Path,
    times: list[str],
    *,
    date: str,
    area: list[float],
    force: bool,
) -> None:
    import cdsapi

    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    client = cdsapi.Client()
    for item in request_plan(date, times, area):
        retrieve(
            client,
            item["dataset"],
            item["request"],
            out / item["target"],
            root=out,
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
    (out / "FETCH_MANIFEST.json").write_text(
        json.dumps(
            {
                "dataset": "era5_cds_hybrid137_raw",
                "date": date,
                "area_nswe": area,
                "times_utc": [time.removesuffix(":00") for time in times],
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


def grib2_hybrid_coefficients(raw: bytes) -> tuple[list[float], list[float]]:
    """Extract the GRIB2 Section 4 PV array without Cargo or Python bindings."""
    candidates: list[tuple[list[float], list[float]]] = []
    offset = 0
    while offset < len(raw):
        if raw[offset : offset + 4] != b"GRIB" or offset + 16 > len(raw):
            raise ValueError(f"invalid GRIB message boundary at byte {offset}")
        if raw[offset + 7] != 2:
            raise ValueError("the ERA5 hybrid PV probe must use GRIB edition 2")
        message_length = int.from_bytes(raw[offset + 8 : offset + 16], "big")
        message_end = offset + message_length
        if message_length < 20 or message_end > len(raw):
            raise ValueError("truncated GRIB2 message")
        if raw[message_end - 4 : message_end] != b"7777":
            raise ValueError("GRIB2 message has no end marker")

        cursor = offset + 16
        section_limit = message_end - 4
        while cursor < section_limit:
            if cursor + 5 > section_limit:
                raise ValueError("truncated GRIB2 section header")
            section_length = int.from_bytes(raw[cursor : cursor + 4], "big")
            section_end = cursor + section_length
            if section_length < 5 or section_end > section_limit:
                raise ValueError("invalid GRIB2 section length")
            if raw[cursor + 4] == 4:
                if section_length < 9:
                    raise ValueError("truncated GRIB2 product section")
                coordinate_count = int.from_bytes(raw[cursor + 5 : cursor + 7], "big")
                if coordinate_count:
                    if coordinate_count % 2:
                        raise ValueError("hybrid PV coordinate count must be even")
                    coordinate_bytes = coordinate_count * 4
                    values_start = section_end - coordinate_bytes
                    if values_start < cursor + 9:
                        raise ValueError("hybrid PV overlaps the product template")
                    values = [
                        float(value)
                        for (value,) in struct.iter_unpack(
                            ">f", raw[values_start:section_end]
                        )
                    ]
                    if any(not math.isfinite(value) for value in values):
                        raise ValueError("hybrid PV contains a non-finite coefficient")
                    split = coordinate_count // 2
                    candidates.append((values[:split], values[split:]))
            cursor = section_end
        offset = message_end

    if not candidates:
        raise ValueError("GRIB2 probe contains no hybrid PV coefficients")
    first = candidates[0]
    if any(candidate != first for candidate in candidates[1:]):
        raise ValueError("GRIB2 probe contains inconsistent hybrid PV arrays")
    if len(first[0]) < 2 or len(first[0]) != len(first[1]):
        raise ValueError("GRIB2 probe contains an invalid hybrid vertical grid")
    return first


def extract_coefficients(out: Path) -> Path:
    raw_probe = out / "raw" / "era5_hybrid_pv_probe.grib"
    derived = out / "derived"
    derived.mkdir(parents=True, exist_ok=True)
    coeff = derived / "era5_l137_ab_coefficients.json"
    if not raw_probe.is_file():
        raise SystemExit(f"missing PV probe {raw_probe}")
    try:
        a_half_pa, b_half = grib2_hybrid_coefficients(raw_probe.read_bytes())
    except (OSError, ValueError) as error:
        raise SystemExit(f"cannot extract hybrid coefficients: {error}") from error
    payload = {
        "source_grib": "raw/era5_hybrid_pv_probe.grib",
        "interface_count": len(a_half_pa),
        "full_level_count": len(a_half_pa) - 1,
        "a_half_pa": a_half_pa,
        "b_half": b_half,
        "active_full_levels": [len(a_half_pa) - 1],
        "note": "Coefficients extracted from official CDS ERA5 GRIB PV metadata; not invented.",
    }
    coeff.write_text(
        json.dumps(payload, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"wrote {coeff} interfaces={len(a_half_pa)} levels={len(a_half_pa) - 1}", flush=True)
    return coeff


def prepare(out: Path, times: list[str], date: str) -> None:
    # Point prepare script at correct layout (coeff in derived/).
    cmd = [
        sys.executable,
        str(ROOT / "tools/prepare_era5_hybrid_anchors.py"),
        "--root",
        str(out),
        "--times",
        *(time.removesuffix(":00") for time in times),
        "--date",
        date,
    ]
    print("+", " ".join(cmd), flush=True)
    proc = subprocess.run(cmd, cwd=ROOT)
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--times", nargs="+", default=TIMES)
    parser.add_argument("--date", default=DEFAULT_DATE)
    parser.add_argument(
        "--area",
        nargs=4,
        type=float,
        metavar=("NORTH", "WEST", "SOUTH", "EAST"),
        default=DEFAULT_AREA,
    )
    parser.add_argument("--force-fetch", action="store_true")
    parser.add_argument("--skip-fetch", action="store_true")
    args = parser.parse_args()
    area = list(args.area)
    if area[0] <= area[2] or area[1] >= area[3]:
        raise SystemExit("--area must satisfy north > south and west < east")
    out = args.out_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    if not args.skip_fetch:
        fetch_all(out, args.times, date=args.date, area=area, force=args.force_fetch)
    extract_coefficients(out)
    prepare(out, args.times, args.date)
    print("pipeline complete:", out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
