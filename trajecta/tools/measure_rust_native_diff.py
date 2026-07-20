#!/usr/bin/env python3
"""Measure pure-Rust dual-load and native backends (no self-adjudication).

Uses the Windows native environment from docs/TRAJECTA_NATIVE_NETCDF_WINDOWS.md when present.
Writes target/native-diff/*.json with per-variable exact mismatch / max abs / max rel / worst index.

Never claims tolerance pass. adjudication always unvalidated_measurement.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target" / "native-diff"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(name: str, payload: dict) -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / name
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print("wrote", path)
    return path


def native_env(*, with_eccodes: bool = False) -> dict[str, str]:
    """Build a native-backend environment for the current platform.

    Windows uses the pinned MSYS2/ecCodes layout documented by Trajecta.
    Linux keeps pkg-config discovery from the calling gate and only removes a
    known-bogus empty/current-directory LIBCLANG_PATH.
    """
    env = os.environ.copy()
    if os.name != "nt":
        raw = (env.get("LIBCLANG_PATH") or "").strip()
        if not raw or raw in {".", "./"}:
            env.pop("LIBCLANG_PATH", None)
        return env

    ucrt = Path("D:/msys64/ucrt64")
    path_parts: list[str] = []
    if ucrt.is_dir():
        path_parts.append(str(ucrt / "bin"))
        env["PKG_CONFIG_PATH"] = str(ucrt / "lib" / "pkgconfig")
        env["NETCDF_DIR"] = str(ucrt)
        env["LIBRARY_PATH"] = str(ucrt / "lib")
        env["CPATH"] = str(ucrt / "include")
    eccodes = ROOT / ".native" / "eccodes" / "Library"
    if with_eccodes and eccodes.is_dir():
        path_parts.insert(0, str(eccodes / "bin"))
        env["ECCODES_DEFINITION_PATH"] = str(
            eccodes / "share" / "eccodes" / "definitions"
        )
        # Append eccodes pkgconfig AFTER netcdf, never before.
        pkg = eccodes / "lib" / "pkgconfig"
        if pkg.is_dir():
            env["PKG_CONFIG_PATH"] = (
                env.get("PKG_CONFIG_PATH", "") + os.pathsep + str(pkg)
                if env.get("PKG_CONFIG_PATH")
                else str(pkg)
            )
        # Do not set CPATH to UCRT when bindgen+eccodes may need a single clang.
        env.pop("CPATH", None)
    if path_parts:
        env["PATH"] = os.pathsep.join(path_parts) + os.pathsep + env.get("PATH", "")
    # Optional libclang for eccodes-sys rebuilds.
    # Never seed candidates with Path("") — on Windows that resolves to cwd (".").
    if with_eccodes:
        candidates: list[Path] = []
        raw = (os.environ.get("LIBCLANG_PATH") or "").strip().strip('"')
        if raw:
            cand = Path(raw)
            # Accept either the directory containing the DLL, or the DLL path itself.
            if cand.is_file():
                candidates.append(cand.parent)
            elif cand.is_dir():
                candidates.append(cand)
        candidates.extend(
            [
                Path(r"C:/Program Files/LLVM/bin"),
                Path(
                    r"C:/Users/dell/miniforge3/pkgs/libclang-22.1.8-default_h570ddc7_3/Library/bin"
                ),
            ]
        )
        chosen: Path | None = None
        for cand in candidates:
            if not cand.is_dir():
                continue
            if (cand / "libclang.dll").is_file() or (cand / "clang.dll").is_file():
                chosen = cand
                break
        if chosen is not None:
            env["LIBCLANG_PATH"] = str(chosen)
        else:
            # Explicitly drop a bogus inherited value (e.g. empty -> ".") so bindgen
            # does not silently point at the repo cwd.
            env.pop("LIBCLANG_PATH", None)
            print(
                "WARNING: no libclang.dll/clang.dll found for native-eccodes; "
                "set LIBCLANG_PATH to the directory containing libclang.dll",
                flush=True,
            )
    else:
        # NetCDF-only path must not inherit a broken LIBCLANG_PATH.
        raw = (os.environ.get("LIBCLANG_PATH") or "").strip()
        if not raw or raw in {".", "./"}:
            env.pop("LIBCLANG_PATH", None)
    return env


def run(cmd: list[str], env: dict | None = None) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=ROOT, text=True, capture_output=True, env=env, check=False)


def probe_features(env: dict) -> dict:
    results = {}
    for feature in ("native-netcdf", "native-eccodes"):
        proc = run(
            ["cargo", "check", "--offline", "-p", "trajecta-met", "--features", feature],
            env=env,
        )
        results[feature] = {
            "exit_code": proc.returncode,
            "available": proc.returncode == 0,
            "stderr_tail": "\n".join(proc.stderr.splitlines()[-40:]),
        }
    status = "measured" if any(v["available"] for v in results.values()) else "external_blocked"
    payload = {
        "status": status,
        "kind": "native_feature_probe",
        "features": results,
        "env_loaded": {
            "NETCDF_DIR": env.get("NETCDF_DIR"),
            "ECCODES_DEFINITION_PATH": env.get("ECCODES_DEFINITION_PATH"),
            "LIBCLANG_PATH": env.get("LIBCLANG_PATH"),
        },
        "adjudication": "unvalidated_measurement",
        "notes": [
            "Loads docs/TRAJECTA_NATIVE_NETCDF_WINDOWS.md paths when present.",
            "Does not self-certify numerical tolerances.",
        ],
    }
    write_json("native_feature_probe.json", payload)
    return payload


def pure_rust_dual_load() -> dict:
    cfsr = ROOT / "target/test-data/cfsr-ncei-pgbl-official"
    files = [
        "pgbl00.gdas.2009010100.grb2",
        "pgbl00.gdas.2009010106.grb2",
        "pgbl00.gdas.2009010112.grb2",
    ]
    if not all((cfsr / n).is_file() for n in files):
        payload = {"status": "fixture_missing", "kind": "pure_rust_dual_load"}
        write_json("pure_rust_dual_load_cfsr.json", payload)
        return payload
    proc = run(
        [
            "cargo",
            "test",
            "--offline",
            "-p",
            "trajecta-met",
            "--test",
            "real_m3_query_chain",
            "cfsr_pgbl_pure_rust_dual_load_full_field_diff",
            "--",
            "--nocapture",
        ]
    )
    payload = {
        "status": "measured" if proc.returncode == 0 else "measured_failed",
        "kind": "pure_rust_dual_load",
        "exit_code": proc.returncode,
        "files": [
            {"name": n, "size": (cfsr / n).stat().st_size, "sha256": sha256_file(cfsr / n)}
            for n in files
        ],
        "result": "bitwise_identical_full_fields" if proc.returncode == 0 else "test_failed",
        "adjudication": "unvalidated_measurement",
        "stdout_tail": "\n".join(proc.stdout.splitlines()[-30:]),
        "stderr_tail": "\n".join(proc.stderr.splitlines()[-30:]),
    }
    write_json("pure_rust_dual_load_cfsr.json", payload)
    return payload


def run_example_measure(env: dict, feature: str) -> dict:
    """Build & run a small Rust one-shot if available; else cargo test native filters."""
    # Prefer running existing native tests under feature; parse unvalidated_measurement lines.
    if feature == "native-netcdf":
        filters = [
            ("real_netcdf_pipeline", "rust_native_field_diff"),
            ("real_netcdf_pipeline", "era5"),
        ]
    else:
        filters = [("real_cfsr_pipeline", "native_and_rust_cfsr")]
    results = []
    for test_pkg, filt in filters:
        proc = run(
            [
                "cargo",
                "test",
                "--offline",
                "-p",
                "trajecta-met",
                "--features",
                feature,
                "--test",
                test_pkg,
                filt,
                "--",
                "--nocapture",
            ],
            env=env,
        )
        measures = [
            line.strip()
            for line in (proc.stdout + "\n" + proc.stderr).splitlines()
            if "unvalidated_measurement" in line or "max_abs" in line or "max_difference" in line
        ]
        results.append(
            {
                "test": test_pkg,
                "filter": filt,
                "exit_code": proc.returncode,
                "measurement_lines": measures[-50:],
                "stdout_tail": "\n".join(proc.stdout.splitlines()[-40:]),
                "stderr_tail": "\n".join(proc.stderr.splitlines()[-40:]),
            }
        )
    payload = {
        "status": "measured" if any(r["exit_code"] == 0 for r in results) else "failed",
        "kind": f"native_{feature}_tests",
        "feature": feature,
        "results": results,
        "adjudication": "unvalidated_measurement",
        "notes": [
            "Legacy absolute/relative thresholds are not A-certified.",
            "Parse measurement_lines for max_abs / mismatch evidence.",
        ],
    }
    write_json(f"native_{feature}_tests.json", payload)
    return payload



CANONICAL_STRUCTURED_OUTPUTS = [
    "native_netcdf_era5_pressure_measurements.json",
    "native_netcdf_era5_surface_measurements.json",
    "native_netcdf_era5_hybrid_measurements.json",
    "native_netcdf_era5_hybrid_surface_measurements.json",
    "native_eccodes_cfsr_pgbl_measurements.json",
    "native_netcdf_structured_skipped.json",
    "native_eccodes_structured_skipped.json",
]


def clear_stale_structured_outputs() -> None:
    """Remove prior canonical structured results so skipped runs cannot keep old successes."""
    OUT.mkdir(parents=True, exist_ok=True)
    removed = []
    for name in CANONICAL_STRUCTURED_OUTPUTS:
        path = OUT / name
        if path.is_file():
            path.unlink()
            removed.append(name)
        meta = OUT / (name + ".runmeta.json")
        if meta.is_file():
            meta.unlink()
            removed.append(meta.name)
    if removed:
        print("cleared stale structured outputs:", ", ".join(removed), flush=True)


def structured_measurements(env_netcdf: dict, env_eccodes: dict, *, netcdf_ok: bool, eccodes_ok: bool) -> dict:
    """Emit structured mismatch JSON for ERA5 pressure/hybrid/near-surface + CFSR GRIB.

    Returns a small status dict used by SUMMARY.json (never claims complete if required
    backends were unavailable or produced no fresh artifact).
    """
    clear_stale_structured_outputs()
    netcdf_cases = [
        (
            ROOT / "target/test-data/era5-cds-pressure-official/ready/era5_pressure_20181201.nc",
            "t,u,v,q,w,z",
            "native_netcdf_era5_pressure_measurements.json",
            "netcdf",
        ),
        (
            ROOT / "target/test-data/era5-cds-pressure-official/ready/era5_surface_20181201.nc",
            "sp,z,u10,v10,t2m,d2m,ishf,ie,blh,fsr,zust",
            "native_netcdf_era5_surface_measurements.json",
            "netcdf",
        ),
        (
            ROOT / "target/test-data/era5-cds-hybrid137-official/ready/era5_hybrid137_prepared_20181201.nc",
            "t,u,v,q,w,sp,lnsp",
            "native_netcdf_era5_hybrid_measurements.json",
            "netcdf",
        ),
        (
            ROOT / "target/test-data/era5-cds-hybrid137-official/ready/era5_surface_20181201.nc",
            "z,u10,v10,t2m,d2m,ishf,ie,blh,fsr,zust",
            "native_netcdf_era5_hybrid_surface_measurements.json",
            "netcdf",
        ),
    ]
    grib_cases = [
        (
            ROOT / "target/test-data/cfsr-ncei-pgbl-official/pgbl00.gdas.2009010100.grb2",
            "0.2.2:isobaric,0.2.3:isobaric,0.0.0:isobaric,0.1.0:isobaric,0.2.8:isobaric,0.3.0:surface,0.3.5:surface",
            "native_eccodes_cfsr_pgbl_measurements.json",
            "grib",
        ),
    ]

    def run_case(path, variables, out_name, fmt, env, feature):
        out = OUT / out_name
        if not path.is_file():
            write_json(out_name, {"status": "fixture_missing", "file": str(path), "format": fmt})
            return
        if not feature:
            write_json(
                out_name,
                {
                    "status": "feature_unavailable",
                    "file": str(path),
                    "format": fmt,
                    "adjudication": "unvalidated_measurement",
                },
            )
            return
        proc = run(
            [
                "cargo",
                "run",
                "--offline",
                "-p",
                "trajecta-met",
                "--features",
                feature,
                "--example",
                "measure_native_field_diff",
                "--",
                "--format",
                fmt,
                str(path),
                variables,
                str(out),
            ],
            env=env,
        )
        nl = chr(10)
        meta = {
            "status": "measured" if proc.returncode == 0 and out.is_file() else "failed",
            "exit_code": proc.returncode,
            "stdout_tail": nl.join(proc.stdout.splitlines()[-20:]),
            "stderr_tail": nl.join(proc.stderr.splitlines()[-40:]),
            "output": str(out.as_posix()),
            "format": fmt,
            "feature": feature,
            "adjudication": "unvalidated_measurement",
        }
        write_json(out_name + ".runmeta.json", meta)

    produced: list[str] = []
    missing_or_failed: list[str] = []
    skipped: list[str] = []

    def track(out_name: str) -> None:
        path = OUT / out_name
        if path.is_file():
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
            except Exception:
                missing_or_failed.append(out_name)
                return
            status = payload.get("status")
            if status in {"unvalidated_measurement", "measured"}:
                produced.append(out_name)
            elif status in {"fixture_missing", "feature_unavailable", "failed"}:
                missing_or_failed.append(out_name)
            else:
                produced.append(out_name)
        else:
            missing_or_failed.append(out_name)

    if netcdf_ok:
        for path, variables, out_name, fmt in netcdf_cases:
            run_case(path, variables, out_name, fmt, env_netcdf, "native-netcdf")
            track(out_name)
    else:
        write_json(
            "native_netcdf_structured_skipped.json",
            {
                "status": "feature_unavailable",
                "feature": "native-netcdf",
                "adjudication": "unvalidated_measurement",
            },
        )
        skipped.append("native-netcdf")

    if eccodes_ok:
        for path, variables, out_name, fmt in grib_cases:
            run_case(path, variables, out_name, fmt, env_eccodes, "native-eccodes")
            track(out_name)
    else:
        write_json(
            "native_eccodes_structured_skipped.json",
            {
                "status": "feature_unavailable",
                "feature": "native-eccodes",
                "adjudication": "unvalidated_measurement",
                "note": "No fresh native_eccodes_* measurement was produced this run; prior files were cleared.",
            },
        )
        skipped.append("native-eccodes")
        missing_or_failed.append("native_eccodes_cfsr_pgbl_measurements.json")

    return {
        "produced": produced,
        "missing_or_failed": missing_or_failed,
        "skipped_features": skipped,
        "netcdf_ok": netcdf_ok,
        "eccodes_ok": eccodes_ok,
    }


def main() -> int:
    env_netcdf = native_env(with_eccodes=False)
    env_eccodes = native_env(with_eccodes=True)
    # Probe each feature with the matching env (never mix).
    results = {}
    nl = chr(10)
    for feature, env in (("native-netcdf", env_netcdf), ("native-eccodes", env_eccodes)):
        proc = run(
            ["cargo", "check", "--offline", "-p", "trajecta-met", "--features", feature],
            env=env,
        )
        results[feature] = {
            "exit_code": proc.returncode,
            "available": proc.returncode == 0,
            "stderr_tail": nl.join(proc.stderr.splitlines()[-40:]),
        }
    probe = {
        "status": "measured"
        if any(v["available"] for v in results.values())
        else "external_blocked",
        "kind": "native_feature_probe",
        "features": results,
        "env_loaded": {
            "netcdf": {
                "NETCDF_DIR": env_netcdf.get("NETCDF_DIR"),
                "PKG_CONFIG_PATH": env_netcdf.get("PKG_CONFIG_PATH"),
            },
            "eccodes": {
                "ECCODES_DEFINITION_PATH": env_eccodes.get("ECCODES_DEFINITION_PATH"),
                "LIBCLANG_PATH": env_eccodes.get("LIBCLANG_PATH"),
                "PKG_CONFIG_PATH": env_eccodes.get("PKG_CONFIG_PATH"),
            },
        },
        "adjudication": "unvalidated_measurement",
        "notes": [
            "native-netcdf uses MSYS2 UCRT64 only.",
            "native-eccodes adds .native/eccodes after netcdf + LIBCLANG_PATH.",
        ],
    }
    write_json("native_feature_probe.json", probe)
    pure_rust_dual_load()
    if results.get("native-netcdf", {}).get("available"):
        run_example_measure(env_netcdf, "native-netcdf")
    if results.get("native-eccodes", {}).get("available"):
        run_example_measure(env_eccodes, "native-eccodes")
    netcdf_ok = bool(results.get("native-netcdf", {}).get("available"))
    eccodes_ok = bool(results.get("native-eccodes", {}).get("available"))
    structured = structured_measurements(
        env_netcdf,
        env_eccodes,
        netcdf_ok=netcdf_ok,
        eccodes_ok=eccodes_ok,
    )

    # Honest rollup: never "complete" if a backend was unavailable or a canonical
    # structured artifact is missing/failed. Exit non-zero so CI cannot greenwash.
    blockers: list[str] = []
    if not netcdf_ok:
        blockers.append("native-netcdf_unavailable")
    if not eccodes_ok:
        blockers.append("native-eccodes_unavailable")
    if structured.get("missing_or_failed"):
        blockers.extend(
            f"structured_missing:{name}" for name in structured["missing_or_failed"]
        )
    if blockers:
        summary_status = "incomplete"
        exit_code = 2
    else:
        summary_status = "measured_all_backends"
        exit_code = 0

    write_json(
        "SUMMARY.json",
        {
            "status": summary_status,
            "exit_code": exit_code,
            "adjudication": "unvalidated_measurement",
            "output_dir": str(OUT.as_posix()),
            "native_available": {k: v.get("available") for k, v in results.items()},
            "libclang_path_eccodes": env_eccodes.get("LIBCLANG_PATH"),
            "structured": structured,
            "blockers": blockers,
            "notes": [
                "status=measured_all_backends only when both native backends ran and every canonical structured file was freshly produced.",
                "status=incomplete means at least one backend/artifact is missing; do not treat as full native matrix.",
                "Never adjudicates tolerances.",
            ],
        },
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
