#!/usr/bin/env python3
"""Validate the frozen M5-A0 product and control-plane contracts."""

from __future__ import annotations

import copy
import json
import re
import tomllib
from pathlib import Path, PurePosixPath

import yaml
from jsonschema import Draft202012Validator

import run_m5_a4_product_matrix as product_matrix


ROOT = Path(__file__).resolve().parents[1]
TESTDATA = ROOT / "testdata"

EXPECTED_COMMANDS = {
    "config init",
    "config path",
    "config list",
    "config get",
    "config set",
    "config unset",
    "config validate",
    "project init",
    "project status",
    "project show",
    "project get",
    "project set",
    "project unset",
    "project validate",
    "project data-plan",
    "project finalize",
    "case validate",
    "case resolve",
    "data inspect",
    "data lock",
    "met probe",
    "met replay",
    "doctor",
    "run",
    "job list",
    "job status",
    "job wait",
    "job events",
    "job cancel",
    "job rerun",
    "job forget",
    "job prune",
    "result inspect",
    "result verify",
    "result trajectory",
}

EXPECTED_STATES = [
    "queued",
    "starting",
    "running",
    "cancelling",
    "complete",
    "completed_with_particle_errors",
    "failed",
    "cancelled",
    "interrupted",
]

TERMINAL_STATES = {
    "complete",
    "completed_with_particle_errors",
    "failed",
    "cancelled",
    "interrupted",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def load_json(name: str) -> object:
    return json.loads((TESTDATA / name).read_text(encoding="utf-8"))


def validator(schema_name: str) -> Draft202012Validator:
    schema = load_json(schema_name)
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


def validate_schema_examples() -> None:
    pairs = [
        ("M5_CLI_CONTRACT.schema.json", "M5_CLI_CONTRACT.v1.json"),
        ("M5_JOB_CONTRACT.schema.json", "M5_JOB_CONTRACT.v1.json"),
        ("M5_DATA_PLAN.schema.json", "M5_DATA_PLAN.example.json"),
        ("M5_JOB_RECORD.schema.json", "M5_JOB_RECORD.example.json"),
        ("M5_JOB_EVENT.schema.json", "M5_JOB_EVENT.example.json"),
        ("M5_PRUNE_PLAN.schema.json", "M5_PRUNE_PLAN.example.json"),
        ("M5_CLI_OUTPUT.schema.json", "M5_CLI_OUTPUT.example.json"),
        ("M5_CLI_STREAM_ITEM.schema.json", "M5_CLI_STREAM_ITEM.example.json"),
        ("M5_RESULT_INSPECTION.schema.json", "M5_RESULT_INSPECTION.example.json"),
        ("M5_TRAJECTORY_RECORD.schema.json", "M5_TRAJECTORY_RECORD.example.json"),
        ("M5_TRAJECTORY_STREAM.schema.json", "M5_TRAJECTORY_STREAM.example.json"),
        ("M5_BUILD_MANIFEST.schema.json", "M5_BUILD_MANIFEST.example.json"),
        ("M5_PRODUCT_CELL.schema.json", "M5_PRODUCT_CELL.example.json"),
        ("M5_FLEXPART_COMPARISON.schema.json", "M5_FLEXPART_COMPARISON.example.json"),
    ]
    for schema_name, example_name in pairs:
        validator(schema_name).validate(load_json(example_name))

    config = tomllib.loads(
        (TESTDATA / "M5_CONFIG.example.toml").read_text(encoding="utf-8")
    )
    validator("M5_CONFIG.schema.json").validate(config)
    resources = config["resources"]
    require(
        resources["memory_reserve_mib"] < resources["memory_pool_mib"],
        "config memory reserve must be smaller than the explicit pool",
    )
    require(
        config["default_reader_backend"] == "rust",
        "M5 default reader backend must remain rust",
    )
    require(config["daemon"]["local_ipc_only"], "daemon must remain local IPC only")
    for name, template in config["profile_templates"].items():
        execution = template["execution"]
        require(name.strip() != "", "empty profile template name")
        require(
            execution["worker_threads"] <= resources["cpu_slots"],
            f"template {name} oversubscribes the configured CPU pool",
        )

    project = yaml.safe_load(
        (TESTDATA / "M5_PROJECT_INDEX.example.yaml").read_text(encoding="utf-8")
    )
    validator("M5_PROJECT_INDEX.schema.json").validate(project)
    validate_project_index(project)
    validate_build_manifest(load_json("M5_BUILD_MANIFEST.example.json"))


def validate_build_manifest(manifest: dict[str, object]) -> None:
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    require(
        manifest["version"] == workspace["workspace"]["package"]["version"],
        "build-manifest example version must match the workspace",
    )
    build = manifest["build"]
    require(
        build["features"] == ["native-eccodes", "native-netcdf"],
        "product package must contain both native reader features",
    )
    require(build["default_reader_backend"] == "rust", "product default reader drift")
    native = manifest["native_components"]
    require(
        {entry["name"] for entry in native} == {"ecCodes", "netCDF-C", "HDF5"},
        "native product component set drift",
    )
    payload = manifest["payload"]
    paths = [entry["path"] for entry in payload]
    require(paths == sorted(paths), "build-manifest payload is not sorted")
    require(len(paths) == len(set(paths)), "build-manifest payload path duplicated")
    require("BUILD-MANIFEST.json" not in paths, "build manifest cannot hash itself")
    require(
        {entry["role"] for entry in payload}
        >= {
            "binary",
            "documentation",
            "license",
            "sbom",
            "license_inventory",
            "example",
            "native_library",
            "native_data",
        },
        "build-manifest is missing a required payload role",
    )
    require(manifest["binary"]["path"] in paths, "binary missing from payload")
    require(manifest["sbom"]["path"] in paths, "SBOM missing from payload")
    require(
        manifest["license_inventory"]["path"] in paths,
        "license inventory missing from payload",
    )


def validate_product_matrix_definition() -> None:
    platforms = ("windows-x86_64", "linux-x86_64")
    all_cells = []
    for platform in platforms:
        cells = product_matrix.formal_cells(platform)
        all_cells.extend(cells)
        require(len(cells) == 30, f"{platform} product matrix must contain 30 cells")
        require(len({cell.id for cell in cells}) == 30, f"{platform} cell id duplicated")
        require(
            sum(cell.phase == "rust-1k" for cell in cells) == 18,
            f"{platform} Rust 1k phase drift",
        )
        require(
            sum(cell.phase == "rust-10k" for cell in cells) == 6,
            f"{platform} Rust 10k phase drift",
        )
        require(
            sum(cell.phase == "native-1k" for cell in cells) == 6,
            f"{platform} native 1k phase drift",
        )
        require(
            {
                (cell.family, cell.population, cell.direction)
                for cell in cells
                if cell.phase == "rust-1k"
            }
            == {
                (family, population, direction)
                for family in product_matrix.FAMILIES
                for population in product_matrix.POPULATIONS
                for direction in product_matrix.DIRECTIONS
            },
            f"{platform} Rust 1k coverage drift",
        )
        require(
            all(
                cell.backend == "native" and cell.population == "release"
                for cell in cells
                if cell.phase == "native-1k"
            ),
            f"{platform} native phase semantics drift",
        )
    require(len(all_cells) == 60, "cross-platform product matrix must contain 60 cells")
    require(len({cell.id for cell in all_cells}) == 60, "cross-platform cell id duplicated")
    require(
        {family: len(definition.files) for family, definition in product_matrix.FAMILIES.items()}
        == {"era5-pressure": 2, "era5-hybrid": 2, "cfsr-pressure": 4},
        "A4 four-frame fixture definition drift",
    )
    require(
        all(
            family.forward_start != family.backward_start
            for family in product_matrix.FAMILIES.values()
        ),
        "A4 direction-specific start times collapsed",
    )


def validate_project_index(project: dict[str, object]) -> None:
    cases = project["cases"]
    profiles = project["profiles"]
    require(isinstance(cases, dict), "project cases must be a map")
    require(isinstance(profiles, dict), "project profiles must be a map")
    for path in list(cases.values()) + [entry["path"] for entry in profiles.values()]:
        require(isinstance(path, str), "project path must be text")
        require("\\" not in path, f"project path must use portable '/' separators: {path}")
        pure = PurePosixPath(path)
        require(not pure.is_absolute(), f"project path must be relative: {path}")
        require(
            path not in {"", "."} and ".." not in pure.parts,
            f"project path must be normalized and contained: {path}",
        )
    default_profile = project.get("default_profile")
    require(
        default_profile is None or default_profile in profiles,
        "default_profile does not name an indexed profile",
    )
    require(
        len(set(cases.values())) == len(cases),
        "multiple Case names point to the same path",
    )
    profile_paths = [entry["path"] for entry in profiles.values()]
    require(
        len(set(profile_paths)) == len(profile_paths),
        "multiple Profile names point to the same path",
    )


def validate_cli_contract() -> None:
    contract = load_json("M5_CLI_CONTRACT.v1.json")
    commands = contract["commands"]
    paths = [entry["path"] for entry in commands]
    require(len(paths) == len(set(paths)), "duplicate CLI command path")
    require(set(paths) == EXPECTED_COMMANDS, "CLI command surface drift")
    require(contract["formats"] == ["human", "json", "jsonl"], "format drift")
    require(
        contract["global_options"]
        == {
            "format": "--format",
            "json_alias": "--json",
            "config": "--config",
            "project": "--project",
        },
        "global option drift",
    )
    require(contract["run"]["default_mode"] == "foreground_wait", "run is not foreground by default")
    require(contract["run"]["detach_flag"] == "--detach", "detach flag drift")
    require(
        contract["run"]["foreground_survives_client_disconnect"] is True,
        "client disconnect must not implicitly cancel a worker",
    )
    require(
        contract["exit_codes"]
        == {
            "complete": 0,
            "non_success_terminal": 1,
            "usage": 2,
            "detached_accepted": 0,
        },
        "CLI exit-code contract drift",
    )


def validate_job_contract() -> None:
    contract = load_json("M5_JOB_CONTRACT.v1.json")
    require(contract["states"] == EXPECTED_STATES, "job state order or membership drift")
    require(set(contract["terminal_states"]) == TERMINAL_STATES, "terminal-state drift")
    transitions = contract["transitions"]
    require(set(transitions) == set(EXPECTED_STATES), "transition source-state drift")
    for state, targets in transitions.items():
        require(len(targets) == len(set(targets)), f"duplicate transition from {state}")
        require(set(targets) <= set(EXPECTED_STATES), f"unknown transition target from {state}")
        if state in TERMINAL_STATES:
            require(targets == [], f"terminal state {state} has an outgoing transition")
    require(contract["automatic_retry"] is False, "automatic retry must remain disabled")
    require(
        contract["cancellation"]
        == {
            "before_run_manifest": "catalog_only_cancelled_no_artifacts",
            "after_run_manifest": "safe_boundary_cancelled_with_provenance",
            "force": "interrupted_forensic",
        },
        "cancellation artifact boundary drift",
    )
    require(contract["scheduling"]["maximum_head_bypass"] == 3, "head bypass drift")
    require(contract["events"]["destructive_read"] is False, "events became destructive")
    pruning = contract["pruning"]
    require(pruning["mode"] == "dry_run", "pruning is not dry-run")
    require(pruning["delete_enabled"] is False, "M5 must not delete artifacts")
    require(
        pruning["forget_deletes_artifacts"] is False,
        "job forget must not delete artifacts",
    )


def timestamp_key(value: dict[str, int]) -> tuple[int, int]:
    return value["seconds_since_unix_epoch"], value["nanosecond"]


def validate_data_plan() -> None:
    plan = load_json("M5_DATA_PLAN.example.json")
    requirements = plan["requirements"]
    keys = [
        (entry["profile_name"], entry["case_name"], entry["dataset_id"])
        for entry in requirements
    ]
    require(keys == sorted(keys), "data-plan requirements are not deterministically sorted")
    require(len(keys) == len(set(keys)), "duplicate data-plan requirement")
    for entry in requirements:
        capabilities = entry["required_capabilities"]
        require(capabilities == sorted(capabilities), "capabilities are not sorted")
        require(
            timestamp_key(entry["coverage_start"]) <= timestamp_key(entry["coverage_end"]),
            "data-plan coverage is reversed",
        )


def validate_prune_plan() -> None:
    plan = load_json("M5_PRUNE_PLAN.example.json")
    require(plan["mode"] == "dry_run", "prune example is not dry-run")
    require(plan["delete_enabled"] is False, "prune example enables deletion")
    for candidate in plan["candidates"]:
        require(
            candidate["superseded_by"] != candidate["run_id"],
            "prune candidate supersedes itself",
        )
        if candidate["protected"]:
            require(
                candidate["reason"] == "artifact_missing",
                "only missing superseded artifacts may remain protected candidates",
            )
        else:
            require(
                candidate["reason"] == "superseded_by_verified_complete_attempt",
                "eligible prune candidate lacks verified-successor reason",
            )


def validate_manifest_revision() -> None:
    schema = load_json("M4_RUN_MANIFEST.schema.json")
    manifest_validator = Draft202012Validator(schema)
    Draft202012Validator.check_schema(schema)
    statuses = schema["properties"]["status"]["enum"]
    require("cancelled" in statuses, "manifest v1 lacks cancelled")
    require("interrupted" in statuses, "manifest v1 lacks interrupted")
    require("job_series_id" in schema["required"], "manifest v1 lacks job_series_id")
    require("attempt" in schema["required"], "manifest v1 lacks attempt")
    require(schema["properties"]["attempt"]["minimum"] == 1, "attempt is not one-based")

    sha0 = "0" * 64
    sha1 = "1" * 64
    running = {
        "schema_version": "trajecta.run-manifest/v1",
        "job_series_id": "018f0000-0000-7000-8000-000000000000",
        "attempt": 1,
        "run_id": "018f0000-0000-7000-8000-000000000001",
        "case_name": "m5-a0-contract",
        "status": "running",
        "started_at": {"seconds_since_unix_epoch": 0, "nanosecond": 0},
        "software": {"crate_versions": {"trajecta-core": "0.0.0"}},
        "inputs": {
            "case_sha256": sha0,
            "run_profile_sha256": sha1,
            "dataset_lock_sha256": {},
            "dataset_profile_sha256": {},
            "dataset_content_sha256": {},
        },
        "execution": {
            "worker_threads": 1,
            "memory_budget_bytes": 1,
            "executor": "cpu",
            "reader_backends": {},
            "io_counters": {},
        },
        "numerical": {
            "random_seed": 0,
            "integrator": "rk2_spherical/v0",
            "boundary_policies": [],
            "population": "release_driven/v1",
            "particle_state_sink": "particle_state_sqlite/v1",
            "tolerance_registry": "trajecta.m4.numerical-contract/v1",
            "tolerances": {"domain_fill_step_relative": 1e-12},
            "deterministic": True,
        },
        "geometries": [],
        "effective_outputs": [
            {
                "product": "particle_state/v1",
                "schedule": {"mode": "endpoints"},
                "sink": {"model": "particle_state_sqlite/v1"},
            }
        ],
        "sqlite": {
            "schema_version": 1,
            "relative_path": "particles.sqlite",
            "journal_mode": "WAL",
            "synchronous": "NORMAL",
            "row_counts": {},
        },
        "terminations": {"normal_count": 0, "abnormal_count": 0, "by_reason": {}},
        "mass_ledger": [],
    }
    manifest_validator.validate(running)
    invalid_attempt = copy.deepcopy(running)
    invalid_attempt["attempt"] = 0
    require(not manifest_validator.is_valid(invalid_attempt), "manifest accepts attempt zero")

    provenance = {
        "schema_version": "trajecta.provenance-bundle/v1",
        "relative_path": "provenance-bundle.json",
        "sha256": sha0,
        "sqlite_sha256": sha1,
        "content_sha256": sha0,
        "sqlite_sql_sha256": sha1,
        "canonical_output_sha256": sha0,
        "record_count": 0,
        "field_set_count": 0,
        "sample_count": 0,
    }
    cancelled = copy.deepcopy(running)
    cancelled["status"] = "cancelled"
    cancelled["finished_at"] = {"seconds_since_unix_epoch": 1, "nanosecond": 0}
    cancelled["provenance"] = provenance
    manifest_validator.validate(cancelled)
    without_provenance = copy.deepcopy(cancelled)
    del without_provenance["provenance"]
    require(
        not manifest_validator.is_valid(without_provenance),
        "cancelled manifest accepts missing provenance",
    )

    interrupted = copy.deepcopy(running)
    interrupted["status"] = "interrupted"
    interrupted["finished_at"] = {"seconds_since_unix_epoch": 1, "nanosecond": 0}
    interrupted["failure"] = {
        "code": "run.interrupted",
        "message": "worker disappeared before safe finalization",
    }
    manifest_validator.validate(interrupted)
    interrupted["provenance"] = provenance
    require(
        not manifest_validator.is_valid(interrupted),
        "interrupted manifest accepts provenance",
    )


def validate_known_placeholders() -> int:
    allowed = {
        "crates/trajecta-cli/src/cli.rs",
        "crates/trajecta-cli/src/lib.rs",
        "crates/trajecta-cli/src/render.rs",
    }
    count = 0
    pattern = re.compile(r"NotImplemented|not implemented")
    source_roots = [ROOT / "crates/trajecta-cli", ROOT / "crates/trajecta-job"]
    for source_root in source_roots:
        for path in source_root.rglob("*.rs"):
            relative = path.relative_to(ROOT).as_posix()
            matches = pattern.findall(path.read_text(encoding="utf-8"))
            if not matches:
                continue
            require(
                relative in allowed,
                f"new reachable placeholder outside allowlist: {relative}",
            )
            count += len(matches)
    require(count <= 11, "known CLI placeholder count increased")
    return count


def reject_version_sprawl() -> None:
    for name in [
        "M5_CLI_CONTRACT.v1.json",
        "M5_JOB_CONTRACT.v1.json",
        "M5_CONFIG.example.toml",
        "M5_PROJECT_INDEX.example.yaml",
        "M5_DATA_PLAN.example.json",
        "M5_JOB_RECORD.example.json",
        "M5_JOB_EVENT.example.json",
        "M5_PRUNE_PLAN.example.json",
        "M5_CLI_OUTPUT.example.json",
        "M5_CLI_STREAM_ITEM.example.json",
    ]:
        text = (TESTDATA / name).read_text(encoding="utf-8")
        require("/v2" not in text, f"unexpected v2 contract in {name}")


def main() -> int:
    validate_schema_examples()
    validate_product_matrix_definition()
    validate_cli_contract()
    validate_job_contract()
    validate_data_plan()
    validate_prune_plan()
    validate_manifest_revision()
    placeholder_count = validate_known_placeholders()
    reject_version_sprawl()
    print(
        json.dumps(
            {
                "status": "passed",
                "cli_commands": len(EXPECTED_COMMANDS),
                "job_states": len(EXPECTED_STATES),
                "manifest_schema_version": "trajecta.run-manifest/v1",
                "pruning_mode": "dry_run",
                "known_cli_placeholders": placeholder_count,
                "product_cells_per_platform": 30,
                "product_cells_total": 60,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
