#!/usr/bin/env python3
"""Prepare official CDS ERA5 hybrid-137 anchors (raw immutable / ready rebuild).

Layout under --root:
  raw/     # CDS original bytes (never stamp)
  ready/   # hybrid prepared + surface merged (fingerprint + CF A/B + lnsp)
  FETCH_MANIFEST.json
  PREPARE_MANIFEST.json

Canonical surface_pressure is Derived from lnsp in the Profile. Ready hybrid
keeps lnsp as Source and may retain convenience sp only for CF formula_terms.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import numpy as np
from netCDF4 import Dataset

FAMILY = "era5_cds_hybrid137"
SKIP_SCALAR = {"number", "expver"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def coords_equal(datasets: list[Dataset], names: tuple[str, ...]) -> None:
    ref = datasets[0]
    for name in names:
        if name not in ref.variables:
            raise SystemExit(f"missing coordinate {name} in reference")
        rv = np.asarray(ref.variables[name][:])
        for ds in datasets[1:]:
            if name not in ds.variables:
                raise SystemExit(f"missing coordinate {name}")
            av = np.asarray(ds.variables[name][:])
            if av.shape != rv.shape or not np.array_equal(av, rv):
                raise SystemExit(f"coordinate mismatch on {name}")


def file_record(path: Path, root: Path) -> dict:
    return {
        "name": path.name,
        "relative_path": str(path.relative_to(root).as_posix()),
        "size": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def merge_surface(base: Path, flux: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(base) as b, Dataset(flux) as f, Dataset(dest, "w", format="NETCDF4") as d:
        coords_equal([b, f], ("valid_time", "latitude", "longitude"))
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
            "Ready surface role from immutable CDS base+flux; values unchanged",
        )
        # Hybrid prepared owns lnsp-derived surface pressure; skip surface sp.
        skip = set(SKIP_SCALAR) | {"sp"}
        for src in (b, f):
            for vname, v in src.variables.items():
                if vname in d.variables or vname in skip:
                    continue
                dtype = np.dtype(v.dtype)
                if dtype.kind in {"U", "S", "O"}:
                    continue
                fill = getattr(v, "_FillValue", None) if "_FillValue" in v.ncattrs() else None
                var = d.createVariable(vname, dtype, v.dimensions, fill_value=fill)
                for attr in v.ncattrs():
                    if attr == "_FillValue":
                        continue
                    try:
                        var.setncattr(attr, getattr(v, attr))
                    except Exception:
                        var.setncattr(attr, str(getattr(v, attr)))
                var[:] = v[:]


def prepare_hybrid(
    hybrid_src: Path,
    lnsp_src: Path,
    coeff_json: Path,
    dest: Path,
) -> None:
    coeffs = json.loads(coeff_json.read_text(encoding="utf-8"))
    a = np.asarray(coeffs["a_half_pa"], dtype="f8")
    b = np.asarray(coeffs["b_half"], dtype="f8")
    if a.shape != b.shape or a.size < 2:
        raise SystemExit(f"invalid coefficients in {coeff_json}")
    ninterface = int(a.size)
    nlevel = ninterface - 1

    dest.parent.mkdir(parents=True, exist_ok=True)
    with Dataset(hybrid_src) as s, Dataset(lnsp_src) as ln, Dataset(dest, "w", format="NETCDF4") as d:
        coords_equal([s, ln], ("valid_time", "latitude", "longitude"))
        ntime = len(s.dimensions["valid_time"])
        nlat = len(s.dimensions["latitude"])
        nlon = len(s.dimensions["longitude"])
        nlev = len(s.dimensions["model_level"])
        if nlev != nlevel:
            raise SystemExit(f"level count {nlev} != coefficient full levels {nlevel}")

        d.createDimension("valid_time", ntime)
        d.createDimension("hybrid", nlev)
        d.createDimension("latitude", nlat)
        d.createDimension("longitude", nlon)
        d.createDimension("nhyi", ninterface)

        for attr in s.ncattrs():
            try:
                d.setncattr(attr, getattr(s, attr))
            except Exception:
                d.setncattr(attr, str(getattr(s, attr)))
        d.setncattr("dataset_family", FAMILY)
        d.setncattr("role", "hybrid")
        d.setncattr("source_product", "copernicus_cds_era5_complete")
        d.setncattr(
            "hybrid_coefficients_source",
            coeffs.get("source_grib", "CDS GRIB PV probe"),
        )
        d.setncattr(
            "anchor_note",
            "Ready hybrid: raw 3D + GRIB-PV A/B + lnsp Source (+ convenience sp for CF only)",
        )

        def copy_1d(src_name: str, dst_name: str, dims: tuple[str, ...]) -> None:
            v = s.variables[src_name]
            dtype = np.dtype(v.dtype)
            # Classic-safe: store times as float64 when original is int64.
            if dtype == np.dtype("i8"):
                dtype = np.dtype("f8")
            fill = getattr(v, "_FillValue", None) if "_FillValue" in v.ncattrs() else None
            var = d.createVariable(dst_name, dtype, dims, fill_value=fill)
            for attr in v.ncattrs():
                if attr == "_FillValue":
                    continue
                try:
                    var.setncattr(attr, getattr(v, attr))
                except Exception:
                    var.setncattr(attr, str(getattr(v, attr)))
            var[:] = np.asarray(v[:], dtype=dtype)

        copy_1d("valid_time", "valid_time", ("valid_time",))
        copy_1d("latitude", "latitude", ("latitude",))
        copy_1d("longitude", "longitude", ("longitude",))

        hybrid = d.createVariable("hybrid", "f8", ("hybrid",))
        hybrid[:] = np.asarray(s.variables["model_level"][:], dtype="f8")
        hybrid.setncattr("standard_name", "atmosphere_hybrid_sigma_pressure_coordinate")
        hybrid.setncattr("long_name", "hybrid model level")
        hybrid.setncattr("units", "1")
        hybrid.setncattr("positive", "down")
        hybrid.setncattr("formula", "p(n,k,j,i) = ap(k) + b(k)*ps(n,j,i)")
        # CF formula_terms may reference convenience sp; Profile uses lnsp as Source.
        hybrid.setncattr("formula_terms", "ap: ap b: b ps: sp")

        for name in ("t", "u", "v", "q", "w"):
            if name not in s.variables:
                raise SystemExit(f"missing {name} in {hybrid_src}")
            v = s.variables[name]
            fill = getattr(v, "_FillValue", None) if "_FillValue" in v.ncattrs() else None
            var = d.createVariable(
                name,
                np.dtype(v.dtype),
                ("valid_time", "hybrid", "latitude", "longitude"),
                fill_value=fill,
            )
            for attr in v.ncattrs():
                if attr == "_FillValue":
                    continue
                try:
                    var.setncattr(attr, getattr(v, attr))
                except Exception:
                    var.setncattr(attr, str(getattr(v, attr)))
            var[:] = v[:]

        ap = d.createVariable("ap", "f8", ("nhyi",))
        ap.units = "Pa"
        ap.long_name = "hybrid A coefficient at half-levels/interfaces"
        ap[:] = a
        bb = d.createVariable("b", "f8", ("nhyi",))
        bb.units = "1"
        bb.long_name = "hybrid B coefficient at half-levels/interfaces"
        bb[:] = b

        lnsp = np.array(ln.variables["lnsp"][:], dtype="f8")
        if lnsp.ndim == 4 and lnsp.shape[1] == 1:
            lnsp = lnsp[:, 0, :, :]
        if lnsp.shape != (ntime, nlat, nlon):
            raise SystemExit(f"lnsp shape {lnsp.shape} != {(ntime, nlat, nlon)}")

        # Source field for Profile: logarithmic surface pressure.
        lnsp_var = d.createVariable("lnsp", "f8", ("valid_time", "latitude", "longitude"))
        lnsp_var.units = "1"
        lnsp_var.long_name = "Logarithm of surface pressure"
        lnsp_var.standard_name = "logarithm_of_surface_air_pressure"
        lnsp_var.comment = f"copied from immutable raw {lnsp_src.name}"
        lnsp_var[:] = lnsp

        # Convenience sp for CF formula_terms only (not Profile Source).
        sp = d.createVariable("sp", "f8", ("valid_time", "latitude", "longitude"))
        sp.units = "Pa"
        sp.standard_name = "surface_air_pressure"
        sp.long_name = "Surface pressure convenience field exp(lnsp) for CF formula_terms"
        sp.comment = "convenience only; Trajecta Profile derives sp from lnsp"
        sp[:] = np.exp(lnsp)


def assert_raw_unstamped(path: Path) -> None:
    if path.suffix.lower() != ".nc":
        return
    with Dataset(path) as ds:
        fam = getattr(ds, "dataset_family", None)
        role = getattr(ds, "role", None)
        note = str(getattr(ds, "anchor_note", "") or "")
        if fam or role or "Trajecta" in note or "Ready" in note:
            raise SystemExit(
                f"raw file is not service-original (has Trajecta attrs): {path} "
                f"family={fam!r} role={role!r}. Re-run era5_hybrid_official_pipeline.py --force-fetch"
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        type=Path,
        default=Path("target/test-data/era5-cds-hybrid137-official"),
    )
    parser.add_argument("--times", nargs="+", default=["00", "03", "06"])
    parser.add_argument("--date", default="2018-12-01")
    args = parser.parse_args()
    stamp = args.date.replace("-", "")
    if len(stamp) != 8 or not stamp.isdigit():
        raise SystemExit("--date must be YYYY-MM-DD")
    root = args.root
    raw = root / "raw"
    derived = root / "derived"
    hybrid_src = raw / f"era5_hybrid137_{stamp}.nc"
    lnsp_src = raw / f"era5_lnsp_{stamp}.nc"
    # Coefficients are derived, never stored as raw service bytes.
    coeff = derived / "era5_l137_ab_coefficients.json"
    if not coeff.is_file():
        # legacy path during migration only
        legacy = raw / "era5_l137_ab_coefficients.json"
        if legacy.is_file():
            derived.mkdir(parents=True, exist_ok=True)
            shutil.move(str(legacy), str(coeff))
    base = raw / f"era5_hybrid_surface_base_{stamp}.nc"
    flux = raw / f"era5_hybrid_surface_flux_{stamp}.nc"
    for path in (hybrid_src, lnsp_src, base, flux):
        if not path.is_file():
            raise SystemExit(
                f"missing required raw input {path}; run tools/era5_hybrid_official_pipeline.py"
            )
        assert_raw_unstamped(path)
    if not coeff.is_file():
        raise SystemExit(f"missing derived coefficients {coeff}")

    ready = root / "ready"
    ready.mkdir(parents=True, exist_ok=True)

    prepared = ready / f"era5_hybrid137_prepared_{stamp}.nc"
    surface = ready / f"era5_surface_{stamp}.nc"
    prepared.unlink(missing_ok=True)
    surface.unlink(missing_ok=True)
    prepare_hybrid(hybrid_src, lnsp_src, coeff, prepared)
    merge_surface(base, flux, surface)

    # FETCH is owned by the pipeline fetch step; only rewrite PREPARE here.
    prepare = {
        "dataset": "era5_cds_hybrid137_ready",
        "dataset_family": FAMILY,
        "date": args.date,
        "times_utc": args.times,
        "model_levels": list(range(1, 138)),
        "primary_files": {
            "hybrid_prepared": f"ready/era5_hybrid137_prepared_{stamp}.nc",
            "surface_merged": f"ready/era5_surface_{stamp}.nc",
            "coefficients": "derived/era5_l137_ab_coefficients.json",
        },
        "files": [file_record(p, root) for p in sorted(ready.glob("*.nc"))]
        + ([file_record(coeff, root)] if coeff.is_file() else []),
        "commands": [
            "python tools/era5_hybrid_official_pipeline.py",
            "python tools/prepare_era5_hybrid_anchors.py",
        ],
        "notes": [
            "Ready rebuilt deterministically from raw; raw never mutated.",
            "A/B coefficients are derived/ not raw/.",
            "Profile derives surface_pressure from lnsp; convenience sp is CF-only.",
        ],
    }
    (root / "PREPARE_MANIFEST.json").write_text(
        json.dumps(prepare, indent=2) + "\n", encoding="utf-8"
    )
    for rec in prepare["files"]:
        print(rec["relative_path"], rec["size"], rec["sha256"])
    print("wrote PREPARE under", root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
