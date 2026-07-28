#!/usr/bin/env python3
"""Execute the frozen M4-A0 JSON and SQLite machine contracts."""

from __future__ import annotations

import hashlib
import json
import sqlite3
import tempfile
from pathlib import Path

from jsonschema import Draft202012Validator


ROOT = Path(__file__).resolve().parents[1]
TESTDATA = ROOT / "testdata"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def load_json(name: str) -> object:
    return json.loads((TESTDATA / name).read_text(encoding="utf-8"))


def canonical_json_bytes(value: object) -> bytes:
    """RFC-8785-equivalent bytes for v1 hash inputs (objects/strings/null only)."""
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    ).encode("utf-8")


def sha256_canonical(value: object) -> str:
    return hashlib.sha256(canonical_json_bytes(value)).hexdigest()


def validate_provenance_semantics(bundle: dict[str, object]) -> None:
    records = bundle["records"]
    field_sets = bundle["field_sets"]
    samples = bundle["samples"]
    require(isinstance(records, list), "provenance records are not an array")
    require(isinstance(field_sets, list), "provenance field_sets are not an array")
    require(isinstance(samples, list), "provenance samples are not an array")

    record_hashes = [entry["sha256"] for entry in records]
    require(record_hashes == sorted(record_hashes), "records are not SHA-sorted")
    require(len(record_hashes) == len(set(record_hashes)), "duplicate record SHA")
    record_by_hash: dict[str, dict[str, object]] = {}
    for entry in records:
        expected = sha256_canonical(entry["record"])
        require(entry["sha256"] == expected, f"record SHA mismatch: {entry['sha256']}")
        record = entry["record"]
        record_by_hash[expected] = record
        for transform in record["transforms"]:
            parameters = transform["parameters"]
            keys = [(item["name"], item["value"]) for item in parameters]
            require(keys == sorted(keys), f"unsorted transform parameters in {expected}")
            require(
                len({item["name"] for item in parameters}) == len(parameters),
                f"duplicate transform parameter name in {expected}",
            )

    set_hashes = [entry["sha256"] for entry in field_sets]
    require(set_hashes == sorted(set_hashes), "field_sets are not SHA-sorted")
    require(len(set_hashes) == len(set(set_hashes)), "duplicate field-set SHA")
    field_set_by_hash: dict[str, dict[str, object]] = {}
    expected_slots = {
        "eastward_wind": "eastward_wind",
        "northward_wind": "northward_wind",
        "geometric_vertical_velocity": "geometric_vertical_velocity",
        "air_pressure": "air_pressure",
        "air_temperature": "air_temperature",
    }
    for entry in field_sets:
        expected = sha256_canonical(entry["fields"])
        require(entry["sha256"] == expected, f"field-set SHA mismatch: {entry['sha256']}")
        fields = entry["fields"]
        field_set_by_hash[expected] = fields
        for slot, expected_field in expected_slots.items():
            reference = fields[slot]
            if reference is None:
                continue
            require(reference in record_by_hash, f"unknown record reference: {reference}")
            require(
                record_by_hash[reference]["field"] == expected_field,
                f"slot {slot} points at field {record_by_hash[reference]['field']}",
            )

    sample_keys = [(entry["particle_id"], entry["sample_sequence"]) for entry in samples]
    require(sample_keys == sorted(sample_keys), "samples are not primary-key sorted")
    require(len(sample_keys) == len(set(sample_keys)), "duplicate sample assignment")
    for entry in samples:
        require(
            entry["field_set_sha256"] in field_set_by_hash,
            f"unknown field-set reference: {entry['field_set_sha256']}",
        )


def require_provenance_rejected(bundle: dict[str, object], label: str) -> None:
    try:
        validate_provenance_semantics(bundle)
    except RuntimeError:
        return
    raise RuntimeError(f"provenance negative fixture unexpectedly accepted: {label}")


def validate_json_contracts() -> None:
    numerical_schema = load_json("M4_NUMERICAL_CONTRACT.schema.json")
    manifest_schema = load_json("M4_RUN_MANIFEST.schema.json")
    provenance_schema = load_json("M4_PROVENANCE_BUNDLE.schema.json")
    numerical = load_json("M4_NUMERICAL_CONTRACT.v1.json")
    provenance = load_json("M4_PROVENANCE_BUNDLE.example.json")

    Draft202012Validator.check_schema(numerical_schema)
    Draft202012Validator.check_schema(manifest_schema)
    Draft202012Validator.check_schema(provenance_schema)
    Draft202012Validator(numerical_schema).validate(numerical)
    Draft202012Validator(provenance_schema).validate(provenance)
    validate_provenance_semantics(provenance)
    bad_record_hash = json.loads(json.dumps(provenance))
    bad_record_hash["records"][0]["sha256"] = "0" * 64
    require_provenance_rejected(bad_record_hash, "record hash mismatch")
    duplicate_sample = json.loads(json.dumps(provenance))
    duplicate_sample["samples"].append(duplicate_sample["samples"][0])
    require_provenance_rejected(duplicate_sample, "duplicate sample")
    wrong_slot = json.loads(json.dumps(provenance))
    full_set = next(
        entry for entry in wrong_slot["field_sets"] if entry["fields"]["eastward_wind"]
    )
    full_set["fields"]["eastward_wind"] = full_set["fields"]["northward_wind"]
    full_set["sha256"] = sha256_canonical(full_set["fields"])
    wrong_slot["field_sets"].sort(key=lambda entry: entry["sha256"])
    require_provenance_rejected(wrong_slot, "field-set slot mismatch")

    sha0 = "0" * 64
    sha1 = "1" * 64
    running_manifest = {
        "schema_version": "trajecta.run-manifest/v1",
        "job_series_id": "018f0000-0000-7000-8000-000000000000",
        "attempt": 1,
        "run_id": "018f0000-0000-7000-8000-000000000000",
        "case_name": "m4-a0-contract",
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
    Draft202012Validator(manifest_schema).validate(running_manifest)
    terminal_manifest = json.loads(json.dumps(running_manifest))
    terminal_manifest["status"] = "complete"
    terminal_manifest["finished_at"] = {
        "seconds_since_unix_epoch": 1,
        "nanosecond": 0,
    }
    terminal_manifest["provenance"] = {
        "schema_version": "trajecta.provenance-bundle/v1",
        "relative_path": "provenance-bundle.json",
        "sha256": sha0,
        "sqlite_sha256": sha1,
        "content_sha256": sha0,
        "sqlite_sql_sha256": sha1,
        "canonical_output_sha256": sha0,
        "record_count": len(provenance["records"]),
        "field_set_count": len(provenance["field_sets"]),
        "sample_count": len(provenance["samples"]),
    }
    Draft202012Validator(manifest_schema).validate(terminal_manifest)
    missing_bundle = json.loads(json.dumps(terminal_manifest))
    del missing_bundle["provenance"]
    require(
        not Draft202012Validator(manifest_schema).is_valid(missing_bundle),
        "terminal manifest unexpectedly accepts a missing provenance bundle",
    )


def validate_sqlite_contract() -> None:
    schema = (TESTDATA / "M4_SQLITE_SCHEMA.v1.sql").read_text(encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="trajecta-m4-a0-") as temporary:
        database = Path(temporary) / "particles.sqlite"
        connection = sqlite3.connect(database)
        try:
            connection.executescript(schema)
            tables = {
                row[0]
                for row in connection.execute(
                    "SELECT name FROM sqlite_schema WHERE type = 'table'"
                )
            }
            indexes = {
                row[0]
                for row in connection.execute(
                    "SELECT name FROM sqlite_schema WHERE type = 'index'"
                )
            }
            expected_tables = {
                "run",
                "particle",
                "particle_mass",
                "output_event",
                "particle_state",
                "termination",
            }
            expected_indexes = {"particle_state_by_time", "output_event_by_time"}
            require(expected_tables <= tables, f"missing tables: {expected_tables - tables}")
            require(expected_indexes <= indexes, f"missing indexes: {expected_indexes - indexes}")
            require(
                connection.execute("PRAGMA user_version").fetchone()[0] == 1,
                "unexpected SQLite user_version",
            )
            require(
                connection.execute("PRAGMA journal_mode").fetchone()[0].lower() == "wal",
                "SQLite did not enter WAL mode",
            )
            require(
                connection.execute("PRAGMA synchronous").fetchone()[0] == 1,
                "SQLite synchronous mode is not NORMAL",
            )
            require(
                connection.execute("PRAGMA foreign_keys").fetchone()[0] == 1,
                "SQLite foreign keys are disabled",
            )
            require(
                connection.execute("PRAGMA integrity_check").fetchone()[0] == "ok",
                "SQLite integrity_check failed",
            )
        finally:
            connection.close()


def main() -> int:
    validate_json_contracts()
    validate_sqlite_contract()
    print(
        json.dumps(
            {
                "status": "passed",
                "json_schema_draft": "2020-12",
                "provenance_schema_version": "trajecta.provenance-bundle/v1",
                "sqlite_user_version": 1,
                "sqlite_journal_mode": "WAL",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
