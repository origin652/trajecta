#!/usr/bin/env python3
"""Freeze the seven ARM TWP-ICE event-C columns used by M6-A3."""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import netCDF4
import numpy as np


EXPECTED_ARCHIVE_SHA256 = "3f901165cf65c075d65190daaa596a4929b1d068828d7275c70f5c63fba2233b"
EXPECTED_NETCDF_SHA256 = "8d065583747ed9f0070e12bf27a8720eecb9b1f4c5aa46351339875b1cb5ee0a"
EXPECTED_REFERENCE_SHA256 = "78bea5ea6afa91e15db01d6b4faf69ad0b81efb3dd2de9690d30d7ce64f7ddd1"
EXPECTED_REFERENCE_COMMIT = "c6cc8a0dcbfe32d79ce82b3e270e6af834558f28"
REFERENCE_LICENSE_SHA256 = "69614ed788a7cdee9570c876262146aa4eb21c4c1304a232ac4c1fe82b659f48"
REFERENCE_FORTRAN_SHA256 = "65bc6f77374d18e58c814e9d50b35747c1192baf3cbda2c9f97ca8e4d135fbac"
STANDARD_GRAVITY = 9.8
DRY_AIR_GAS_CONSTANT = 287.04
WATER_VAPOR_GAS_CONSTANT = 461.5


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require_identity(path: Path, expected: str, label: str) -> None:
    actual = sha256(path)
    if actual != expected:
        raise ValueError(f"unexpected {label} SHA-256: {actual}")


def finite(variable: netCDF4.Variable) -> np.ndarray:
    values = np.ma.asarray(variable[:], dtype=np.float64)
    if np.any(np.ma.getmaskarray(values)):
        raise ValueError(f"{variable.name} contains masked values")
    result = np.asarray(values, dtype=np.float64)
    if not np.all(np.isfinite(result)):
        raise ValueError(f"{variable.name} contains non-finite values")
    return result


def event_indices(dataset: netCDF4.Dataset) -> list[int]:
    day = finite(dataset.variables["day"]).astype(np.int64)
    hour = finite(dataset.variables["hour"]).astype(np.int64)
    selected = [
        index
        for index, (day_value, hour_value) in enumerate(zip(day, hour, strict=True))
        if (day_value == 23 and hour_value >= 12)
        or (day_value == 24 and hour_value <= 6)
    ]
    if [(int(day[i]), int(hour[i])) for i in selected] != [
        (23, 12),
        (23, 15),
        (23, 18),
        (23, 21),
        (24, 0),
        (24, 3),
        (24, 6),
    ]:
        raise ValueError("TWP-ICE event-C selection changed")
    return selected


def layer_geometry(
    pressure_hpa: np.ndarray,
    temperature_k: np.ndarray,
    specific_humidity: np.ndarray,
    surface_pressure_hpa: float,
    terrain_height_m: float,
) -> tuple[np.ndarray, np.ndarray]:
    interfaces = np.empty(pressure_hpa.size + 1, dtype=np.float64)
    interfaces[0] = surface_pressure_hpa
    interfaces[1:-1] = 0.5 * (pressure_hpa[:-1] + pressure_hpa[1:])
    interfaces[-1] = max(
        0.1,
        pressure_hpa[-1] - 0.5 * (pressure_hpa[-2] - pressure_hpa[-1]),
    )
    layer_mass = (interfaces[:-1] - interfaces[1:]) * 100.0 / STANDARD_GRAVITY
    virtual_temperature = temperature_k * (
        1.0
        + specific_humidity
        * (WATER_VAPOR_GAS_CONSTANT / DRY_AIR_GAS_CONSTANT - 1.0)
    )
    height = np.empty_like(pressure_hpa)
    height[0] = terrain_height_m + (
        DRY_AIR_GAS_CONSTANT
        * virtual_temperature[0]
        / STANDARD_GRAVITY
        * np.log(surface_pressure_hpa / pressure_hpa[0])
    )
    for index in range(1, pressure_hpa.size):
        mean_virtual = 0.5 * (
            virtual_temperature[index - 1] + virtual_temperature[index]
        )
        height[index] = height[index - 1] + (
            DRY_AIR_GAS_CONSTANT
            * mean_virtual
            / STANDARD_GRAVITY
            * np.log(pressure_hpa[index - 1] / pressure_hpa[index])
        )
    if not np.all(layer_mass > 0.0) or not np.all(np.diff(height) > 0.0):
        raise ValueError("derived TWP-ICE vertical geometry is invalid")
    return height, layer_mass


def load_reference(path: Path) -> list[dict[str, Any]]:
    require_identity(path, EXPECTED_REFERENCE_SHA256, "reference summary")
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, list) or len(payload) != 7:
        raise ValueError("reference summary must contain seven columns")
    return payload


def derive(
    input_path: Path,
    archive_path: Path,
    reference_path: Path,
) -> dict[str, Any]:
    require_identity(archive_path, EXPECTED_ARCHIVE_SHA256, "raw archive")
    require_identity(input_path, EXPECTED_NETCDF_SHA256, "TWP-ICE NetCDF")
    reference = load_reference(reference_path)
    columns: list[dict[str, Any]] = []
    with netCDF4.Dataset(input_path) as dataset:
        pressure_hpa_all = finite(dataset.variables["lev"])
        temperatures = finite(dataset.variables["T"])
        mixing_ratios_g_kg = finite(dataset.variables["q"])
        surface_pressure_hpa = finite(dataset.variables["p_srf_aver"])
        precipitation_mm_h = finite(dataset.variables["prec_srf"])
        days = finite(dataset.variables["day"]).astype(np.int64)
        hours = finite(dataset.variables["hour"]).astype(np.int64)
        terrain_height_m = float(finite(dataset.variables["alt"]))
        longitude = float(finite(dataset.variables["lon"]))
        latitude_degrees_south = float(finite(dataset.variables["lat"]))
        indices = event_indices(dataset)
        for output_index, source_index in enumerate(indices):
            surface_pressure = float(surface_pressure_hpa[source_index])
            keep = pressure_hpa_all <= surface_pressure
            pressure_hpa = pressure_hpa_all[keep]
            temperature_k = temperatures[source_index, keep]
            mixing_ratio = mixing_ratios_g_kg[source_index, keep] * 1.0e-3
            specific_humidity = mixing_ratio / (1.0 + mixing_ratio)
            height_m, layer_mass = layer_geometry(
                pressure_hpa,
                temperature_k,
                specific_humidity,
                surface_pressure,
                terrain_height_m,
            )
            instant = datetime(
                2006,
                1,
                int(days[source_index]),
                int(hours[source_index]),
                tzinfo=timezone.utc,
            ).isoformat().replace("+00:00", "Z")
            oracle = reference[output_index]
            if oracle.get("time") != instant:
                raise ValueError("reference and observation times differ")
            upward = np.asarray(
                oracle["upward_interface_mass_flux_kg_m2_s"], dtype=np.float64
            )
            downward = np.asarray(
                oracle["downward_interface_mass_flux_kg_m2_s"], dtype=np.float64
            )
            if upward.shape != (pressure_hpa.size - 1,) or downward.shape != upward.shape:
                raise ValueError("reference mass-flux profile shape changed")
            columns.append(
                {
                    "time_utc": instant,
                    "observed_precipitation_mm_h": float(
                        precipitation_mm_h[source_index]
                    ),
                    "thermodynamic_column": {
                        "surface_pressure_pa": surface_pressure * 100.0,
                        "terrain_height_asl_m": terrain_height_m,
                        "pressure_pa": (pressure_hpa[::-1] * 100.0).tolist(),
                        "height_asl_m": height_m[::-1].tolist(),
                        "temperature_k": temperature_k[::-1].tolist(),
                        "specific_humidity_kg_kg": specific_humidity[::-1].tolist(),
                        "pressure_layer_mass_kg_m2": layer_mass[::-1].tolist(),
                        "valid": [True] * pressure_hpa.size,
                    },
                    "independent_reference": {
                        "flag": int(oracle["flag"]),
                        "cloud_base_mass_flux_kg_m2_s": float(
                            oracle["cloud_base_mass_flux_kg_m2_s"]
                        ),
                        "upward_interface_mass_flux_kg_m2_s": upward[::-1].tolist(),
                        "downward_interface_mass_flux_kg_m2_s": downward[::-1].tolist(),
                        "maximum_generator_column_residual_s-1": float(
                            oracle["maximum_column_sum_residual_s-1"]
                        ),
                        "minimum_generator_off_diagonal_s-1": float(
                            oracle["minimum_off_diagonal_s-1"]
                        ),
                    },
                }
            )
    return {
        "schema_version": "trajecta.m6.twp-ice-convection/v1",
        "asset_id": "arm-twp-ice-convective-column/v1",
        "period": {
            "name": "event C",
            "start_utc": "2006-01-23T12:00:00Z",
            "end_utc": "2006-01-24T06:00:00Z",
            "interval_seconds": 10_800,
        },
        "location": {
            "longitude_degrees_east": longitude,
            "latitude_degrees_north": -latitude_degrees_south,
        },
        "observation_source": {
            "provider": "U.S. Department of Energy ARM / NCAR CCPP-SCM",
            "campaign": "Tropical Warm Pool International Cloud Experiment",
            "campaign_url": "https://www.arm.gov/research/campaigns/twp2006twp-ice",
            "ccpp_scm_release": "v7.0.0",
            "archive_url": "https://github.com/NCAR/ccpp-scm/releases/download/v7.0.0/raw_case_input.tar.gz",
            "archive_member": "twp180iopsndgvarana_v2.1_C3.c1.20060117.000000.cdf",
            "archive_size_bytes": archive_path.stat().st_size,
            "archive_sha256": EXPECTED_ARCHIVE_SHA256,
            "netcdf_size_bytes": input_path.stat().st_size,
            "netcdf_sha256": EXPECTED_NETCDF_SHA256,
        },
        "independent_reference": {
            "repository": "https://github.com/climlab/climlab-emanuel-convection",
            "commit": EXPECTED_REFERENCE_COMMIT,
            "license": "MIT",
            "license_sha256": REFERENCE_LICENSE_SHA256,
            "convect43c_sha256": REFERENCE_FORTRAN_SHA256,
            "summary_input_size_bytes": reference_path.stat().st_size,
            "summary_input_sha256": EXPECTED_REFERENCE_SHA256,
            "role": "validation oracle only; no runtime or build dependency",
            "roundoff_policy": "negative off-diagonal rates no smaller than -1e-10 s-1 are clipped only while deriving reference mass-flux profiles",
        },
        "derivation": {
            "stored_order": "model_top_to_surface",
            "specific_humidity_formula": "q = r / (1 + r)",
            "height_formula": "hypsometric integration with virtual temperature",
            "layer_mass_formula": "delta_p / 9.8",
            "below_surface_policy": "exclude levels with pressure greater than surface pressure",
            "missing_value_policy": "reject masked or non-finite source values",
        },
        "validation_normalization": {
            "mass_flux_scale_kg_m2_s": max(
                max(
                    column["independent_reference"]["upward_interface_mass_flux_kg_m2_s"]
                    + column["independent_reference"]["downward_interface_mass_flux_kg_m2_s"]
                )
                for column in columns
            ),
            "minimum_profile_observations": 20,
        },
        "columns": columns,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    payload = derive(arguments.input, arguments.archive, arguments.reference)
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    arguments.output.write_text(encoded, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
