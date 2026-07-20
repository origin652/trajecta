#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Generate M3 oracle query matrices (A contract §6).

Canonical formal queries are published under target/m3-oracle/queries/ ONLY when
all three families succeed terrain selection. Before every run the canonical
directory is wiped. Partial successes never leave stale INDEX/query bytes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target" / "m3-oracle" / "queries"
SKELETON_OUT = ROOT / "target" / "m3-oracle" / "queries-skeleton"
G0 = 9.80665
FLEXPART_COMMIT = "dace3affa2ba71677f12f3858b04aaf59f8ee51e"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def relposix(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def write_bytes(path: Path, data: bytes) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    on_disk = path.read_bytes()
    if on_disk != data:
        raise SystemExit(f"write drift for {path}")
    return sha256_bytes(on_disk)


def wipe_dir(path: Path) -> None:
    if path.is_dir():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def parse_units_to_meters_factor(units: str) -> float:
    """H = Φ/g0 when units are geopotential; identity when already metres."""
    raw = (units or "").strip().lower()
    u = raw.replace("**", "^").replace(" ", "").replace("·", "")
    if (
        any(
            tok in u
            for tok in (
                "m2s-2",
                "m^2s-2",
                "m2/s2",
                "m^2/s^2",
                "m2s^-2",
                "m^2s^-2",
            )
        )
        or ("m^2" in u and "s" in u)
        or ("m2" in u and "s" in u)
        or "geopotential" in raw
    ):
        return 1.0 / G0
    if u in {"m", "meter", "meters", "metre", "metres"}:
        return 1.0
    raise SystemExit(f"unrecognized terrain units: {units!r}")


def try_load_terrain_netcdf(nc_path: Path):
    if not nc_path.is_file() or nc_path.suffix.lower() != ".nc":
        return None
    try:
        import netCDF4  # type: ignore
        import numpy as np
    except ImportError as e:
        raise SystemExit(f"INCOMPLETE: netCDF4+numpy required for terrain: {e}") from e

    ds = netCDF4.Dataset(nc_path.as_posix())
    try:
        lon_name = next(n for n in ("longitude", "lon", "x") if n in ds.variables)
        lat_name = next(n for n in ("latitude", "lat", "y") if n in ds.variables)
        lons = [float(v) for v in ds.variables[lon_name][:]]
        lats = [float(v) for v in ds.variables[lat_name][:]]
        zvar = None
        for cand in ("z", "orog", "zs", "geopotential"):
            if cand in ds.variables:
                zvar = ds.variables[cand]
                break
        if zvar is None:
            return None
        units = ""
        for attr in ("units", "GRIB_units"):
            if attr in zvar.ncattrs():
                units = str(zvar.getncattr(attr))
                break
        factor = parse_units_to_meters_factor(units)
        arr = np.array(zvar[:], dtype=float)
        while arr.ndim > 2:
            arr = arr[0]
        if arr.ndim != 2:
            return None
        z_m = (arr * factor).tolist()
        return {
            "lons": lons,
            "lats": lats,
            "z_m": z_m,
            "units_raw": units,
            "meters_scale_factor": factor,
            "source": relposix(nc_path),
            "kind": "netcdf_surface_geopotential",
        }
    finally:
        ds.close()


def _eccodes_bin(name: str) -> str | None:
    import os
    import shutil

    if os.name == "nt":
        local = ROOT / ".native/eccodes/Library/bin" / name
        for cand in (local, Path(str(local) + ".exe")):
            if cand.is_file():
                return str(cand)
    return shutil.which(name)


def try_load_terrain_cfsr_grib(paths: list[Path]):
    """Decode GRIB 0.3.5 surface orography (shortName=orog) via ecCodes CLI.

    Verifies 00/06/12 terrain arrays are exactly identical (metres).
    """
    if not paths or any(not p.is_file() for p in paths):
        return None
    try:
        import numpy as np
    except ImportError as e:
        raise SystemExit(f"INCOMPLETE: numpy required for CFSR terrain: {e}") from e

    grib_ls = _eccodes_bin("grib_ls")
    grib_get_data = _eccodes_bin("grib_get_data")
    if not grib_ls or not grib_get_data:
        return None

    import os
    import subprocess

    env = os.environ.copy()
    defs = ROOT / ".native/eccodes/Library/share/eccodes/definitions"
    if os.name == "nt" and defs.is_dir():
        env["ECCODES_DEFINITION_PATH"] = str(defs)

    identity_filter = (
        "discipline=0,parameterCategory=3,parameterNumber=5,"
        "typeOfFirstFixedSurface=1"
    )

    decoded = []
    for path in paths:
        meta_p = subprocess.run(
            [
                grib_ls,
                "-w",
                identity_filter,
                "-p",
                "shortName,units,Ni,Nj,"
                "latitudeOfFirstGridPointInDegrees,longitudeOfFirstGridPointInDegrees,"
                "latitudeOfLastGridPointInDegrees,longitudeOfLastGridPointInDegrees,"
                "jScansPositively,discipline,parameterCategory,parameterNumber",
                str(path),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
            check=False,
        )
        if meta_p.returncode != 0:
            raise SystemExit(f"CFSR orog/0.3.5 surface not found in {path}")
        # parse last data row
        lines = [ln for ln in (meta_p.stdout or "").splitlines() if ln.strip()]
        data_line = None
        for ln in lines:
            parts = ln.split()
            if len(parts) < 12:
                continue
            try:
                identity = tuple(int(value) for value in parts[-3:])
            except ValueError:
                continue
            if identity == (0, 3, 5):
                data_line = parts
                break
        if not data_line or len(data_line) < 10:
            raise SystemExit(f"failed to parse orog metadata for {path}: {lines[:5]}")
        # shortName units Ni Nj lat1 lon1 lat2 lon2 jScan disc cat num
        units = data_line[1]
        ni = int(data_line[2])
        nj = int(data_line[3])
        lat1 = float(data_line[4])
        lon1 = float(data_line[5])
        lat2 = float(data_line[6])
        lon2 = float(data_line[7])
        di = (lon2 - lon1) / max(ni - 1, 1)
        dj = (lat2 - lat1) / max(nj - 1, 1)
        lons = [lon1 + i * di for i in range(ni)]
        lats = [lat1 + j * dj for j in range(nj)]

        data_p = subprocess.run(
            [grib_get_data, "-w", identity_filter, str(path)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
            check=False,
        )
        if data_p.returncode != 0:
            raise SystemExit(f"grib_get_data failed for {path}")
        vals = []
        for ln in (data_p.stdout or "").splitlines():
            parts = ln.split()
            if len(parts) != 3:
                continue
            try:
                vals.append(float(parts[2]))
            except ValueError:
                continue
        if len(vals) != ni * nj:
            raise SystemExit(
                f"orog value count {len(vals)} != Ni*Nj {ni*nj} for {path}"
            )
        factor = parse_units_to_meters_factor(units)
        arr = np.array(vals, dtype=float).reshape((nj, ni)) * factor
        decoded.append(
            {
                "lons": lons,
                "lats": lats,
                "z_m": arr.tolist(),
                "units_raw": units,
                "meters_scale_factor": factor,
                "source": relposix(path),
                "kind": "grib_0.3.5_surface_orog_cli",
                "flat": arr.reshape(-1).copy(),
                "identity": "0.3.5:surface",
            }
        )

    ref = decoded[0]["flat"]
    for d in decoded[1:]:
        if not np.array_equal(d["flat"], ref):
            raise SystemExit(
                "CFSR terrain 0.3.5:surface/orog differs across 00/06/12 — hand to A"
            )
    base = decoded[0]
    base["cross_time_identical"] = True
    base["files"] = [relposix(p) for p in paths]
    del base["flat"]
    return base


def half_step_center(coords: list[float], idx: int) -> float:
    n = len(coords)
    if n < 2:
        return coords[idx]
    if idx < n - 1:
        return 0.5 * (coords[idx] + coords[idx + 1])
    return 0.5 * (coords[idx] + coords[idx - 1])


def select_sites(lons: list[float], lats: list[float], z: list[list[float]]) -> dict[str, dict]:
    """Select terrain classes inside Trajecta's one-cell safe interpolation halo.

    H = Φ/g0 is already applied and mountain must be >= 500 m.  Native anchors
    use the selected grid node; interpolated records use the cell immediately
    toward increasing array indices.  A node therefore needs both the leading
    halo and one complete cell before the trailing halo.
    """
    ny = len(lats)
    nx = len(lons)
    halo = 1
    if nx < 2 * halo + 2 or ny < 2 * halo + 2:
        raise SystemExit(
            f"terrain grid {nx}x{ny} is too small for {halo}-cell safe halo"
        )
    sea = plain = mountain = None
    mountain_z = -1e300
    for j in range(halo, ny - halo - 1):
        for i in range(halo, nx - halo - 1):
            h = float(z[j][i])
            if not math.isfinite(h):
                continue
            idx = j * nx + i
            if abs(h) <= 10.0 and (sea is None or idx < sea["linear_index"]):
                sea = _site("sea", lons, lats, i, j, h, idx)
            if 50.0 <= h <= 300.0 and (plain is None or idx < plain["linear_index"]):
                plain = _site("plain", lons, lats, i, j, h, idx)
            if h > mountain_z or (
                math.isclose(h, mountain_z)
                and (mountain is None or idx < mountain["linear_index"])
            ):
                mountain_z = h
                mountain = _site("mountain", lons, lats, i, j, h, idx)
    if sea is None or plain is None or mountain is None:
        raise SystemExit(
            "terrain site selection failed: missing sea/plain/mountain — hand to A"
        )
    if mountain["terrain_m"] < 500.0:
        raise SystemExit(
            f"mountain terrain {mountain['terrain_m']:.4f} m < 500 m; hand to A "
            "(do not lower threshold)"
        )
    for site in (sea, plain, mountain):
        site["safe_interpolation_halo_cells"] = halo
    return {"sea": sea, "plain": plain, "mountain": mountain}


def _site(cls, lons, lats, i, j, h, idx):
    return {
        "site_class": cls,
        "longitude_degrees": lons[i],
        "latitude_degrees": lats[j],
        "terrain_m": h,
        "linear_index": idx,
        "i": i,
        "j": j,
        "cell_center_longitude_degrees": half_step_center(lons, i),
        "cell_center_latitude_degrees": half_step_center(lats, j),
        "grid_dlon_degrees": abs(lons[1] - lons[0]) if len(lons) > 1 else None,
        "grid_dlat_degrees": abs(lats[1] - lats[0]) if len(lats) > 1 else None,
    }


def hybrid_levels_1based_from_top(nc_path: Path) -> list[int] | None:
    if not nc_path.is_file():
        return None
    try:
        import netCDF4  # type: ignore
    except ImportError:
        return None
    ds = netCDF4.Dataset(nc_path.as_posix())
    try:
        n = None
        for name in ("hybrid", "level", "lev", "nhym", "nlev"):
            if name in ds.dimensions:
                n = len(ds.dimensions[name])
                break
        if not n or n < 3:
            return None
        z0 = sorted(
            {
                max(0, min(n - 1, int(round(0.20 * (n - 1))))),
                max(0, min(n - 1, int(round(0.50 * (n - 1))))),
                max(0, min(n - 1, int(round(0.80 * (n - 1))))),
            }
        )
        while len(z0) < 3:
            z0.append(min(n - 1, z0[-1] + 1))
        return [i + 1 for i in z0[:3]]
    finally:
        ds.close()


def build_records(*, times, mid, levels, sites, level_kind) -> list[dict]:
    rows: list[dict] = []
    for t in times:
        for site_name, site in sites.items():
            for level in levels:
                rows.append(
                    {
                        "point_id": f"native:{site_name}:lev{level}:t{t}",
                        "sample_scope": "native_anchor",
                        "time_unix": t,
                        "longitude_degrees": site["longitude_degrees"],
                        "latitude_degrees": site["latitude_degrees"],
                        "vertical": level,
                        "vertical_coordinate": level_kind,
                        "site_class": site_name,
                        "grid_node": {
                            "i": site["i"],
                            "j": site["j"],
                            "linear_index": site["linear_index"],
                            "terrain_m": site["terrain_m"],
                        },
                    }
                )
    coords = [
        ("above_sea_level", 5000.0),
        ("above_ground", 1000.0),
        ("pressure", 50000.0),
    ]
    for t in mid:
        for site_name, site in sites.items():
            for coord, vert in coords:
                rows.append(
                    {
                        "point_id": f"interp:{site_name}:{coord}:t{t}",
                        "sample_scope": "interpolated_common",
                        "time_unix": t,
                        "longitude_degrees": site["cell_center_longitude_degrees"],
                        "latitude_degrees": site["cell_center_latitude_degrees"],
                        "vertical": vert,
                        "vertical_coordinate": coord,
                        "site_class": site_name,
                    }
                )
    for t in mid:
        for site_name, site in sites.items():
            for agl in (10.0, 50.0):
                rows.append(
                    {
                        "point_id": f"surface:{site_name}:agl{agl}:t{t}",
                        "sample_scope": "surface_layer",
                        "time_unix": t,
                        "longitude_degrees": site["longitude_degrees"],
                        "latitude_degrees": site["latitude_degrees"],
                        "vertical": agl,
                        "vertical_coordinate": "above_ground",
                        "site_class": site_name,
                    }
                )
            for kind in ("geometric_w", "density", "geometric_terrain"):
                rows.append(
                    {
                        "point_id": f"modern:{site_name}:{kind}:t{t}",
                        "sample_scope": "modern_difference",
                        "time_unix": t,
                        "longitude_degrees": site["longitude_degrees"],
                        "latitude_degrees": site["latitude_degrees"],
                        "vertical": 50.0,
                        "vertical_coordinate": "above_ground",
                        "site_class": site_name,
                        "modern_kind": kind,
                    }
                )
    rows.sort(key=lambda r: r["point_id"])
    return rows


def pack_doc(family, records, status, note, sites, extra=None) -> dict:
    body = {
        "schema_hint": "trajecta.m3.oracle_query/v1",
        "dataset_family": family,
        "flexpart_commit": FLEXPART_COMMIT,
        "default_real_bits": 32,
        "status": status,
        "note": note,
        "sites": sites,
        "height_definition": "geopotential_height_m = Phi / g0 with g0=9.80665",
        "counts": {
            "native_anchor": sum(1 for r in records if r["sample_scope"] == "native_anchor"),
            "interpolated_common": sum(
                1 for r in records if r["sample_scope"] == "interpolated_common"
            ),
            "surface_layer": sum(1 for r in records if r["sample_scope"] == "surface_layer"),
            "modern_difference": sum(
                1 for r in records if r["sample_scope"] == "modern_difference"
            ),
            "total": len(records),
        },
        "records": records,
    }
    if extra:
        body.update(extra)
    return body


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--skeleton-ok",
        action="store_true",
        help="write skeleton under queries-skeleton/ for families that cannot formalize",
    )
    args = parser.parse_args()

    # Always wipe canonical formal dir first so stale INDEX/queries cannot survive.
    wipe_dir(OUT)
    print("wiped canonical formal dir", relposix(OUT))

    families = {
        "era5_pressure": {
            "times": [1543622400, 1543644000, 1543665600],
            "mid": [1543633200, 1543654800],
            "levels": [25000.0, 50000.0, 85000.0],
            "level_kind": "pressure_pa",
            "surface": ROOT
            / "target/test-data/era5-cds-pressure-official/ready/era5_surface_20181201.nc",
            "main": ROOT
            / "target/test-data/era5-cds-pressure-official/ready/era5_pressure_20181201.nc",
        },
        "era5_hybrid": {
            "times": [1543622400, 1543633200, 1543644000],
            "mid": [1543627800, 1543638600],
            "levels": [28, 69, 110],
            "level_kind": "native_level",
            "surface": ROOT
            / "target/test-data/era5-cds-hybrid137-official/ready/era5_surface_20181201.nc",
            "main": ROOT
            / "target/test-data/era5-cds-hybrid137-official/ready/era5_hybrid137_prepared_20181201.nc",
        },
        "cfsr_pressure": {
            "times": [1230768000, 1230789600, 1230811200],
            "mid": [1230778800, 1230800400],
            "levels": [25000.0, 50000.0, 85000.0],
            "level_kind": "pressure_pa",
            "grib_files": [
                ROOT
                / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010100.grb2",
                ROOT
                / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010106.grb2",
                ROOT
                / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010112.grb2",
            ],
        },
    }

    formal_docs: dict[str, tuple[dict, Path]] = {}
    failures: list[str] = []
    skeleton_items = []

    for family, cfg in families.items():
        levels = list(cfg["levels"])
        if family == "era5_hybrid":
            hl = hybrid_levels_1based_from_top(cfg["main"])
            if hl is not None:
                levels = hl

        terrain = None
        try:
            if family == "cfsr_pressure":
                terrain = try_load_terrain_cfsr_grib(cfg["grib_files"])
            else:
                terrain = try_load_terrain_netcdf(cfg["surface"])
        except SystemExit as e:
            failures.append(f"{family}: {e}")
            terrain = None

        if terrain is None:
            failures.append(f"{family}: terrain unavailable")
            if args.skeleton_ok:
                sites = {
                    "sea": _site("sea", [2.0, 3.0], [50.0, 49.0], 0, 0, 0.0, 0),
                    "plain": _site("plain", [2.0, 3.0], [50.0, 49.0], 1, 0, 120.0, 1),
                    "mountain": _site("mountain", [2.0, 3.0, 7.0], [50.0, 49.0, 46.0], 2, 2, 2500.0, 8),
                }
                # fix mountain coords
                sites["mountain"]["longitude_degrees"] = 7.0
                sites["mountain"]["latitude_degrees"] = 46.0
                records = build_records(
                    times=cfg["times"],
                    mid=cfg["mid"],
                    levels=levels,
                    sites=sites,
                    level_kind=cfg["level_kind"],
                )
                doc = pack_doc(
                    family,
                    records,
                    "skeleton_sites_not_terrain_selected",
                    "Skeleton only; never published as formal canonical.",
                    sites,
                )
                skeleton_items.append((family, doc, levels))
            continue

        try:
            sites = select_sites(terrain["lons"], terrain["lats"], terrain["z_m"])
        except SystemExit as e:
            failures.append(f"{family}: {e}")
            continue

        records = build_records(
            times=cfg["times"],
            mid=cfg["mid"],
            levels=levels,
            sites=sites,
            level_kind=cfg["level_kind"],
        )
        if len(records) < 75:
            failures.append(f"{family}: only {len(records)} records < 75")
            continue
        extra = {
            "terrain_source": {
                k: terrain[k]
                for k in (
                    "source",
                    "units_raw",
                    "meters_scale_factor",
                    "kind",
                    "cross_time_identical",
                    "files",
                )
                if k in terrain
            },
            "levels": levels,
            "cds_area_nswe_note": "[53,0,45,10] expanded box when anchors refreshed",
        }
        doc = pack_doc(
            family,
            records,
            "frozen_terrain_selected",
            "Sites selected from same-grid surface geopotential; H=Phi/g0.",
            sites,
            extra=extra,
        )
        formal_docs[family] = (doc, levels)

    # Skeleton path (optional, never touches canonical)
    if skeleton_items:
        wipe_dir(SKELETON_OUT)
        sk_index = []
        for family, doc, levels in skeleton_items:
            raw = (json.dumps(doc, indent=2, sort_keys=True) + "\n").encode("utf-8")
            path = SKELETON_OUT / f"{family}_queries.json"
            digest = write_bytes(path, raw)
            sk_index.append(
                {
                    "family": family,
                    "path": relposix(path),
                    "size": path.stat().st_size,
                    "sha256": digest,
                    "total_records": doc["counts"]["total"],
                    "status": doc["status"],
                    "levels": levels,
                }
            )
            print("skeleton", relposix(path), digest[:16])
        idx_raw = (json.dumps(sk_index, indent=2, sort_keys=True) + "\n").encode("utf-8")
        idx_path = SKELETON_OUT / "INDEX.json"
        idx_sha = write_bytes(idx_path, idx_raw)
        write_bytes(SKELETON_OUT / "INDEX.json.sha256", (idx_sha + "\n").encode("utf-8"))

    required = ("era5_pressure", "era5_hybrid", "cfsr_pressure")
    if any(f not in formal_docs for f in required):
        print("FORMAL_FAILURES:", file=sys.stderr)
        for f in failures:
            print(" -", f, file=sys.stderr)
        missing = [f for f in required if f not in formal_docs]
        print(
            f"INCOMPLETE: formal {len(formal_docs)}/3; missing={missing}. "
            "Canonical queries/ left empty (wiped). No INDEX published.",
            file=sys.stderr,
        )
        # Ensure still empty
        wipe_dir(OUT)
        return 2

    # Publish all three atomically into canonical dir (already wiped).
    index = []
    for family in required:
        doc, levels = formal_docs[family]
        raw = (json.dumps(doc, indent=2, sort_keys=True) + "\n").encode("utf-8")
        path = OUT / f"{family}_queries.json"
        digest = write_bytes(path, raw)
        if sha256_bytes(path.read_bytes()) != digest:
            raise SystemExit(f"SHA self-check failed {path}")
        index.append(
            {
                "family": family,
                "path": relposix(path),
                "size": path.stat().st_size,
                "sha256": digest,
                "total_records": doc["counts"]["total"],
                "status": doc["status"],
                "levels": levels,
            }
        )
        print("formal", relposix(path), "n=", doc["counts"]["total"], digest[:16])

    for item in index:
        p = ROOT / item["path"]
        b = p.read_bytes()
        if len(b) != item["size"] or sha256_bytes(b) != item["sha256"]:
            raise SystemExit(f"INDEX self-check failed for {p}")
    idx_raw = (json.dumps(index, indent=2, sort_keys=True) + "\n").encode("utf-8")
    idx_path = OUT / "INDEX.json"
    idx_sha = write_bytes(idx_path, idx_raw)
    write_bytes(OUT / "INDEX.json.sha256", (idx_sha + "\n").encode("utf-8"))
    print("INDEX ok", relposix(idx_path), idx_sha[:16], "formal 3/3")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
