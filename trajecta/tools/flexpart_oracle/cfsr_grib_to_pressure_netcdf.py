#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Convert frozen CFSR PGBl GRIB2 files into pressure-meter NetCDF inputs.

Uses the native ecCodes CLI (grib_copy / grib_get_data / grib_ls) rather than the
Python bindings, because some Windows Python eccodes builds silently drop the V
wind messages from these NCEI files. This remains a GPL-side oracle adapter and
never calls Trajecta readers.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[2]
NATIVE_BIN = ROOT / ".native/eccodes/Library/bin"
G0 = 9.80665


def configure_env() -> dict[str, str]:
    env = os.environ.copy()
    if NATIVE_BIN.is_dir():
        env["PATH"] = str(NATIVE_BIN) + os.pathsep + env.get("PATH", "")
    defs = ROOT / ".native/eccodes/Library/share/eccodes/definitions"
    if defs.is_dir():
        env["ECCODES_DEFINITION_PATH"] = str(defs)
    samples = ROOT / ".native/eccodes/Library/share/eccodes/samples"
    if samples.is_dir():
        env["ECCODES_SAMPLES_PATH"] = str(samples)
    return env


def run(cmd: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(cmd, capture_output=True, text=True, env=env, check=False)
    if result.returncode != 0:
        raise SystemExit(
            f"command failed ({result.returncode}): {' '.join(cmd)}\n{result.stderr}"
        )
    return result.stdout


def list_levels(path: Path, short_name: str, env: dict[str, str]) -> list[int]:
    text = run(
        [
            "grib_ls",
            "-p",
            "level",
            "-w",
            f"shortName={short_name},typeOfLevel=isobaricInhPa",
            str(path),
        ],
        env,
    )
    levels: set[int] = set()
    for line in text.splitlines():
        token = line.strip()
        # Accept only a pure integer token (reject "37 of 628 messages in ...").
        if not re.fullmatch(r"\d+", token):
            continue
        levels.add(int(token))
    ordered = sorted(levels, reverse=True)
    if not ordered:
        raise SystemExit(f"no isobaric levels for {short_name} in {path}\n{text}")
    return ordered


def message_time_unix(path: Path, env: dict[str, str]) -> int:
    text = run(
        [
            "grib_ls",
            "-p",
            "dataDate,dataTime",
            "-w",
            "shortName=u,typeOfLevel=isobaricInhPa,level=250",
            str(path),
        ],
        env,
    )
    date = None
    time = None
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0].isdigit() and len(parts[0]) == 8:
            date = int(parts[0])
            time = int(parts[1])
            break
    if date is None or time is None:
        raise SystemExit(f"cannot parse dataDate/dataTime from {path}\n{text}")
    year = date // 10000
    month = (date // 100) % 100
    day = date % 100
    hour = time // 100
    minute = time % 100
    return int(datetime(year, month, day, hour, minute, tzinfo=timezone.utc).timestamp())


def grid_axes(path: Path, env: dict[str, str], work: Path) -> tuple[np.ndarray, np.ndarray]:
    tmp = work / "axis.grib"
    run(
        [
            "grib_copy",
            "-w",
            "shortName=u,typeOfLevel=isobaricInhPa,level=250",
            str(path),
            str(tmp),
        ],
        env,
    )
    meta = run(
        [
            "grib_ls",
            "-p",
            "Ni,Nj,latitudeOfFirstGridPointInDegrees,longitudeOfFirstGridPointInDegrees,"
            "jDirectionIncrementInDegrees,iDirectionIncrementInDegrees",
            str(tmp),
        ],
        env,
    )
    ni = nj = None
    lat1 = lon1 = dlat = dlon = None
    for line in meta.splitlines():
        parts = line.split()
        if len(parts) >= 6 and parts[0].isdigit():
            ni = int(parts[0])
            nj = int(parts[1])
            lat1 = float(parts[2])
            lon1 = float(parts[3])
            dlat = float(parts[4])
            dlon = float(parts[5])
            break
    if None in (ni, nj, lat1, lon1, dlat, dlon):
        raise SystemExit(f"cannot parse grid axes\n{meta}")
    lats = lat1 - dlat * np.arange(nj, dtype=np.float64)
    lons = lon1 + dlon * np.arange(ni, dtype=np.float64)
    if not np.all(np.diff(lons) > 0):
        raise SystemExit("longitude not strictly increasing")
    if not np.all(np.diff(lats) < 0):
        raise SystemExit("latitude not strictly decreasing")
    return lats, lons


def read_field(
    path: Path,
    short_name: str,
    type_of_level: str,
    level: int | None,
    ny: int,
    nx: int,
    env: dict[str, str],
    work: Path,
    *,
    where_override: str | None = None,
    label: str | None = None,
) -> np.ndarray:
    where = where_override or f"shortName={short_name},typeOfLevel={type_of_level}"
    if where_override is None and level is not None:
        where += f",level={level}"
    tag = label or f"{short_name}_{type_of_level}_{level if level is not None else 'x'}"
    tmp = work / f"{tag}.grib"
    run(["grib_copy", "-w", where, str(path), str(tmp)], env)
    if not tmp.is_file() or tmp.stat().st_size == 0:
        raise SystemExit(f"grib_copy produced empty file for {where} from {path}")
    text = run(["grib_get_data", str(tmp)], env)
    values: list[float] = []
    for line in text.splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        try:
            values.append(float(parts[2]))
        except ValueError:
            continue
    arr = np.asarray(values, dtype=np.float64)
    if arr.size != ny * nx:
        raise SystemExit(
            f"{where}: expected {ny*nx} values, got {arr.size} from {path.name}"
        )
    return arr.reshape(ny, nx).astype(np.float32)


def read_hpbl(
    path: Path, ny: int, nx: int, env: dict[str, str], work: Path
) -> np.ndarray:
    """Official CFSR planetary boundary layer height (HPBL).

    NCEI pgbl encodes HPBL as GRIB2:
      centre=7, discipline=0, parameterCategory=3, parameterNumber=196, surface.
    shortName is often 'unknown' in ecCodes tables; do not rely on shortName=hpbl.
    """
    where = (
        "centre=7,discipline=0,parameterCategory=3,parameterNumber=196,"
        "typeOfLevel=surface"
    )
    return read_field(
        path,
        short_name="hpbl",
        type_of_level="surface",
        level=0,
        ny=ny,
        nx=nx,
        env=env,
        work=work,
        where_override=where,
        label="hpbl_official_c7_d0_c3_n196",
    )


def convert(files: list[Path], pressure_out: Path, surface_out: Path) -> None:
    import netCDF4  # type: ignore

    env = configure_env()
    for tool in ("grib_ls", "grib_copy", "grib_get_data"):
        if subprocess.run(["where" if os.name == "nt" else "which", tool], capture_output=True).returncode != 0:
            # fallback PATH already set; hard fail later on run()
            pass

    work = Path(tempfile.mkdtemp(prefix="cfsr_oracle_adapter_"))
    try:
        lats, lons = grid_axes(files[0], env, work)
        ny, nx = len(lats), len(lons)
        levels_hpa = list_levels(files[0], "u", env)
        # Require the same levels for v.
        v_levels = list_levels(files[0], "v", env)
        if v_levels != levels_hpa:
            raise SystemExit(f"u/v level mismatch u={levels_hpa} v={v_levels}")
        times = [message_time_unix(path, env) for path in files]
        if not all(times[i] < times[i + 1] for i in range(len(times) - 1)):
            raise SystemExit(f"times not increasing: {times}")

        nt, nlev = len(files), len(levels_hpa)
        u = np.zeros((nt, nlev, ny, nx), dtype=np.float32)
        v = np.zeros_like(u)
        t = np.zeros_like(u)
        q = np.zeros_like(u)
        w = np.zeros_like(u)
        z = np.zeros_like(u)
        sp = np.zeros((nt, ny, nx), dtype=np.float32)
        surface_z = np.zeros_like(sp)
        t2m = np.zeros_like(sp)
        d2m = np.zeros_like(sp)
        u10 = np.zeros_like(sp)
        v10 = np.zeros_like(sp)
        ishf = np.zeros_like(sp)
        fricv = np.full_like(sp, -1.0)
        fsr = np.zeros_like(sp)
        blh = np.full_like(sp, -1.0)

        for it, path in enumerate(files):
            for ilev, lev in enumerate(levels_hpa):
                u[it, ilev] = read_field(path, "u", "isobaricInhPa", lev, ny, nx, env, work)
                v[it, ilev] = read_field(path, "v", "isobaricInhPa", lev, ny, nx, env, work)
                t[it, ilev] = read_field(path, "t", "isobaricInhPa", lev, ny, nx, env, work)
                q[it, ilev] = read_field(path, "q", "isobaricInhPa", lev, ny, nx, env, work)
                w[it, ilev] = read_field(path, "w", "isobaricInhPa", lev, ny, nx, env, work)
                gh = read_field(path, "gh", "isobaricInhPa", lev, ny, nx, env, work)
                z[it, ilev] = gh * np.float32(G0)
            sp[it] = read_field(path, "sp", "surface", 0, ny, nx, env, work)
            orog = read_field(path, "orog", "surface", 0, ny, nx, env, work)
            surface_z[it] = orog * np.float32(G0)
            t2m[it] = read_field(path, "2t", "heightAboveGround", 2, ny, nx, env, work)
            d2m[it] = read_field(path, "2d", "heightAboveGround", 2, ny, nx, env, work)
            u10[it] = read_field(path, "10u", "heightAboveGround", 10, ny, nx, env, work)
            v10[it] = read_field(path, "10v", "heightAboveGround", 10, ny, nx, env, work)
            # Real near-surface/PBL inputs for FLEXPART NCEP calcpar path.
            ishf[it] = read_field(path, "ishf", "surface", 0, ny, nx, env, work)
            fricv[it] = read_field(path, "fricv", "surface", 0, ny, nx, env, work)
            blh[it] = read_hpbl(path, ny, nx, env, work)
            try:
                fsr[it] = read_field(path, "fsr", "surface", 0, ny, nx, env, work)
            except SystemExit:
                fsr[it] = 0.0
        if not np.all(np.isfinite(blh)) or np.any(blh < 0.0):
            raise SystemExit(
                f"official HPBL/blh invalid: finite={bool(np.all(np.isfinite(blh)))} "
                f"min={float(np.nanmin(blh))} max={float(np.nanmax(blh))}"
            )

        pressure_out.parent.mkdir(parents=True, exist_ok=True)
        surface_out.parent.mkdir(parents=True, exist_ok=True)

        with netCDF4.Dataset(pressure_out, "w", format="NETCDF4") as ds:
            ds.createDimension("valid_time", nt)
            ds.createDimension("pressure_level", nlev)
            ds.createDimension("latitude", ny)
            ds.createDimension("longitude", nx)
            ds.createVariable("valid_time", "i8", ("valid_time",))[:] = np.asarray(
                times, dtype=np.int64
            )
            ds.createVariable("pressure_level", "f8", ("pressure_level",))[:] = np.asarray(
                levels_hpa, dtype=np.float64
            )
            ds.createVariable("latitude", "f8", ("latitude",))[:] = lats
            ds.createVariable("longitude", "f8", ("longitude",))[:] = lons
            for name, arr in (("u", u), ("v", v), ("t", t), ("q", q), ("w", w), ("z", z)):
                ds.createVariable(
                    name, "f4", ("valid_time", "pressure_level", "latitude", "longitude")
                )[:] = arr
            ds.setncattr("title", "CFSR PGBl oracle adapter pressure fields")
            ds.setncattr("source", "native ecCodes CLI decode of official NCEI pgbl files")

        with netCDF4.Dataset(surface_out, "w", format="NETCDF4") as ds:
            ds.createDimension("valid_time", nt)
            ds.createDimension("latitude", ny)
            ds.createDimension("longitude", nx)
            ds.createVariable("valid_time", "i8", ("valid_time",))[:] = np.asarray(
                times, dtype=np.int64
            )
            ds.createVariable("latitude", "f8", ("latitude",))[:] = lats
            ds.createVariable("longitude", "f8", ("longitude",))[:] = lons
            for name, arr in (
                ("sp", sp),
                ("z", surface_z),
                ("t2m", t2m),
                ("d2m", d2m),
                ("u10", u10),
                ("v10", v10),
                ("ishf", ishf),
                ("fricv", fricv),
                ("zust", fricv),  # alias friction velocity for pressure-meter driver
                ("blh", blh),  # official HPBL (centre=7, disc=0, cat=3, num=196)
                ("fsr", fsr),
            ):
                ds.createVariable(name, "f4", ("valid_time", "latitude", "longitude"))[:] = arr
            ds.setncattr("title", "CFSR PGBl oracle adapter surface fields")
            ds.setncattr("orography_units", "geopotential m2 s-2 (orog*g0)")
            ds.setncattr(
                "pbl_mapping",
                "ishf->sshf; fricv/zust->sfcstress via u*^2*rho; "
                "official HPBL(c=7,d=0,cat=3,n=196)->blh->hmix (NCEP calcpar path)",
            )
            ds.setncattr(
                "hpbl_grib_keys",
                "centre=7,discipline=0,parameterCategory=3,parameterNumber=196,typeOfLevel=surface",
            )

        # Diagnostics at frozen sea/plain/mountain query cells (query SHA unchanged).
        import json

        query_path = ROOT / "target/m3-oracle/queries/cfsr_pressure_queries.json"
        query_doc = json.loads(query_path.read_text(encoding="utf-8"))
        sites = query_doc["sites"]
        orog_m = surface_z[0] / np.float32(G0)

        def nearest_ij(lat: float, lon: float) -> tuple[int, int]:
            j = int(np.argmin(np.abs(lats - lat)))
            i = int(np.argmin(np.abs(lons - lon)))
            return j, i

        def bilinear_sample(field_t0: np.ndarray, lat: float, lon: float, j0: int, i0: int, j1: int, i1: int) -> float:
            """Bilinear sample at (lat,lon) using frozen cell corners (j0,i0)-(j1,i1)."""
            lat0 = float(lats[j0]); lat1 = float(lats[j1])
            lon0 = float(lons[i0]); lon1 = float(lons[i1])
            if abs(lat1 - lat0) < 1e-12:
                ty = 0.0
            else:
                ty = (lat - lat0) / (lat1 - lat0)
            if abs(lon1 - lon0) < 1e-12:
                tx = 0.0
            else:
                tx = (lon - lon0) / (lon1 - lon0)
            # Clamp tiny numeric excursions outside [0,1] from float rounding.
            tx = float(min(1.0, max(0.0, tx)))
            ty = float(min(1.0, max(0.0, ty)))
            v00 = float(field_t0[j0, i0])
            v01 = float(field_t0[j0, i1])
            v10 = float(field_t0[j1, i0])
            v11 = float(field_t0[j1, i1])
            return (
                v00 * (1.0 - tx) * (1.0 - ty)
                + v01 * tx * (1.0 - ty)
                + v10 * (1.0 - tx) * ty
                + v11 * tx * ty
            )

        def cell_diag(site_class: str, site: dict) -> dict:
            # Frozen query SW corner (i,j) and geometric cell center.
            j0 = int(site["j"])
            i0 = int(site["i"])
            j1 = min(j0 + 1, ny - 1)
            i1 = min(i0 + 1, nx - 1)
            lat_c = float(site.get("cell_center_latitude_degrees", site["latitude_degrees"]))
            lon_c = float(site.get("cell_center_longitude_degrees", site["longitude_degrees"]))
            corners = {
                "j0_i0": {
                    "j": j0,
                    "i": i0,
                    "lat": float(lats[j0]),
                    "lon": float(lons[i0]),
                    "terrain_m": float(orog_m[j0, i0]),
                    "sp_pa_t0": float(sp[0, j0, i0]),
                    "blh_m_t0": float(blh[0, j0, i0]),
                    "ishf_t0": float(ishf[0, j0, i0]),
                    "fricv_t0": float(fricv[0, j0, i0]),
                },
                "j0_i1": {
                    "j": j0,
                    "i": i1,
                    "lat": float(lats[j0]),
                    "lon": float(lons[i1]),
                    "terrain_m": float(orog_m[j0, i1]),
                    "sp_pa_t0": float(sp[0, j0, i1]),
                    "blh_m_t0": float(blh[0, j0, i1]),
                    "ishf_t0": float(ishf[0, j0, i1]),
                    "fricv_t0": float(fricv[0, j0, i1]),
                },
                "j1_i0": {
                    "j": j1,
                    "i": i0,
                    "lat": float(lats[j1]),
                    "lon": float(lons[i0]),
                    "terrain_m": float(orog_m[j1, i0]),
                    "sp_pa_t0": float(sp[0, j1, i0]),
                    "blh_m_t0": float(blh[0, j1, i0]),
                    "ishf_t0": float(ishf[0, j1, i0]),
                    "fricv_t0": float(fricv[0, j1, i0]),
                },
                "j1_i1": {
                    "j": j1,
                    "i": i1,
                    "lat": float(lats[j1]),
                    "lon": float(lons[i1]),
                    "terrain_m": float(orog_m[j1, i1]),
                    "sp_pa_t0": float(sp[0, j1, i1]),
                    "blh_m_t0": float(blh[0, j1, i1]),
                    "ishf_t0": float(ishf[0, j1, i1]),
                    "fricv_t0": float(fricv[0, j1, i1]),
                },
            }
            # Nearest grid anchor (not the bilinear center).
            j_near = int(np.argmin(np.abs(lats - lat_c)))
            i_near = int(np.argmin(np.abs(lons - lon_c)))
            grid_anchor = {
                "j": j_near,
                "i": i_near,
                "lat": float(lats[j_near]),
                "lon": float(lons[i_near]),
                "terrain_m": float(orog_m[j_near, i_near]),
                "sp_pa_t0": float(sp[0, j_near, i_near]),
                "blh_m_t0": float(blh[0, j_near, i_near]),
                "ishf_t0": float(ishf[0, j_near, i_near]),
                "fricv_t0": float(fricv[0, j_near, i_near]),
            }
            bilinear_center = {
                "lat": lat_c,
                "lon": lon_c,
                "j0": j0,
                "i0": i0,
                "j1": j1,
                "i1": i1,
                "terrain_m": bilinear_sample(orog_m, lat_c, lon_c, j0, i0, j1, i1),
                "sp_pa_t0": bilinear_sample(sp[0], lat_c, lon_c, j0, i0, j1, i1),
                "blh_m_t0": bilinear_sample(blh[0], lat_c, lon_c, j0, i0, j1, i1),
                "ishf_t0": bilinear_sample(ishf[0], lat_c, lon_c, j0, i0, j1, i1),
                "fricv_t0": bilinear_sample(fricv[0], lat_c, lon_c, j0, i0, j1, i1),
            }
            under = []
            center_sp = float(bilinear_center["sp_pa_t0"])
            for lev in levels_hpa:
                p_pa = float(lev) * 100.0
                corner_sp = [
                    corners["j0_i0"]["sp_pa_t0"],
                    corners["j0_i1"]["sp_pa_t0"],
                    corners["j1_i0"]["sp_pa_t0"],
                    corners["j1_i1"]["sp_pa_t0"],
                ]
                under_n = sum(1 for s in corner_sp if s < p_pa)
                under.append(
                    {
                        "level_hpa": float(lev),
                        "underground_corner_count_t0": under_n,
                        "underground_corner_fraction_t0": under_n / 4.0,
                        "center_underground": bool(center_sp < p_pa),
                        "center_sp_pa_t0": center_sp,
                    }
                )
            return {
                "site_class": site_class,
                "query_site": site,
                "grid_anchor": grid_anchor,
                "bilinear_center": bilinear_center,
                "corners": corners,
                "underground_by_level": under,
            }

        site_diags = {
            name: cell_diag(name, site) for name, site in sorted(sites.items())
        }
        # Explicit mountain emphasis required by A.
        mountain = site_diags["mountain"]
        diag = {
            "query_path": str(query_path.as_posix()),
            "query_sha256": __import__("hashlib")
            .sha256(query_path.read_bytes())
            .hexdigest(),
            "grid": {
                "ny": ny,
                "nx": nx,
                "lat0": float(lats[0]),
                "lon0": float(lons[0]),
                "dlat": float(lats[1] - lats[0]) if ny > 1 else None,
                "dlon": float(lons[1] - lons[0]) if nx > 1 else None,
            },
            "field_presence": {
                "ishf": True,
                "fricv": True,
                "fsr": bool(np.any(np.isfinite(fsr))),
                "blh": True,
                "hpbl_keys": "centre=7,discipline=0,parameterCategory=3,parameterNumber=196",
            },
            "sites": site_diags,
            "mountain_corners": mountain["corners"],
            "mountain_grid_anchor": mountain["grid_anchor"],
            "mountain_bilinear_center": mountain["bilinear_center"],
            "mountain_surface_pressure_pa_t0": mountain["bilinear_center"]["sp_pa_t0"],
            "mountain_underground_by_level": mountain["underground_by_level"],
            "note": (
                "query cells frozen; diagnostics only — no query SHA rewrite; "
                "mountain_bilinear_center is true cell-center bilinear, "
                "mountain_grid_anchor is nearest grid point"
            ),
        }
        diag_path = surface_out.with_name(surface_out.stem + "_diagnostics.json")
        diag_path.write_text(
            json.dumps(diag, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )

        print(
            {
                "pressure": str(pressure_out),
                "surface": str(surface_out),
                "diagnostics": str(diag_path),
                "times": times,
                "levels_hpa_head": levels_hpa[:5],
                "levels_hpa_tail": levels_hpa[-5:],
                "nlev": nlev,
                "ny": ny,
                "nx": nx,
            }
        )
    finally:
        # Best-effort cleanup of temp grib slices.
        for path in work.glob("*"):
            try:
                path.unlink()
            except OSError:
                pass
        try:
            work.rmdir()
        except OSError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--grib", action="append", type=Path, required=True)
    parser.add_argument("--pressure-out", type=Path, required=True)
    parser.add_argument("--surface-out", type=Path, required=True)
    args = parser.parse_args()
    convert(args.grib, args.pressure_out, args.surface_out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
