#!/usr/bin/env python3
"""Offline fetch of a tiny real ERA5 pressure-level case from CDS.

Credentials: ~/.cdsapirc or CDSAPI_URL / CDSAPI_KEY environment variables.
Does not run from the Trajecta runtime; prepare data under target/test-data/.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import cdsapi


AREA_NSWE = [52, 0, 48, 6]  # small 4x6 deg box over western Europe
PRESSURE_LEVELS = [
    "1", "2", "3", "5", "7", "10", "20", "30", "50", "70",
    "100", "125", "150", "175", "200", "225", "250", "300", "350", "400",
    "450", "500", "550", "600", "650", "700", "750", "775", "800", "825",
    "850", "875", "900", "925", "950", "975", "1000",
]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-official"),
    )
    parser.add_argument("--date", default="2018-12-01")
    parser.add_argument("--times", nargs="+", default=["00:00", "06:00"])
    args = parser.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    client = cdsapi.Client()
    year, month, day = args.date.split("-")
    records = []

    pressure_target = args.out_dir / f"era5_pressure_{year}{month}{day}.nc"
    if not pressure_target.is_file():
        print("requesting pressure levels ->", pressure_target, flush=True)
        client.retrieve(
            "reanalysis-era5-pressure-levels",
            {
                "product_type": "reanalysis",
                "variable": [
                    "temperature",
                    "u_component_of_wind",
                    "v_component_of_wind",
                    "specific_humidity",
                    "vertical_velocity",
                ],
                "pressure_level": PRESSURE_LEVELS,
                "year": year,
                "month": month,
                "day": day,
                "time": args.times,
                "area": AREA_NSWE,
                "format": "netcdf",
            },
            str(pressure_target),
        )
    else:
        print("reuse", pressure_target)

    surface_target = args.out_dir / f"era5_surface_{year}{month}{day}.nc"
    if not surface_target.is_file():
        print("requesting surface ->", surface_target, flush=True)
        client.retrieve(
            "reanalysis-era5-single-levels",
            {
                "product_type": "reanalysis",
                "variable": [
                    "surface_pressure",
                    "geopotential",
                ],
                "year": year,
                "month": month,
                "day": day,
                "time": args.times,
                "area": AREA_NSWE,
                "format": "netcdf",
            },
            str(surface_target),
        )
    else:
        print("reuse", surface_target)

    for path in (pressure_target, surface_target):
        size = path.stat().st_size
        digest = sha256_file(path)
        records.append(
            {
                "name": path.name,
                "path": str(path.as_posix()),
                "size": size,
                "sha256": digest,
            }
        )
        print(path.name, size, digest)

    manifest = {
        "dataset": "era5_cds_pressure_official",
        "source": "https://cds.climate.copernicus.eu",
        "datasets": [
            "reanalysis-era5-pressure-levels",
            "reanalysis-era5-single-levels",
        ],
        "date": args.date,
        "times": args.times,
        "area_nswe": AREA_NSWE,
        "pressure_levels_hpa": PRESSURE_LEVELS,
        "license": "Copernicus CDS licence (see CDS terms)",
        "attribution": "Generated using Copernicus Climate Change Service information",
        "files": records,
    }
    out = args.out_dir / "FETCH_MANIFEST.json"
    out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("wrote", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
