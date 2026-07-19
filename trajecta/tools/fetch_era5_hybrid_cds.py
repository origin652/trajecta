"""DEPRECATED: use tools/era5_hybrid_official_pipeline.py"""
#!/usr/bin/env python3
"""Fetch official CDS ERA5 model-level (hybrid) M3 anchors on a regular lat/lon grid.

Requirements from M3 plan:
- 2018-12-01 00/03/06 UTC
- full model levels 1--137 (not the 130--137 flex_extract subset)
- regular lat/lon delivery from CDS (not reduced Gaussian)
- Transport + NearSurfaceTransport 3D/surface/near-surface inputs

Credentials: ~/.cdsapirc or CDSAPI_URL / CDSAPI_KEY.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

import cdsapi

# A-frozen expanded box: N, W, S, E
AREA_NSWE = [53, 0, 45, 10]
# Full 137 model levels.
MODEL_LEVELS = [str(level) for level in range(1, 138)]
MODEL_VARS = [
    "temperature",
    "u_component_of_wind",
    "v_component_of_wind",
    "specific_humidity",
    "vertical_velocity",  # omega on model levels when available via CDS
    "logarithm_of_surface_pressure",  # common ML companion; surface pressure also requested
]
# Surface / near-surface companion file.
SURFACE_VARS = [
    "surface_pressure",
    "geopotential",
    "10m_u_component_of_wind",
    "10m_v_component_of_wind",
    "2m_temperature",
    "2m_dewpoint_temperature",
    "forecast_surface_roughness",
    "boundary_layer_height",
    "instantaneous_surface_sensible_heat_flux",
    "instantaneous_surface_latent_heat_flux",
    "friction_velocity",
]
DEFAULT_TIMES = ["00:00", "03:00", "06:00"]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def retrieve_if_needed(client: cdsapi.Client, dataset: str, request: dict, target: Path) -> None:
    if target.is_file() and target.stat().st_size > 0:
        print("reuse", target, flush=True)
        return
    part = target.with_suffix(target.suffix + ".part")
    if part.is_file():
        part.unlink()
    print("requesting", dataset, "->", target, flush=True)
    print("request keys:", sorted(request.keys()), flush=True)
    client.retrieve(dataset, request, str(part))
    part.replace(target)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-hybrid137-official"),
    )
    parser.add_argument("--date", default="2018-12-01")
    parser.add_argument("--times", nargs="+", default=DEFAULT_TIMES)
    parser.add_argument(
        "--grid",
        default="0.25/0.25",
        help="regular lat/lon grid step as CDS 'grid' (default 0.25/0.25)",
    )
    args = parser.parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)

    client = cdsapi.Client()
    year, month, day = args.date.split("-")
    records = []

    model_target = args.out_dir / f"era5_hybrid137_{year}{month}{day}.nc"
    # CDS model-level product name (ERA5 complete).
    retrieve_if_needed(
        client,
        "reanalysis-era5-complete",
        {
            "class": "ea",
            "date": f"{year}-{month}-{day}",
            "expver": "1",
            "levelist": "/".join(MODEL_LEVELS),
            "levtype": "ml",
            "param": "130/131/132/133/135/152",  # T/U/V/q/w/lnsp
            "stream": "oper",
            "time": "/".join(t.replace(":", "")[:2] + ":00:00" if ":" in t else t for t in args.times),
            "type": "an",
            "area": "/".join(str(v) for v in AREA_NSWE),
            "grid": args.grid,
            "format": "netcdf",
        },
        model_target,
    )

    surface_target = args.out_dir / f"era5_hybrid_surface_{year}{month}{day}.nc"
    retrieve_if_needed(
        client,
        "reanalysis-era5-single-levels",
        {
            "product_type": "reanalysis",
            "variable": SURFACE_VARS,
            "year": year,
            "month": month,
            "day": day,
            "time": args.times,
            "area": AREA_NSWE,
            "format": "netcdf",
        },
        surface_target,
    )

    for path in (model_target, surface_target):
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
        print(path.name, size, digest, flush=True)

    manifest = {
        "dataset": "era5_cds_hybrid137_official",
        "source": "https://cds.climate.copernicus.eu",
        "datasets": ["reanalysis-era5-complete", "reanalysis-era5-single-levels"],
        "date": args.date,
        "times": args.times,
        "area_nswe": AREA_NSWE,
        "model_levels": MODEL_LEVELS,
        "grid": args.grid,
        "surface_variables": SURFACE_VARS,
        "license": "Copernicus CDS licence (see CDS terms)",
        "attribution": "Generated using Copernicus Climate Change Service information",
        "fetch_tool": "tools/fetch_era5_hybrid_cds.py",
        "fetch_command": (
            f"python tools/fetch_era5_hybrid_cds.py --out-dir {args.out_dir.as_posix()} "
            f"--date {args.date} --times {' '.join(args.times)} --grid {args.grid}"
        ),
        "notes": [
            "Must not be confused with flex_extract 130-137 subset fixtures.",
            "Regular lat/lon grid requested from CDS; reduced Gaussian is out of M3 scope.",
        ],
        "files": records,
    }
    out = args.out_dir / "FETCH_MANIFEST.json"
    out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("wrote", out, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
