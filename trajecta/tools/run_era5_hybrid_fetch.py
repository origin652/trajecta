#!/usr/bin/env python3
"""Long-running ERA5 hybrid-137 official fetch (background-friendly)."""
from __future__ import annotations

import hashlib
import json
import shutil
import sys
import traceback
from pathlib import Path

import cdsapi

OUT = Path("target/test-data/era5-cds-hybrid137-official")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    client = cdsapi.Client()
    target = OUT / "era5_hybrid137_20181201.nc"
    if not (target.is_file() and target.stat().st_size > 0):
        part = OUT / "era5_hybrid137_20181201.nc.part"
        if part.is_file():
            part.unlink()
        req = {
            "class": "ea",
            "date": "2018-12-01",
            "expver": "1",
            "levelist": "/".join(str(i) for i in range(1, 138)),
            "levtype": "ml",
            "param": "130/131/132/133/135/152",
            "stream": "oper",
            "time": "00:00:00/03:00:00/06:00:00",
            "type": "an",
            "area": "52/0/48/6",
            "grid": "0.25/0.25",
            "format": "netcdf",
        }
        print("requesting reanalysis-era5-complete hybrid 1-137", flush=True)
        try:
            client.retrieve("reanalysis-era5-complete", req, str(part))
            part.replace(target)
            print("hybrid ok", target.stat().st_size, flush=True)
        except Exception as error:  # noqa: BLE001
            print("HYBRID_FAIL", error, flush=True)
            traceback.print_exc()
            (OUT / "FETCH_FAILURE.txt").write_text(str(error), encoding="utf-8")
            # keep going to write status manifest
    else:
        print("reuse", target, flush=True)

    # Companion surface files: reuse official pressure-area surface freeze when present.
    pressure_official = Path("target/test-data/era5-cds-pressure-official")
    for name in (
        "era5_surface_base_20181201.nc",
        "era5_surface_flux_20181201.nc",
    ):
        src = pressure_official / name
        dst = OUT / name
        if src.is_file() and not dst.is_file():
            shutil.copy2(src, dst)
            print("copied", src, "->", dst, flush=True)

    files = []
    for path in sorted(OUT.glob("*.nc")):
        files.append(
            {
                "name": path.name,
                "path": str(path.as_posix()),
                "size": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
        print(path.name, path.stat().st_size, files[-1]["sha256"], flush=True)

    ready = target.is_file() and target.stat().st_size > 0
    manifest = {
        "dataset": "era5_cds_hybrid137_official",
        "source": "https://cds.climate.copernicus.eu",
        "datasets": ["reanalysis-era5-complete", "reanalysis-era5-single-levels"],
        "date": "2018-12-01",
        "times": ["00:00", "03:00", "06:00"],
        "model_levels": list(range(1, 138)),
        "area_nswe": [52, 0, 48, 6],
        "grid": "0.25/0.25",
        "status": "ready" if ready else "blocked_or_pending",
        "fetch_tool": "tools/run_era5_hybrid_fetch.py",
        "fetch_command": "python tools/run_era5_hybrid_fetch.py",
        "files": files,
        "notes": [
            "Full 1-137 model levels; not flex_extract 130-137 subset.",
            "Surface companions may be shared with pressure-area freeze (same box/date).",
        ],
    }
    (OUT / "FETCH_MANIFEST.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("wrote manifest status=", manifest["status"], flush=True)
    return 0 if ready else 2


if __name__ == "__main__":
    sys.exit(main())
