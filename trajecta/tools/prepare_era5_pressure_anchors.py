#!/usr/bin/env python3
"""Prepare ERA5 pressure anchors: raw stays immutable; ready is rebuilt.

Layout:
  raw/     # service original bytes (read-only; never stamp)
  ready/   # stamped/merged/classic fixtures for Trajecta
  FETCH_MANIFEST.json   # raw only (no self-hash, no CDS keys)
  PREPARE_MANIFEST.json # ready only
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from netCDF4 import Dataset

FAMILY = "era5_cf_pressure_netcdf"
SKIP_SCALAR = {"number", "expver"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def coords_equal(a: Dataset, b: Dataset, names: tuple[str, ...]) -> None:
    for name in names:
        if name not in a.variables or name not in b.variables:
            raise SystemExit(f"missing coordinate {name}")
        av = np.asarray(a.variables[name][:])
        bv = np.asarray(b.variables[name][:])
        if av.shape != bv.shape or not np.array_equal(av, bv):
            raise SystemExit(f"coordinate mismatch on {name}: {av} vs {bv}")


def copy_var(src, d, vname: str, dims: tuple[str, ...] | None = None, *, classic: bool = False) -> None:
    if vname in SKIP_SCALAR or vname in d.variables:
        return
    v = src.variables[vname]
    dtype = np.dtype(v.dtype)
    if dtype.kind in {"U", "S", "O"}:
        return
    # NETCDF3_CLASSIC has no int64; promote to float64.
    if classic and dtype == np.dtype("i8"):
        dtype = np.dtype("f8")
    use_dims = dims if dims is not None else v.dimensions
    fill = getattr(v, "_FillValue", None) if "_FillValue" in v.ncattrs() else None
    # NETCDF3_CLASSIC is picky about fill type; coordinates never need fill.
    if classic or vname in {"valid_time", "latitude", "longitude", "pressure_level"}:
        fill = None
    var = d.createVariable(vname, dtype, use_dims, fill_value=fill)
    for attr in v.ncattrs():
        if attr == "_FillValue":
            continue
        try:
            var.setncattr(attr, getattr(v, attr))
        except Exception:
            var.setncattr(attr, str(getattr(v, attr)))
    var[:] = np.asarray(v[:], dtype=dtype)


def write_ready_nc(src: Path, dest: Path, *, role: str, keep: set[str] | None = None) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(src) as s, Dataset(dest, "w", format="NETCDF4") as d:
        for name, dim in s.dimensions.items():
            d.createDimension(name, len(dim) if not dim.isunlimited() else None)
        for attr in s.ncattrs():
            try:
                d.setncattr(attr, getattr(s, attr))
            except Exception:
                d.setncattr(attr, str(getattr(s, attr)))
        d.setncattr("dataset_family", FAMILY)
        d.setncattr("role", role)
        d.setncattr("source_product", "copernicus_cds_era5")
        d.setncattr("source_file", src.name)
        d.setncattr(
            "anchor_note",
            "Ready copy of immutable CDS raw; values unchanged; fingerprint attrs only",
        )
        for vname in s.variables:
            if keep is not None and vname not in keep:
                continue
            if vname in SKIP_SCALAR:
                continue
            copy_var(s, d, vname)


def merge_surface(base: Path, flux: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(base) as b, Dataset(flux) as f, Dataset(dest, "w", format="NETCDF4") as d:
        coords_equal(b, f, ("valid_time", "latitude", "longitude"))
        for name, dim in b.dimensions.items():
            d.createDimension(name, len(dim) if not dim.isunlimited() else None)
        for attr in b.ncattrs():
            try:
                d.setncattr(attr, getattr(b, attr))
            except Exception:
                d.setncattr(attr, str(getattr(b, attr)))
        d.setncattr("dataset_family", FAMILY)
        d.setncattr("role", "surface")
        d.setncattr("merged_from", f"{base.name}+{flux.name}")
        d.setncattr(
            "anchor_note",
            "Merged CDS surface base+flux into ready surface role; values unchanged",
        )
        for src in (b, f):
            for vname in src.variables:
                if vname in SKIP_SCALAR or vname in d.variables:
                    continue
                copy_var(src, d, vname)


def convert_classic(src: Path, dst: Path, keep_vars: list[str]) -> None:
    """Classic NetCDF3 dual-backend fixture; retains 3-D z when present."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(src) as s, Dataset(dst, "w", format="NETCDF3_CLASSIC") as d:
        for name, dim in s.dimensions.items():
            d.createDimension(name, len(dim) if not dim.isunlimited() else None)
        d.setncattr("dataset_family", FAMILY)
        if "role" in s.ncattrs():
            d.setncattr("role", s.getncattr("role"))
        d.setncattr("source_product", "copernicus_cds_era5")
        d.setncattr(
            "anchor_note",
            "Classic NetCDF3 dual-backend fixture derived from ready NetCDF4; values unchanged",
        )
        for vname in keep_vars:
            if vname not in s.variables:
                continue
            copy_var(s, d, vname, classic=True)


def file_record(path: Path, root: Path) -> dict:
    return {
        "name": path.name,
        "relative_path": str(path.relative_to(root).as_posix()),
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-official"),
    )
    parser.add_argument(
        "--classic-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-classic"),
    )
    args = parser.parse_args()
    root = args.root
    raw = root / "raw"
    ready = root / "ready"
    pressure_raw = raw / "era5_pressure_20181201.nc"
    base_raw = raw / "era5_surface_base_20181201.nc"
    flux_raw = raw / "era5_surface_flux_20181201.nc"
    for path in (pressure_raw, base_raw, flux_raw):
        if not path.is_file():
            raise SystemExit(
                f"missing immutable raw input {path}; run tools/fetch_era5_pressure_cds.py --force"
            )
        # Refuse stamped "raw" leftovers from earlier sessions.
        with Dataset(path) as ds:
            fam = getattr(ds, "dataset_family", None)
            note = str(getattr(ds, "anchor_note", "") or "")
            if fam or "Trajecta" in note or "Official CDS NetCDF4 with Trajecta" in note:
                raise SystemExit(
                    f"raw is not service-original (Trajecta attrs present): {path}. "
                    "Re-run: python tools/fetch_era5_pressure_cds.py --force"
                )

    if ready.exists():
        shutil.rmtree(ready)
    ready.mkdir(parents=True)

    write_ready_nc(
        pressure_raw,
        ready / "era5_pressure_20181201.nc",
        role="pressure",
        keep={
            "valid_time",
            "pressure_level",
            "latitude",
            "longitude",
            "t",
            "u",
            "v",
            "q",
            "w",
            "z",
        },
    )
    merge_surface(base_raw, flux_raw, ready / "era5_surface_20181201.nc")

    # Classic dual-backend fixture keeps 3-D z (role+layout disambiguates).
    convert_classic(
        ready / "era5_pressure_20181201.nc",
        args.classic_dir / "era5_pressure_20181201.nc",
        [
            "valid_time",
            "pressure_level",
            "latitude",
            "longitude",
            "t",
            "u",
            "v",
            "q",
            "w",
            "z",
        ],
    )
    convert_classic(
        ready / "era5_surface_20181201.nc",
        args.classic_dir / "era5_surface_20181201.nc",
        [
            "valid_time",
            "latitude",
            "longitude",
            "sp",
            "z",
            "u10",
            "v10",
            "t2m",
            "d2m",
            "fsr",
            "blh",
            "zust",
            "ishf",
            "ie",
        ],
    )

    fetch = {
        "dataset": "era5_cds_pressure_raw",
        "dataset_family": FAMILY,
        "date": "2018-12-01",
        "times_utc": ["00", "06", "12"],
        "source": "https://cds.climate.copernicus.eu",
        "product": "reanalysis-era5-pressure-levels + reanalysis-era5-single-levels",
        "files": [file_record(p, root) for p in sorted(raw.glob("*.nc"))],
        "notes": [
            "Raw CDS service bytes only; never stamp or mutate.",
            "No API keys or key fragments are recorded.",
        ],
    }
    prepare = {
        "dataset": "era5_cds_pressure_ready",
        "dataset_family": FAMILY,
        "date": "2018-12-01",
        "times_utc": ["00", "06", "12"],
        "files": [file_record(p, root) for p in sorted(ready.glob("*.nc"))]
        + [file_record(p, args.classic_dir.parent if False else p.parent) for p in []],
        "classic_files": [
            {
                "name": p.name,
                "path": str(p.as_posix()),
                "size": p.stat().st_size,
                "sha256": sha256_file(p),
            }
            for p in sorted(args.classic_dir.glob("*.nc"))
        ],
        "notes": [
            "Ready files are deterministically rebuilt from raw.",
            "Classic NetCDF3 retains 3-D geopotential z for role/layout selection.",
        ],
        "commands": [
            "python tools/fetch_era5_pressure_cds.py",
            "python tools/prepare_era5_pressure_anchors.py",
        ],
    }
    # Fix classic path records relative to classic dir
    prepare["files"] = [file_record(p, root) for p in sorted(ready.glob("*.nc"))]
    (root / "FETCH_MANIFEST.json").write_text(json.dumps(fetch, indent=2) + "\n", encoding="utf-8")
    (root / "PREPARE_MANIFEST.json").write_text(
        json.dumps(prepare, indent=2) + "\n", encoding="utf-8"
    )
    for rec in fetch["files"] + prepare["files"] + prepare["classic_files"]:
        print(rec.get("name") or rec.get("relative_path"), rec["size"], rec["sha256"])
    print("wrote FETCH/PREPARE manifests under", root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
