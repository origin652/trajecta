#!/usr/bin/env python3
"""Validate the frozen M6-A0 science, data, result, and recovery contracts."""

from __future__ import annotations

import copy
import hashlib
import json
import re
import sqlite3
import tomllib
from pathlib import Path, PurePosixPath

from jsonschema import Draft202012Validator


ROOT = Path(__file__).resolve().parents[1]
TESTDATA = ROOT / "testdata"
ENGINEERING = ROOT / "docs" / "engineering"

EXPECTED_MODULES = [
    "emission_time_profile",
    "buoyant_plume_rise",
    "subgrid_orography",
    "boundary_layer_langevin",
    "mesoscale_markov",
    "gravitational_settling",
    "deep_convection_column",
    "water_vapor_exchange",
    "dry_deposition",
    "wet_scavenging",
    "first_order_decay",
    "oh_oxidation",
]

EXPECTED_PRESETS = {
    "water_vapor_tracking": [
        "subgrid_orography",
        "boundary_layer_langevin",
        "mesoscale_markov",
        "deep_convection_column",
        "water_vapor_exchange",
    ],
    "gas_transport": [
        "subgrid_orography",
        "boundary_layer_langevin",
        "mesoscale_markov",
        "deep_convection_column",
        "dry_deposition",
        "wet_scavenging",
        "first_order_decay",
        "oh_oxidation",
    ],
    "aerosol_transport": [
        "subgrid_orography",
        "boundary_layer_langevin",
        "mesoscale_markov",
        "gravitational_settling",
        "deep_convection_column",
        "dry_deposition",
        "wet_scavenging",
    ],
    "buoyant_release": [
        "emission_time_profile",
        "buoyant_plume_rise",
        "subgrid_orography",
        "boundary_layer_langevin",
        "mesoscale_markov",
        "deep_convection_column",
    ],
}

EXPECTED_AUXILIARY = {
    "gmted2010-30arcsec-mean-std/v1",
    "modis-mcd12c1.061-igbp/v1",
    "modis-mcd15a3h.061-lai-climatology-2003-2022/v1",
    "spivakovsky-2000-oh-monthly-3d/v1",
}

EXPECTED_SQL_TABLES = {
    "chemistry_event",
    "convection_event",
    "deposition_event",
    "emission_event",
    "output_event",
    "particle",
    "particle_adjoint",
    "particle_aerosol_property",
    "particle_mass",
    "particle_state",
    "process_event",
    "process_summary",
    "run",
    "termination",
    "water_vapor_event",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def unique_json_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        require(key not in value, f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_json(name: str) -> dict[str, object]:
    value = json.loads(
        (TESTDATA / name).read_text(encoding="utf-8"),
        object_pairs_hook=unique_json_object,
    )
    require(isinstance(value, dict), f"{name} must contain a JSON object")
    return value


def schema_validator(name: str) -> Draft202012Validator:
    schema = load_json(name)
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


def expect_integrity_error(action: object, message: str) -> None:
    try:
        action()  # type: ignore[operator]
    except sqlite3.IntegrityError:
        return
    raise RuntimeError(message)


def timestamp_key(value: dict[str, int]) -> tuple[int, int]:
    return value["seconds_since_unix_epoch"], value["nanosecond"]


def validate_schema_examples() -> None:
    pairs = [
        ("M6_PHYSICS_CONTRACT.schema.json", "M6_PHYSICS_CONTRACT.v1.json"),
        ("M6_TOLERANCES.schema.json", "M6_TOLERANCES.v1.json"),
        ("M6_VALIDATION_ASSETS.schema.json", "M6_VALIDATION_ASSETS.v1.json"),
        ("M6_CHECKPOINT_MANIFEST.schema.json", "M6_CHECKPOINT_MANIFEST.example.json"),
        ("M6_PROCESS_QUERY.schema.json", "M6_PROCESS_QUERY.example.json"),
    ]
    for schema_name, example_name in pairs:
        schema_validator(schema_name).validate(load_json(example_name))

    checkpoint_validator = schema_validator("M6_CHECKPOINT_MANIFEST.schema.json")
    escaping = copy.deepcopy(load_json("M6_CHECKPOINT_MANIFEST.example.json"))
    escaping["payload"][0]["relative_path"] = "../particles.sqlite"
    require(
        not checkpoint_validator.is_valid(escaping),
        "checkpoint schema accepts an escaping payload path",
    )

    process_validator = schema_validator("M6_PROCESS_QUERY.schema.json")
    both_directions = copy.deepcopy(load_json("M6_PROCESS_QUERY.example.json"))
    both_directions["events"][0]["backward"] = {
        "survival_multiplier": 1.0,
        "source_sensitivity": 0.0,
        "importance_weight": 1.0,
    }
    require(
        not process_validator.is_valid(both_directions),
        "process event accepts simultaneous forward and backward payloads",
    )


def validate_versions() -> None:
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    require(
        workspace["workspace"]["package"]["version"] == "0.1.0-alpha.1",
        "workspace software version drifted from M6-A0",
    )
    physics = load_json("M6_PHYSICS_CONTRACT.v1.json")
    require(
        physics["versions"]
        == {"software": "0.1.0-alpha.1", "case_schema": 0, "sqlite_user_version": 2},
        "M6 version boundary drift",
    )

    science_rs = (ROOT / "crates/trajecta-core/src/science.rs").read_text(
        encoding="utf-8"
    )
    require(
        "pub const SQLITE_SCHEMA_VERSION: u32 = 2;" in science_rs,
        "M6-A1 production SQLite version is not 2",
    )
    sqlite_rs = (ROOT / "crates/trajecta-core/src/output/sqlite.rs").read_text(
        encoding="utf-8"
    )
    require(
        'include_str!("../../../../testdata/M6_SQLITE_SCHEMA.v2.sql")' in sqlite_rs,
        "M6-A1 production sink does not use the frozen SQLite v2 schema",
    )


def validate_a1_wiring() -> None:
    physics_rs = (ROOT / "crates/trajecta-core/src/physics/mod.rs").read_text(
        encoding="utf-8"
    )
    boundary_layer_rs = (
        ROOT / "crates/trajecta-core/src/physics/boundary_layer.rs"
    ).read_text(encoding="utf-8")
    runner_rs = (ROOT / "crates/trajecta-core/src/runner/mod.rs").read_text(
        encoding="utf-8"
    )
    result_rs = (ROOT / "crates/trajecta-cli/src/result_products.rs").read_text(
        encoding="utf-8"
    )
    trajectory_schema = load_json("M5_TRAJECTORY_RECORD.schema.json")
    manifest_schema = load_json("M4_RUN_MANIFEST.schema.json")

    require("pub struct PhysicsPipeline" in physics_rs, "single physics pipeline missing")
    require(
        "thomson_hanna_skewed_cbl_langevin/v1" in physics_rs,
        "boundary-layer implementation identity missing",
    )
    require("ProcessRandomKey" in boundary_layer_rs, "process counter RNG is not wired")
    require(
        "if self.physics.is_none()" in runner_rs,
        "pure-advection branch is no longer explicit",
    )
    require(
        "physics.module_stage_not_available" in physics_rs,
        "later-stage module diagnostic missing",
    )
    require(
        "trajecta.process-query/v1" in result_rs
        and "result.process_schema_unavailable" in result_rs,
        "process result query is not wired",
    )
    require(
        "sensitivity_weight" not in json.dumps(trajectory_schema, sort_keys=True),
        "legacy scalar sensitivity remains in the trajectory contract",
    )
    particle = trajectory_schema["$defs"]["particle"]["allOf"][1]
    require(
        "substance_adjoint_weight" in particle["required"],
        "trajectory contract lacks per-substance adjoint state",
    )
    require(
        manifest_schema["$defs"]["sqlite"]["properties"]["schema_version"]["enum"]
        == [1, 2],
        "run manifest does not describe readable v1 and current v2 SQLite",
    )
    require(
        (ROOT / "crates/trajecta-core/tests/m6_a1_physics.rs").is_file(),
        "M6-A1 end-to-end test is missing",
    )


def validate_a2_wiring() -> None:
    physics_rs = (ROOT / "crates/trajecta-core/src/physics/mod.rs").read_text(
        encoding="utf-8"
    )
    builder_rs = (ROOT / "crates/trajecta-core/src/runner/builder.rs").read_text(
        encoding="utf-8"
    )
    query_rs = (ROOT / "crates/trajecta-met/src/query/engine.rs").read_text(
        encoding="utf-8"
    )
    mesoscale_rs = (
        ROOT / "crates/trajecta-met/src/query/mesoscale.rs"
    ).read_text(encoding="utf-8")
    gmted_rs = (
        ROOT / "crates/trajecta-met/src/auxiliary/gmted2010.rs"
    ).read_text(encoding="utf-8")
    helper = (ROOT / "tools/fetch_trajecta_data.py").read_text(encoding="utf-8")

    require(
        "gmted2010_anomaly_stability_limited_mixing/v1" in physics_rs
        and ".prepare_stability_batch(" in physics_rs,
        "M6-A2 terrain correction is not wired to the stability query",
    )
    require(
        ".prepare_mesoscale_batch(" in physics_rs
        and "pub fn prepare_mesoscale_batch" in query_rs
        and "three_dimensional_local_variance_ou/v1" in mesoscale_rs,
        "M6-A2 mesoscale query is not wired to the production pipeline",
    )
    require(
        "requires_gmted2010" in builder_rs
        and "open_from_dataset_lock" in builder_rs,
        "M6-A2 production runner does not require and open the GMTED2010 lock",
    )
    require(
        "pub fn build_global_dataset_lock" in gmted_rs
        and "pub fn open_from_dataset_lock" in gmted_rs
        and "trajecta.gmted2010-grid/v1" in gmted_rs,
        "M6-A2 GMTED2010 lock or prepared-grid contract is missing",
    )
    require(
        "GMTED2010_RELEASE_BASE" in helper and "gmted2010_requests" in helper,
        "GMTED2010 data helper support is missing",
    )
    for relative_path in (
        "crates/trajecta-met/tests/real_gmted2010.rs",
        "crates/trajecta-core/tests/m6_a2_gmted_boundary.rs",
        "crates/trajecta-core/tests/m6_a2_real_families.rs",
    ):
        require(ROOT.joinpath(relative_path).is_file(), f"M6-A2 test is missing: {relative_path}")


def validate_physics_contract() -> None:
    contract = load_json("M6_PHYSICS_CONTRACT.v1.json")
    configuration = contract["configuration"]
    require(configuration["absent_physics"] == "pure_advection", "absent physics drift")
    require(
        configuration["merge_order"]
        == [
            "expand_preset",
            "apply_remove",
            "apply_sparse_overrides",
            "append_modules",
            "validate_uniqueness_order_and_dependencies",
        ],
        "physics merge order drift",
    )
    require(
        set(configuration["legacy_fields_rejected"]) == {"enabled", "parameters"},
        "legacy physics fields are not rejected",
    )

    modules = contract["modules"]
    module_ids = [entry["id"] for entry in modules]
    require(module_ids == EXPECTED_MODULES, "module membership or stage order drift")
    require(len(module_ids) == len(set(module_ids)), "duplicate module id")
    known = set(module_ids)

    for module in modules:
        module_id = module["id"]
        require(
            module["override_fields"] == sorted(module["override_fields"]),
            f"{module_id} override fields are not sorted",
        )
        require(
            module["requires_capabilities"] == sorted(module["requires_capabilities"]),
            f"{module_id} capabilities are not sorted",
        )
        require(
            set(module["requires_modules"]) <= known,
            f"{module_id} depends on an unknown module",
        )
        parameter_names = [entry["name"] for entry in module["parameter_contracts"]]
        require(
            len(parameter_names) == len(set(parameter_names)),
            f"{module_id} has duplicate parameter contracts",
        )
        require(
            set(parameter_names) == set(module["override_fields"]),
            f"{module_id} override and parameter contracts differ",
        )
        for parameter in module["parameter_contracts"]:
            lower = parameter["minimum_si"]
            upper = parameter["maximum_si"]
            require(
                lower is None or upper is None or lower <= upper,
                f"{module_id}.{parameter['name']} bounds reversed",
            )
            default = parameter["default"]
            if isinstance(default, (int, float)) and not isinstance(default, bool):
                require(
                    lower is None or default >= lower,
                    f"{module_id}.{parameter['name']} default below minimum",
                )
                require(
                    upper is None or default <= upper,
                    f"{module_id}.{parameter['name']} default above maximum",
                )

    constants = contract["module_constants"]
    require(set(constants) == known, "module constants do not cover exactly all modules")

    presets = contract["presets"]
    actual_presets = {entry["id"]: entry["modules"] for entry in presets}
    require(actual_presets == EXPECTED_PRESETS, "preset module order drift")
    for preset, preset_modules in actual_presets.items():
        require(len(preset_modules) == len(set(preset_modules)), f"duplicate in {preset}")
        require(set(preset_modules) <= known, f"unknown module in {preset}")
        positions = {module: index for index, module in enumerate(preset_modules)}
        for module_id in preset_modules:
            module = modules[module_ids.index(module_id)]
            for dependency in module["requires_modules"]:
                require(
                    dependency in positions and positions[dependency] < positions[module_id],
                    f"preset {preset} violates {module_id} dependency {dependency}",
                )

    mapping = contract["land_cover_mapping"]
    require(set(mapping) == {str(value) for value in range(1, 18)}, "IGBP mapping gap")
    require(set(mapping.values()) == set(range(1, 14)), "Wesely mapping category gap")

    dimensions = contract["randomness"]["dimensions"]
    require(len(set(dimensions.values())) == len(dimensions), "random dimension collision")
    require(
        contract["pipeline"]["determinism"]
        == "same_seed_w1_wn_normalized_science_byte_identical",
        "worker determinism drift",
    )
    require(
        {entry["path"] for entry in contract["commands"]}
        == {"job resume", "result processes"},
        "M6 command surface drift",
    )
    require(
        contract["automatic_recovery"]["maximum_automatic_recoveries"] == 3,
        "automatic recovery limit drift",
    )
    require(
        contract["checkpoint"]["completed_job_policy"]
        == "reject_resume_and_do_not_rerun",
        "completed-job recovery policy drift",
    )


def validate_tolerances() -> None:
    tolerances = load_json("M6_TOLERANCES.v1.json")
    require(tolerances["frozen_before_optimization"] is True, "tolerances not frozen")
    require(tolerances["analytic_and_manufactured"]["relative"] == 1e-10, "analytic gate drift")
    require(tolerances["conservation"]["relative"] == 1e-10, "conservation gate drift")
    require(tolerances["adjoint"]["dot_product_relative"] == 1e-8, "adjoint gate drift")
    require(
        tolerances["stochastic"]["ou"]
        == {
            "mean_sigma": 0.02,
            "variance_relative": 0.03,
            "lag_one_correlation_absolute": 0.02,
        },
        "OU gates drift",
    )
    require(tolerances["stochastic"]["minimum_samples"] >= 1_000_000, "OU sample count too small")
    require(tolerances["stochastic"]["minimum_independent_seeds"] >= 8, "OU seed count too small")
    require(
        tolerances["external_validation"]["normalized_bias_absolute"] == 0.20
        and tolerances["external_validation"]["normalized_rmse"] == 0.30,
        "external validation gates drift",
    )
    resource = tolerances["performance"]["formal_resource"]
    require(resource["peak_rss_bytes"] == 4 * 1024**3, "RSS gate is not 4 GiB")
    require(resource["sqlite_bytes"] == 4 * 1024**3, "SQLite gate is not 4 GiB")


def validate_assets() -> None:
    registry = load_json("M6_VALIDATION_ASSETS.v1.json")
    auxiliary = registry["auxiliary_datasets"]
    ids = [entry["id"] for entry in auxiliary]
    require(set(ids) == EXPECTED_AUXILIARY, "auxiliary dataset set drift")
    require(len(ids) == len(set(ids)), "duplicate auxiliary dataset")
    for entry in auxiliary:
        require(entry["bundled_with_software"] is False, f"{entry['id']} became bundled")
        if entry["local_use"] == "blocked_pending_provenance":
            require(entry["acquisition"] == "blocked", f"{entry['id']} blocker bypass")
            require(entry["redistribution"] == "unverified", f"{entry['id']} license guessed")
            require(entry["use_terms_url"] is None, f"{entry['id']} guessed use terms")
            require(entry["attribution"] is None, f"{entry['id']} guessed attribution")
            require(entry["blocker"], f"{entry['id']} has no blocker explanation")
        else:
            require(entry["use_terms_url"], f"{entry['id']} lacks official use terms")
            require(entry["attribution"], f"{entry['id']} lacks frozen attribution")
            require(entry["blocker"] is None, f"{entry['id']} has a stale blocker")

    oh = next(entry for entry in auxiliary if entry["id"].startswith("spivakovsky"))
    require(oh["source"]["doi"] == "10.1029/1999JD901006", "wrong OH DOI")
    require(
        oh["candidate_file"]["sha256"]
        == "c8c362df48a525e240c86fbead7b9e78709a26a330ac050ac7676fb53f7110d7",
        "OH candidate identity drift",
    )

    assets = registry["validation_assets"]
    asset_ids = [entry["id"] for entry in assets]
    require(len(asset_ids) == len(set(asset_ids)), "duplicate validation asset")
    kincaid = next(entry for entry in assets if entry["id"] == "epa-kincaid-sf6/v1")
    require(kincaid["artifact"]["size_bytes"] == 3_129_058, "Kincaid size drift")
    require(
        kincaid["artifact"]["sha256"]
        == "8b645ac7c4dbbc126a315124d4bf7bb8863077b25f06155bca17bc00d2445470",
        "Kincaid SHA drift",
    )

    matrix = registry["formal_matrix"]
    require(matrix["platforms"] == ["ubuntu-24.04-x86_64", "windows-x86_64"], "platform drift")
    require(matrix["worker_counts"] == [1, 4], "worker matrix drift")
    matrix_cells = (
        len(matrix["platforms"])
        * len(matrix["meteorology"])
        * len(matrix["directions"])
        * len(matrix["tasks"])
        * len(matrix["worker_counts"])
    )
    require(matrix_cells == 96, "formal matrix must contain 96 base cells")

    gates = registry["stage_gates"]
    require([entry["stage"] for entry in gates] == [f"m6_a{i}" for i in range(10)], "stage gate order drift")
    blocked = [entry for entry in gates if entry["status"] == "external_blocked"]
    require(len(blocked) == 1 and blocked[0]["stage"] == "m6_a6", "unexpected stage blocker set")
    require(
        registry["a0_disposition"] == "passed_with_a6_oh_external_blocker_declared",
        "A0 disposition drift",
    )

    all_m6_text = "\n".join(
        path.read_text(encoding="utf-8")
        for path in [
            TESTDATA / "M6_PHYSICS_CONTRACT.v1.json",
            TESTDATA / "M6_VALIDATION_ASSETS.v1.json",
            ENGINEERING / "TRAJECTA_M6_A0_SCIENCE_CONTRACT.md",
        ]
    )
    require("10.1029/2000JD900439" not in all_m6_text, "known wrong OH DOI remains")


def validate_checkpoint() -> None:
    checkpoint = load_json("M6_CHECKPOINT_MANIFEST.example.json")
    particle = checkpoint["particle_state"]
    require(
        particle["alive_count"] + particle["terminated_count"]
        == particle["particle_count"],
        "checkpoint particle counts do not close",
    )
    expected_next = checkpoint["boundary"]["macro_step"] + 1
    require(checkpoint["physical_clock"]["next_macro_step"] == expected_next, "clock step drift")
    require(checkpoint["random_state"]["next_macro_step"] == expected_next, "RNG step drift")

    payload = checkpoint["payload"]
    paths = [entry["relative_path"] for entry in payload]
    require(paths == sorted(paths), "checkpoint payload is not sorted")
    require(len(paths) == len(set(paths)), "checkpoint payload path duplicated")
    for raw_path in paths:
        path = PurePosixPath(raw_path)
        require(not path.is_absolute(), f"absolute checkpoint path: {raw_path}")
        require(".." not in path.parts and "\\" not in raw_path, f"escaping checkpoint path: {raw_path}")
        require(path.name != "checkpoint-manifest.json", "checkpoint manifest hashes itself")

    payload_by_path = {entry["relative_path"]: entry for entry in payload}
    database = checkpoint["database"]
    db_payload = payload_by_path[database["relative_path"]]
    require(db_payload["role"] == "database", "checkpoint database role drift")
    require(db_payload["sha256"] == database["sha256"], "checkpoint database SHA mismatch")
    require(db_payload["size_bytes"] == database["size_bytes"], "checkpoint database size mismatch")
    require(database["user_version"] == 2 and database["wal_bytes"] == 0, "checkpoint database not clean v2")

    for state in checkpoint["module_states"]:
        payload_entry = payload_by_path.get(state["relative_path"])
        require(payload_entry is not None, f"module state missing: {state['module_id']}")
        require(payload_entry["role"] == "module_state", "module state payload role drift")
        require(payload_entry["sha256"] == state["sha256"], "module state SHA mismatch")


def validate_process_query() -> None:
    query = load_json("M6_PROCESS_QUERY.example.json")
    filters = query["filters"]
    require(filters["particle_ids"] == sorted(filters["particle_ids"]), "particle filters unsorted")
    require(filters["module_ids"] == sorted(filters["module_ids"]), "module filters unsorted")
    require(filters["substance_ids"] == sorted(filters["substance_ids"]), "substance filters unsorted")
    require(timestamp_key(filters["start"]) <= timestamp_key(filters["end"]), "query time reversed")
    require(query["event_count_returned"] == len(query["events"]), "returned event count mismatch")
    require(query["event_count_returned"] <= query["event_count_total"], "returned count exceeds total")
    require(not query["truncated"], "example unexpectedly truncated")
    for group in query["groups"]:
        require(group["closure"]["kind"] == "forward_mass", "forward query has adjoint closure")
    for event in query["events"]:
        require(event["forward"] is not None and event["backward"] is None, "forward event direction mismatch")


def insert_run(connection: sqlite3.Connection, run_id: str, direction: str) -> None:
    connection.execute(
        """
        INSERT INTO run (
            run_id, job_series_id, attempt, manifest_schema, case_name,
            direction, status, started_seconds, started_nanosecond
        ) VALUES (?, ?, 1, 'trajecta.run-manifest/v1', 'm6-a0-sql', ?, 'running', 0, 0)
        """,
        (run_id, run_id, direction),
    )
    connection.execute(
        """
        INSERT INTO particle (
            run_id, particle_id, population_id, origin_kind, origin_event_id,
            birth_seconds, birth_nanosecond, dry_air_mass_kg
        ) VALUES (?, 0, 'release', 'release', 'source-0', 0, 0, 1.0)
        """,
        (run_id,),
    )


def validate_sqlite_schema() -> None:
    sql = (TESTDATA / "M6_SQLITE_SCHEMA.v2.sql").read_text(encoding="utf-8")
    require("PRAGMA user_version = 2;" in sql, "SQLite v2 pragma missing")
    require("sensitivity_weight" not in sql, "ambiguous v1 sensitivity field leaked into v2")

    connection = sqlite3.connect(":memory:")
    connection.executescript(sql)
    require(connection.execute("PRAGMA user_version").fetchone()[0] == 2, "SQLite user_version drift")
    require(connection.execute("PRAGMA integrity_check").fetchone()[0] == "ok", "empty schema integrity failed")
    tables = {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'"
        )
    }
    require(tables == EXPECTED_SQL_TABLES, "SQLite public table set drift")

    forward = "018f0000-0000-7000-8000-000000000101"
    insert_run(connection, forward, "forward")
    connection.execute(
        "INSERT INTO particle_mass VALUES (?, 0, 'water', 1.0, 1.1)",
        (forward,),
    )
    connection.execute(
        """
        INSERT INTO process_event (
            run_id, event_sequence, particle_id, macro_step, module_id,
            substance_id, physical_seconds, physical_nanosecond,
            direction, detail_kind, mass_delta_kg
        ) VALUES (?, 0, 0, 0, 'water_vapor_exchange', 'water', 0, 0,
                  'forward', 'water_vapor', 0.1)
        """,
        (forward,),
    )
    connection.execute(
        "INSERT INTO water_vapor_event VALUES (?, 0, 0.1, 0.0, 0.0, NULL, NULL, NULL)",
        (forward,),
    )
    connection.execute(
        """
        INSERT INTO process_summary (
            run_id, module_id, substance_id, direction, event_count,
            initial_mass_kg, positive_mass_delta_kg, negative_mass_delta_kg,
            final_mass_kg, closure_residual
        ) VALUES (?, 'water_vapor_exchange', 'water', 'forward', 1,
                  1.0, 0.1, 0.0, 1.1, 0.0)
        """,
        (forward,),
    )

    expect_integrity_error(
        lambda: connection.execute(
            """
            INSERT INTO process_event (
                run_id, event_sequence, particle_id, macro_step, module_id,
                substance_id, physical_seconds, physical_nanosecond,
                direction, detail_kind, mass_delta_kg
            ) VALUES (?, 1, 0, 0, 'water_vapor_exchange', 'water', 1, 0,
                      'forward', 'water_vapor', 0.0)
            """,
            (forward,),
        ),
        "SQLite accepts a duplicate coalescing key",
    )
    connection.rollback()

    backward = "018f0000-0000-7000-8000-000000000102"
    insert_run(connection, backward, "backward")
    connection.execute(
        "INSERT INTO particle_adjoint VALUES (?, 0, 'gas', 1.0, 0.8, 0.2)",
        (backward,),
    )
    connection.execute(
        """
        INSERT INTO process_event (
            run_id, event_sequence, particle_id, macro_step, module_id,
            substance_id, physical_seconds, physical_nanosecond,
            direction, detail_kind, survival_multiplier,
            source_sensitivity, importance_weight
        ) VALUES (?, 0, 0, 0, 'first_order_decay', 'gas', 0, 0,
                  'backward', 'chemistry', 0.8, 0.2, 1.0)
        """,
        (backward,),
    )
    connection.execute(
        "INSERT INTO chemistry_event VALUES (?, 0, 'half_life', 0.1, 0.8, NULL)",
        (backward,),
    )
    expect_integrity_error(
        lambda: connection.execute(
            """
            INSERT INTO process_event (
                run_id, event_sequence, particle_id, macro_step, module_id,
                substance_id, physical_seconds, physical_nanosecond,
                direction, detail_kind, mass_delta_kg
            ) VALUES (?, 1, 0, 1, 'first_order_decay', 'gas', 1, 0,
                      'backward', 'chemistry', -0.1)
            """,
            (backward,),
        ),
        "SQLite accepts mass in a backward process event",
    )
    connection.rollback()

    aerosol = "018f0000-0000-7000-8000-000000000103"
    insert_run(connection, aerosol, "forward")
    expect_integrity_error(
        lambda: connection.execute(
            "INSERT INTO particle_aerosol_property VALUES (?, 0, 'aerosol', 1e-3, 1000.0, 'sphere')",
            (aerosol,),
        ),
        "SQLite accepts aerosol diameter above 100 micrometres",
    )
    connection.rollback()
    require(connection.execute("PRAGMA foreign_key_check").fetchall() == [], "SQLite foreign-key failure")


def validate_science_document() -> None:
    path = ENGINEERING / "TRAJECTA_M6_A0_SCIENCE_CONTRACT.md"
    text = path.read_text(encoding="utf-8")
    required_mentions = [
        "M6_PHYSICS_CONTRACT.v1.json",
        "M6_TOLERANCES.v1.json",
        "M6_VALIDATION_ASSETS.v1.json",
        "trajecta.checkpoint/v1",
        "trajecta.process-query/v1",
        "trajecta.particle-state-sqlite/v2",
        "job resume",
        "result processes",
        "10.1029/1999JD901006",
        "external_blocked",
    ]
    for mention in required_mentions:
        require(mention in text, f"science contract omits {mention}")
    require(len(text.splitlines()) >= 400, "science contract is unexpectedly terse")


def validate_delivery_report_hashes() -> None:
    report = (ENGINEERING / "TRAJECTA_M6_A0_DELIVERY_REPORT.md").read_text(
        encoding="utf-8"
    )
    frozen = dict(re.findall(r"\| `([^`]+)` \| `([0-9a-f]{64})` \|", report))
    expected_paths = {
        "docs/engineering/TRAJECTA_M6_A0_SCIENCE_CONTRACT.md",
        "testdata/M6_PHYSICS_CONTRACT.schema.json",
        "testdata/M6_PHYSICS_CONTRACT.v1.json",
        "testdata/M6_TOLERANCES.schema.json",
        "testdata/M6_TOLERANCES.v1.json",
        "testdata/M6_VALIDATION_ASSETS.schema.json",
        "testdata/M6_VALIDATION_ASSETS.v1.json",
        "testdata/M6_CHECKPOINT_MANIFEST.schema.json",
        "testdata/M6_CHECKPOINT_MANIFEST.example.json",
        "testdata/M6_PROCESS_QUERY.schema.json",
        "testdata/M6_PROCESS_QUERY.example.json",
        "testdata/M6_SQLITE_SCHEMA.v2.sql",
        "tools/validate_m6_a0_contracts.py",
    }
    require(set(frozen) == expected_paths, "delivery report artifact list drift")
    for relative_path, expected_sha in frozen.items():
        actual_sha = hashlib.sha256((ROOT / relative_path).read_bytes()).hexdigest()
        require(actual_sha == expected_sha, f"delivery report SHA drift: {relative_path}")


def validate_file_hygiene() -> None:
    expected_testdata = {
        "M6_CHECKPOINT_MANIFEST.example.json",
        "M6_CHECKPOINT_MANIFEST.schema.json",
        "M6_PHYSICS_CONTRACT.schema.json",
        "M6_PHYSICS_CONTRACT.v1.json",
        "M6_PROCESS_QUERY.example.json",
        "M6_PROCESS_QUERY.schema.json",
        "M6_SQLITE_SCHEMA.v2.sql",
        "M6_TOLERANCES.schema.json",
        "M6_TOLERANCES.v1.json",
        "M6_VALIDATION_ASSETS.schema.json",
        "M6_VALIDATION_ASSETS.v1.json",
        "M6_WILLIS_DEARDORFF_CBL.v1.json",
    }
    actual_testdata = {path.name for path in TESTDATA.glob("M6_*")}
    require(actual_testdata == expected_testdata, "unexpected M6 testdata files")
    files = [
        ENGINEERING / "TRAJECTA_M6_A0_SCIENCE_CONTRACT.md",
        ENGINEERING / "TRAJECTA_M6_A0_DELIVERY_REPORT.md",
        *sorted(TESTDATA.glob("M6_*")),
        ROOT / "tools/validate_m6_a0_contracts.py",
    ]
    for path in files:
        raw = path.read_bytes()
        require(not raw.startswith(b"\xef\xbb\xbf"), f"UTF-8 BOM in {path.name}")
        require(b"\r" not in raw, f"non-LF line ending in {path.name}")
        require(raw.endswith(b"\n"), f"missing final newline in {path.name}")
        text = raw.decode("utf-8")
        require(
            all(line == line.rstrip(" \t") for line in text.splitlines()),
            f"trailing whitespace in {path.name}",
        )


def main() -> int:
    validate_schema_examples()
    validate_versions()
    validate_a1_wiring()
    validate_a2_wiring()
    validate_physics_contract()
    validate_tolerances()
    validate_assets()
    validate_checkpoint()
    validate_process_query()
    validate_sqlite_schema()
    validate_science_document()
    validate_delivery_report_hashes()
    validate_file_hygiene()
    print(
        json.dumps(
            {
                "status": "passed",
                "modules": len(EXPECTED_MODULES),
                "presets": len(EXPECTED_PRESETS),
                "auxiliary_datasets": len(EXPECTED_AUXILIARY),
                "validation_assets": len(load_json("M6_VALIDATION_ASSETS.v1.json")["validation_assets"]),
                "base_matrix_cells": 96,
                "sqlite_user_version": 2,
                "sqlite_tables": len(EXPECTED_SQL_TABLES),
                "production_stage": "m6_a2",
                "a6_oh_status": "external_blocked",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
