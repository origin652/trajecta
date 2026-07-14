#!/usr/bin/env python3
"""Convert official CDS ERA5 NetCDF4 to classic NetCDF3 dual-backend anchors.

Values are copied unchanged. Scalar ensemble metadata (number/expver) is dropped
because pure-Rust netcdf-reader currently requires DIMENSION_LIST on all datasets.
"""
from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
from netCDF4 import Dataset


def convert(src: Path, dst: Path, keep_vars: list[str]) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(src) as s, Dataset(dst, "w", format="NETCDF3_CLASSIC") as d:
        for name, dim in s.dimensions.items():
            d.createDimension(name, len(dim) if not dim.isunlimited() else None)
        for attr in s.ncattrs():
            try:
                d.setncattr(attr, getattr(s, attr))
            except Exception:
                d.setncattr(attr, str(getattr(s, attr)))
        d.setncattr("dataset_family", "era5_cf_pressure_netcdf")
        d.setncattr("source_product", "copernicus_cds_era5")
        d.setncattr("source_file", src.name)
        d.setncattr(
            "conversion",
            "offline classic NetCDF3 subset for dual-backend readers; data values unchanged",
        )
        for vname in keep_vars:
            v = s.variables[vname]
            dtype = v.dtype
            if dtype.kind in {"U", "S", "O"}:
                continue
            if vname == "valid_time":
                dtype = "f8"
            elif str(dtype) == "int64":
                dtype = "i4"
            var = d.createVariable(vname, dtype, v.dimensions)
            for attr in v.ncattrs():
                if attr == "_FillValue":
                    continue
                try:
                    var.setncattr(attr, getattr(v, attr))
                except Exception:
                    var.setncattr(attr, str(getattr(v, attr)))
            data = v[:]
            if vname == "valid_time":
                data = np.array(data, dtype="f8")
            var[:] = data


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--src-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-official"),
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/test-data/era5-cds-pressure-classic"),
    )
    args = parser.parse_args()
    convert(
        args.src_dir / "era5_pressure_20181201.nc",
        args.out_dir / "era5_pressure_20181201.nc",
        ["valid_time", "pressure_level", "latitude", "longitude", "t", "u", "v", "q", "w"],
    )
    convert(
        args.src_dir / "era5_surface_20181201.nc",
        args.out_dir / "era5_surface_20181201.nc",
        ["valid_time", "latitude", "longitude", "sp", "z"],
    )
    print("wrote classic anchors under", args.out_dir)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
