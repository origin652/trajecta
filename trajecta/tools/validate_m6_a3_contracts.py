#!/usr/bin/env python3
"""Validate the M6-A3 deep-convection production slice and frozen assets."""

from __future__ import annotations

import hashlib
import json
import tomllib
from pathlib import Path

from jsonschema import Draft202012Validator


ROOT = Path(__file__).resolve().parents[1]
TESTDATA = ROOT / "testdata"

EXPECTED_SHA256 = {
    "testdata/m6-a3/M6_ARM_TWP_ICE_COLUMN.schema.json":
        "24bd510a0b51272c2ef47508d918df681e4779005f05aa26ba28dd81b92e428a",
    "testdata/m6-a3/M6_ARM_TWP_ICE_COLUMN.v1.json":
        "144ac0a0e0f4043df5fb5b1fe57ddebe57e2c65c9cc5df6b33c5bf727e86b259",
    "tools/derive_m6_twpice_column.py":
        "b09f38d2687c518f1dd2004049e4dd4a79293c7d9cef085a907ade5089fcca0e",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def load_unique_json(path: Path) -> dict[str, object]:
    def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        value: dict[str, object] = {}
        for key, item in pairs:
            require(key not in value, f"duplicate JSON key in {path.name}: {key}")
            value[key] = item
        return value

    value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object)
    require(isinstance(value, dict), f"{path.name} must contain an object")
    return value


def read_source(relative: str) -> str:
    path = ROOT / relative
    require(path.is_file(), f"M6-A3 file is missing: {relative}")
    return path.read_text(encoding="utf-8")


def validate_asset() -> dict[str, object]:
    for relative, expected in EXPECTED_SHA256.items():
        path = ROOT / relative
        require(path.is_file(), f"M6-A3 asset is missing: {relative}")
        require(
            hashlib.sha256(path.read_bytes()).hexdigest() == expected,
            f"M6-A3 asset identity drift: {relative}",
        )
        raw = path.read_bytes()
        require(not raw.startswith(b"\xef\xbb\xbf"), f"UTF-8 BOM in {relative}")
        require(b"\r" not in raw, f"non-LF line ending in {relative}")
        require(raw.endswith(b"\n"), f"missing final newline in {relative}")

    schema = load_unique_json(TESTDATA / "m6-a3/M6_ARM_TWP_ICE_COLUMN.schema.json")
    asset = load_unique_json(TESTDATA / "m6-a3/M6_ARM_TWP_ICE_COLUMN.v1.json")
    Draft202012Validator.check_schema(schema)
    Draft202012Validator(schema).validate(asset)
    require(asset["schema_version"] == "trajecta.m6.twp-ice-convection/v1", "A3 schema drift")
    columns = asset["columns"]
    require(isinstance(columns, list) and len(columns) == 7, "A3 requires seven TWP-ICE columns")
    normalization = asset["validation_normalization"]
    observations = sum(
        2 * len(column["independent_reference"]["upward_interface_mass_flux_kg_m2_s"])
        for column in columns
    )
    require(
        observations >= normalization["minimum_profile_observations"],
        "M6-A3 external validation has too few profile observations",
    )
    return asset


def validate_production_wiring() -> None:
    physics = read_source("crates/trajecta-core/src/physics/mod.rs")
    convection = read_source("crates/trajecta-core/src/physics/convection.rs")
    runner = read_source("crates/trajecta-core/src/runner/mod.rs")
    query = read_source("crates/trajecta-met/src/query/engine.rs")
    sqlite = read_source("crates/trajecta-core/src/output/sqlite.rs")
    verification = read_source("crates/trajecta-core/src/verification.rs")
    cli = read_source("crates/trajecta-cli/src/result_products.rs")
    real_matrix = read_source("crates/trajecta-core/tests/m6_a2_real_families.rs")
    cli_test = read_source("crates/trajecta-cli/tests/m6_a3_process_queries.rs")

    require(
        "emanuel_zivkovic_rothman_conservative_column/v1" in physics,
        "M6-A3 implementation identity is missing",
    )
    for symbol in (
        "precipitation_downdraft_flux",
        "exponentiate_generator",
        "sample_adjoint",
        "buoyancy-sorting distribution does not close",
    ):
        require(symbol in convection, f"M6-A3 column implementation omits {symbol}")
    require(
        "apply_convection(" in runner and "record_convection_transfers" in runner,
        "M6-A3 post-motion column transfer is missing",
    )
    require(
        "prepare_convection_batch" in query and "sample_convection_point" in query,
        "M6-A3 complete thermodynamic-column query is missing",
    )
    require(
        "write_convection_events" in sqlite and "INSERT INTO convection_event" in sqlite,
        "M6-A3 process-event sink is missing",
    )
    require(
        "audit_convection_particle_closure" in verification
        and "result.convection_particle_closure_invalid" in verification,
        "M6-A3 full verification lacks particle-level closure",
    )
    require(
        "filtered_process_particles" in cli
        and "ORDER BY e.macro_step, e.event_sequence" in cli,
        "M6-A3 process query does not preserve particle/operator order",
    )
    require(
        "deep_convection_runs_on_all_real_families_in_both_directions" in real_matrix,
        "M6-A3 three-family directional matrix is missing",
    )
    require(
        "production_forward_and_backward_processes_cover_filters_and_stream_shapes" in cli_test,
        "M6-A3 production CLI query test is missing",
    )
    require(
        not any(value in runner for value in ("ConvectionHalf", "apply_convection_half")),
        "M6-A3 retains the superseded split convection path",
    )

    legacy_validator = ROOT / "tools/validate_m6_a0_contracts.py"
    require(
        hashlib.sha256(legacy_validator.read_bytes()).hexdigest()
        == "51e026fcb51f233852a7ca340c2b2f8997bc2a0bb9e1d8fa2e7d68bef53050e4",
        "the frozen M6-A0 validator changed during M6-A3",
    )


def validate_versions() -> None:
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    require(
        workspace["workspace"]["package"]["version"] == "0.1.0-alpha.1",
        "software version changed during M6-A3",
    )
    case_schema = read_source("crates/trajecta-case/src/schema.rs")
    require(
        "pub const CURRENT_SCHEMA_VERSION: u32 = 0;" in case_schema,
        "Case schema version changed during M6-A3",
    )
    require(
        "pub const SQLITE_SCHEMA_VERSION: u32 = 2;"
        in read_source("crates/trajecta-core/src/science.rs"),
        "SQLite user_version changed during M6-A3",
    )


def main() -> int:
    asset = validate_asset()
    validate_production_wiring()
    validate_versions()
    print(
        json.dumps(
            {
                "status": "passed",
                "production_stage": "m6_a3",
                "twp_ice_columns": len(asset["columns"]),
                "sqlite_user_version": 2,
                "software_version": "0.1.0-alpha.1",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
