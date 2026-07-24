#!/usr/bin/env python3
"""Build and compare the GPL FLEXPART PV60 scalar rule with the Rust subject."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
FROZEN_FLEXPART_COMMIT = "dace3affa2ba71677f12f3858b04aaf59f8ee51e"
RELATIVE_TOLERANCE = 5.0e-7
ABSOLUTE_TOLERANCE_KG = 1.0e-30

VECTORS: tuple[dict[str, Any], ...] = (
    {
        "id": "eligible_northern",
        "carrier_mass_kg": 1.25,
        "height_asl_m": 4500.0,
        "latitude_degrees": 45.0,
        "potential_vorticity_pvu": 3.5,
    },
    {
        "id": "eligible_southern_sign_flip",
        "carrier_mass_kg": 2.5,
        "height_asl_m": 6500.0,
        "latitude_degrees": -30.0,
        "potential_vorticity_pvu": -4.25,
    },
    {
        "id": "height_exactly_3000_rejected",
        "carrier_mass_kg": 1.0,
        "height_asl_m": 3000.0,
        "latitude_degrees": 45.0,
        "potential_vorticity_pvu": 3.0,
    },
    {
        "id": "height_strictly_above_3000",
        "carrier_mass_kg": 1.0,
        "height_asl_m": 3000.5,
        "latitude_degrees": 45.0,
        "potential_vorticity_pvu": 3.0,
    },
    {
        "id": "pv_exactly_2_rejected",
        "carrier_mass_kg": 1.0,
        "height_asl_m": 4000.0,
        "latitude_degrees": 45.0,
        "potential_vorticity_pvu": 2.0,
    },
    {
        "id": "pv_strictly_above_2",
        "carrier_mass_kg": 1.0,
        "height_asl_m": 4000.0,
        "latitude_degrees": 45.0,
        "potential_vorticity_pvu": 2.001,
    },
    {
        "id": "wrong_sign_northern_rejected",
        "carrier_mass_kg": 3.0,
        "height_asl_m": 8000.0,
        "latitude_degrees": 60.0,
        "potential_vorticity_pvu": -4.0,
    },
    {
        "id": "wrong_sign_southern_rejected",
        "carrier_mass_kg": 3.0,
        "height_asl_m": 8000.0,
        "latitude_degrees": -60.0,
        "potential_vorticity_pvu": 4.0,
    },
    {
        "id": "equator_uses_northern_sign",
        "carrier_mass_kg": 10.0,
        "height_asl_m": 8000.0,
        "latitude_degrees": 0.0,
        "potential_vorticity_pvu": 2.5,
    },
    {
        "id": "small_carrier_mass",
        "carrier_mass_kg": 1.0e-9,
        "height_asl_m": 12000.0,
        "latitude_degrees": -75.0,
        "potential_vorticity_pvu": -8.0,
    },
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result.stdout.strip()


def verify_flexpart_sources(flexpart_root: Path) -> dict[str, str]:
    # The desktop sandbox runs under a different Windows account than the
    # user-owned FLEXPART checkout.  Keep the ownership exception local to
    # this read-only command; never mutate the user's global Git config.
    head = run(
        [
            "git",
            "-c",
            f"safe.directory={flexpart_root.as_posix()}",
            "rev-parse",
            "HEAD",
        ],
        cwd=flexpart_root,
    )
    if head != FROZEN_FLEXPART_COMMIT:
        raise RuntimeError(
            f"FLEXPART commit mismatch: {head} != {FROZEN_FLEXPART_COMMIT}"
        )
    par_mod = flexpart_root / "src/par_mod.f90"
    initdomain = flexpart_root / "src/initdomain_mod.f90"
    getfields = flexpart_root / "src/getfields_mod.f90"
    for path in (par_mod, initdomain, getfields):
        if not path.is_file():
            raise RuntimeError(f"missing FLEXPART source: {path}")

    normalized_par = " ".join(par_mod.read_text(encoding="utf-8").lower().split())
    normalized_init = " ".join(initdomain.read_text(encoding="utf-8").lower().split())
    required_par = "real,parameter :: ozonescale=60., pvcrit=2."
    required_formula = (
        "mass(numpart+jj,1)*pvpart*48./29.*ozonescale/10.**9"
    )
    if required_par not in normalized_par:
        raise RuntimeError("frozen ozonescale/pvcrit declaration not found")
    if required_formula not in normalized_init:
        raise RuntimeError("frozen ozone mass formula not found")
    if "part(numpart+jj)%z.gt.3000." not in normalized_init:
        raise RuntimeError("frozen strict 3000 m mask not found")
    if "pvpart.gt.pvcrit" not in normalized_init:
        raise RuntimeError("frozen strict PV mask not found")
    if "if (ylat.lt.0.) pvpart=-1.*pvpart" not in normalized_init:
        raise RuntimeError("frozen southern-hemisphere sign rule not found")

    return {
        "commit": head,
        "par_mod_sha256": sha256(par_mod),
        "initdomain_mod_sha256": sha256(initdomain),
        "getfields_mod_sha256": sha256(getfields),
    }


def parse_rows(output: str, expected: int, label: str) -> list[tuple[int, float]]:
    lines = [line for line in output.splitlines() if line.strip()]
    if len(lines) != expected:
        raise RuntimeError(f"{label}: expected {expected} rows, got {len(lines)}")
    rows: list[tuple[int, float]] = []
    for line in lines:
        tokens = line.split()
        if len(tokens) != 2:
            raise RuntimeError(f"{label}: malformed row: {line!r}")
        eligible = int(tokens[0])
        mass = float(tokens[1])
        if eligible not in (0, 1) or not (mass >= 0.0):
            raise RuntimeError(f"{label}: invalid row: {line!r}")
        rows.append((eligible, mass))
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--flexpart-root",
        type=Path,
        default=ROOT.parent / "flexpart",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=ROOT / "target/m4-a3/oracle",
    )
    args = parser.parse_args()

    artifact_dir = args.artifact_dir.resolve()
    build_dir = artifact_dir / "build"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    build_dir.mkdir(parents=True, exist_ok=True)
    source_identity = verify_flexpart_sources(args.flexpart_root.resolve())

    environment = os.environ.copy()
    compiler_tmp = build_dir / "tmp"
    compiler_tmp.mkdir(parents=True, exist_ok=True)
    for variable in ("TMP", "TEMP", "TMPDIR"):
        environment[variable] = str(compiler_tmp)
    if os.name == "nt":
        # Put the pinned UCRT64 runtime before unrelated applications that
        # also ship a libwinpthread-1.dll.  Otherwise f951 can load an
        # ABI-incompatible DLL and exit without diagnostics.
        ucrt_bin = Path("D:/msys64/ucrt64/bin")
        if ucrt_bin.is_dir():
            environment["PATH"] = (
                str(ucrt_bin) + os.pathsep + environment.get("PATH", "")
            )

    executable_suffix = ".exe" if os.name == "nt" else ""
    fortran_source = ROOT / "tools/flexpart_oracle/src/ozone_pv60_scalar_driver.f90"
    fortran_binary = build_dir / f"ozone_pv60_scalar_driver{executable_suffix}"
    run(
        [
            "gfortran",
            "-O0",
            "-std=f2008",
            "-Wall",
            "-Wextra",
            str(fortran_source),
            "-o",
            str(fortran_binary),
        ],
        cwd=ROOT,
        env=environment,
    )

    cargo_target = Path(
        environment.get(
            "TRAJECTA_M4_A3_ORACLE_CARGO_TARGET_DIR",
            str(build_dir / "cargo-target"),
        )
    )
    environment["CARGO_TARGET_DIR"] = str(cargo_target)
    run(
        [
            "cargo",
            "build",
            "--offline",
            "--release",
            "-p",
            "trajecta-core",
            "--example",
            "m4_a3_ozone_scalar_subject",
        ],
        cwd=ROOT,
        env=environment,
    )
    rust_binary = (
        cargo_target
        / "release/examples"
        / f"m4_a3_ozone_scalar_subject{executable_suffix}"
    )
    if not rust_binary.is_file():
        raise RuntimeError(f"missing Rust subject binary: {rust_binary}")

    input_text = "".join(
        f"{vector['carrier_mass_kg']:.17g} {vector['height_asl_m']:.17g} "
        f"{vector['latitude_degrees']:.17g} "
        f"{vector['potential_vorticity_pvu']:.17g}\n"
        for vector in VECTORS
    )
    fortran_result = subprocess.run(
        [str(fortran_binary)],
        cwd=ROOT,
        env=environment,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if fortran_result.returncode != 0:
        raise RuntimeError(f"Fortran oracle failed:\n{fortran_result.stderr}")
    rust_result = subprocess.run(
        [str(rust_binary)],
        cwd=ROOT,
        env=environment,
        input=input_text,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if rust_result.returncode != 0:
        raise RuntimeError(f"Rust subject failed:\n{rust_result.stderr}")

    oracle_rows = parse_rows(fortran_result.stdout, len(VECTORS), "Fortran")
    subject_rows = parse_rows(rust_result.stdout, len(VECTORS), "Rust")
    records: list[dict[str, Any]] = []
    all_passed = True
    for vector, oracle, subject in zip(VECTORS, oracle_rows, subject_rows, strict=True):
        oracle_eligible, oracle_mass = oracle
        subject_eligible, subject_mass = subject
        absolute_difference = abs(subject_mass - oracle_mass)
        scale = max(abs(subject_mass), abs(oracle_mass), sys.float_info.min)
        relative_difference = absolute_difference / scale
        mass_passed = (
            absolute_difference <= ABSOLUTE_TOLERANCE_KG
            or relative_difference <= RELATIVE_TOLERANCE
        )
        passed = oracle_eligible == subject_eligible and mass_passed
        all_passed = all_passed and passed
        records.append(
            {
                "id": vector["id"],
                "input": {key: value for key, value in vector.items() if key != "id"},
                "flexpart_default_real32": {
                    "eligible": bool(oracle_eligible),
                    "ozone_mass_kg": oracle_mass,
                },
                "trajecta_f64": {
                    "eligible": bool(subject_eligible),
                    "ozone_mass_kg": subject_mass,
                },
                "absolute_difference_kg": absolute_difference,
                "relative_difference": relative_difference,
                "passed": passed,
            }
        )

    report = {
        "schema_version": "trajecta.m4-a3-ozone-scalar-oracle/v1",
        "status": "passed" if all_passed else "failed",
        "license_boundary": "GPL subprocess oracle; no GPL code linked into MIT crates",
        "flexpart": source_identity,
        "oracle_driver": {
            "source_sha256": sha256(fortran_source),
            "binary_sha256": sha256(fortran_binary),
            "compile_flags": ["-O0", "-std=f2008", "-Wall", "-Wextra"],
            "default_real_bits": 32,
            "host_platform": platform.platform(),
        },
        "trajecta_subject": {
            "binary_sha256": sha256(rust_binary),
            "rule_id": "flexpart_stratospheric_ozone_pv60/v1",
        },
        "tolerance": {
            "relative": RELATIVE_TOLERANCE,
            "absolute_kg": ABSOLUTE_TOLERANCE_KG,
            "reason": "FLEXPART oracle uses default REAL(32); Trajecta uses f64",
        },
        "coverage": {
            "expected_records": len(VECTORS),
            "compared_records": len(records),
            "strict_height_boundary": True,
            "strict_pv_boundary": True,
            "both_hemispheres": True,
            "equator": True,
        },
        "records": records,
    }
    output = artifact_dir / "M4_A3_OZONE_SCALAR_ORACLE.json"
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if all_passed else 2


if __name__ == "__main__":
    raise SystemExit(main())
