#!/usr/bin/env python3
"""Run the frozen M5-A4 clean-package smoke and product matrix."""

from __future__ import annotations

import argparse
from contextlib import closing
import json
import math
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import m5_a4_package as package


ROOT = Path(__file__).resolve().parents[1]
CELL_RESULT_NAME = "M5_A4_PRODUCT_CELL.json"
MATRIX_SUMMARY_NAME = "M5_A4_PRODUCT_MATRIX_SUMMARY.json"
PREFLIGHT_NAME = "M5_A4_CLEAN_EXTRACTION_PREFLIGHT.json"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
UUID7_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


class ProductError(RuntimeError):
    """A stable M5-A4 product execution failure."""


class FixtureUnavailable(ProductError):
    """A required frozen external input is not locally available."""


class CommandFailure(ProductError):
    """A packaged CLI command failed or returned invalid machine output."""


@dataclass(frozen=True)
class Family:
    id: str
    profile: str
    forward_start: int
    backward_start: int
    periodic_longitude: bool
    release_coordinates: tuple[float, float]
    files: tuple[tuple[str, str], ...]


FAMILIES = {
    "era5-pressure": Family(
        id="era5-pressure",
        profile="era5-cf-pressure-netcdf-v0",
        forward_start=1_543_644_000,
        backward_start=1_543_665_600,
        periodic_longitude=False,
        release_coordinates=(5.0, 49.0),
        files=(
            (
                "era5_pressure_20181201.nc",
                "9d1b2a64aa01acd950b091ac83cc69bbdcc43b5e6c0eba478787703828db167e",
            ),
            (
                "era5_surface_20181201.nc",
                "0f438e28a084b313939fdde0ee5e91ebf218d7fbf0a9d12c0552135eec872969",
            ),
        ),
    ),
    "era5-hybrid": Family(
        id="era5-hybrid",
        profile="era5-cds-hybrid137-v0",
        forward_start=1_543_633_200,
        backward_start=1_543_644_000,
        periodic_longitude=False,
        release_coordinates=(5.0, 49.0),
        files=(
            (
                "era5_hybrid137_prepared_20181201.nc",
                "eb8f8554c2ed87e0a49d8a7e5ddf52b7337c96461db7c650a8d890e3f400e0e6",
            ),
            (
                "era5_surface_20181201.nc",
                "3c21ad12b54e559331710ada547d4fed6cdf8ece1450d7c92ec364691b51a730",
            ),
        ),
    ),
    "cfsr-pressure": Family(
        id="cfsr-pressure",
        profile="cfsr-pgbl-pressure-v0",
        forward_start=1_230_789_600,
        backward_start=1_230_811_200,
        periodic_longitude=True,
        release_coordinates=(0.0, 0.0),
        files=(
            (
                "pgbl00.gdas.2009010100.grb2",
                "fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c",
            ),
            (
                "pgbl00.gdas.2009010106.grb2",
                "00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b",
            ),
            (
                "pgbl00.gdas.2009010112.grb2",
                "f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5",
            ),
            (
                "pgbl00.gdas.2009010118.grb2",
                "a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade",
            ),
        ),
    ),
}

POPULATIONS = ("release", "air-mass", "ozone")
DIRECTIONS = ("forward", "backward")
EXPECTED_POPULATION_MODEL = {
    "release": "release_driven/v1",
    "air-mass": "dry_air_domain_fill/v1",
    "ozone": "stratospheric_ozone_domain_fill/v1",
}
RANDOM_SEEDS = {"release": 4_201, "air-mass": 4_202, "ozone": 4_203}


@dataclass(frozen=True)
class Cell:
    platform: str
    phase: str
    family: str
    population: str
    direction: str
    backend: str
    particles: int
    worker_threads: int

    @property
    def id(self) -> str:
        return (
            f"{self.platform}__{self.family}__{self.population}__{self.direction}"
            f"__{self.backend}__p{self.particles}__w{self.worker_threads}"
        )

    def as_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "platform": self.platform,
            "phase": self.phase,
            "family": self.family,
            "population": self.population,
            "direction": self.direction,
            "backend": self.backend,
            "particles": self.particles,
            "worker_threads": self.worker_threads,
        }


@dataclass(frozen=True)
class ProductIdentity:
    package_root: Path
    binary: Path
    platform: str
    source_tree_sha256: str
    build_manifest_sha256: str
    binary_sha256: str


def utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def host_platform() -> str:
    if sys.platform == "win32":
        return "windows-x86_64"
    if sys.platform.startswith("linux"):
        return "linux-x86_64"
    raise ProductError(f"unsupported M5-A4 execution host: {sys.platform}")


def is_within(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def formal_cells(platform_label: str) -> list[Cell]:
    cells = [
        Cell(platform_label, "rust-1k", family, population, direction, "rust", 1_000, 1)
        for family in FAMILIES
        for population in POPULATIONS
        for direction in DIRECTIONS
    ]
    cells.extend(
        [
            Cell(platform_label, "rust-10k", "era5-pressure", "release", "forward", "rust", 10_000, 4),
            Cell(platform_label, "rust-10k", "era5-pressure", "air-mass", "backward", "rust", 10_000, 4),
            Cell(platform_label, "rust-10k", "era5-hybrid", "air-mass", "forward", "rust", 10_000, 4),
            Cell(platform_label, "rust-10k", "era5-hybrid", "ozone", "backward", "rust", 10_000, 4),
            Cell(platform_label, "rust-10k", "cfsr-pressure", "ozone", "forward", "rust", 10_000, 4),
            Cell(platform_label, "rust-10k", "cfsr-pressure", "release", "backward", "rust", 10_000, 4),
        ]
    )
    cells.extend(
        Cell(platform_label, "native-1k", family, "release", direction, "native", 1_000, 1)
        for family in FAMILIES
        for direction in DIRECTIONS
    )
    return cells


def smoke_cells(platform_label: str) -> list[Cell]:
    return [
        Cell(platform_label, "rust-1k", "cfsr-pressure", "release", "forward", "rust", 1_000, 1),
        Cell(platform_label, "native-1k", "cfsr-pressure", "release", "backward", "native", 1_000, 1),
    ]


def validate_product_root(root: Path) -> ProductIdentity:
    root = root.resolve()
    if not root.is_dir():
        raise FixtureUnavailable(f"clean-extracted package root is missing: {root}")
    if is_within(root, ROOT.resolve()):
        raise ProductError("clean-extracted package root must be outside the source tree")
    try:
        manifest = package.validate_build_manifest(root)
    except package.PackageError as error:
        raise ProductError(str(error)) from error
    platform_label = manifest["platform"]["label"]
    if platform_label != host_platform():
        raise ProductError(
            f"package platform {platform_label} does not match execution host {host_platform()}"
        )
    binary = root / Path(manifest["binary"]["path"])
    return ProductIdentity(
        package_root=root,
        binary=binary,
        platform=platform_label,
        source_tree_sha256=manifest["source"]["source_tree_sha256"],
        build_manifest_sha256=package.sha256(root / package.MANIFEST_NAME),
        binary_sha256=manifest["binary"]["sha256"],
    )


def validate_fixture(family: Family, root: Path) -> dict[str, str]:
    root = root.resolve()
    if not root.is_dir():
        raise FixtureUnavailable(f"{family.id} fixture directory is missing: {root}")
    identities: dict[str, str] = {}
    for name, expected in family.files:
        path = root / name
        if not path.is_file():
            raise FixtureUnavailable(f"{family.id} fixture file is missing: {path}")
        actual = package.sha256(path)
        if actual != expected:
            raise ProductError(
                f"{family.id} fixture identity mismatch for {name}: {actual} != {expected}"
            )
        identities[name] = actual
    return identities


def case_document(cell: Cell, duration_seconds: int = 600) -> dict[str, Any]:
    family = FAMILIES[cell.family]
    start = family.forward_start if cell.direction == "forward" else family.backward_start
    end = start + (
        duration_seconds if cell.direction == "forward" else -duration_seconds
    )
    domain_id = "global" if family.periodic_longitude else "limited"
    if cell.population == "release":
        population: dict[str, Any] = {
            "strategy": "release_driven",
            "id": "release",
            "events": [
                {
                    "id": "event",
                    "start": {"seconds_since_unix_epoch": start, "nanosecond": 0},
                    "end": {"seconds_since_unix_epoch": start, "nanosecond": 0},
                    "particle_count": cell.particles,
                    "mass": {"tracer": {"value": 1, "unit": "kg"}},
                    "geometry": {
                        "source": "inline",
                        "geometry": {
                            "type": "Point",
                            "coordinates": list(family.release_coordinates),
                        },
                    },
                    "vertical": {
                        "coordinate": "above_sea_level",
                        "lower": {"value": 1_000, "unit": "m"},
                    },
                }
            ],
        }
        substances = [{"id": "tracer", "display_name": "Tracer"}]
    elif cell.population == "air-mass":
        population = {
            "strategy": "domain_fill_air_mass",
            "id": "air",
            "domain_id": domain_id,
            "target_particle_count": cell.particles,
        }
        substances = []
    elif cell.population == "ozone":
        population = {
            "strategy": "domain_fill_stratospheric_ozone",
            "air_mass": {
                "id": "ozone",
                "domain_id": domain_id,
                "target_particle_count": cell.particles,
            },
            "ozone_rule": "flexpart_stratospheric_ozone_pv60/v1",
            "ozone_substance": "ozone",
        }
        substances = [{"id": "ozone", "display_name": "Ozone"}]
    else:
        raise ProductError(f"unknown product population: {cell.population}")
    horizontal_policy = (
        "global_periodic/v0" if family.periodic_longitude else "limited_domain_terminate/v0"
    )
    return {
        "schema_version": 0,
        "kind": "case",
        "metadata": {"name": f"m5-a4-{cell.family}-{cell.population}-{cell.direction}"},
        "time": {
            "start": {"seconds_since_unix_epoch": start, "nanosecond": 0},
            "end": {"seconds_since_unix_epoch": end, "nanosecond": 0},
            "direction": cell.direction,
        },
        "meteorology": {
            "domains": [
                {
                    "id": domain_id,
                    "dataset": cell.family,
                    "priority": 1,
                    "horizontal_halo_cells": 1,
                }
            ]
        },
        "particle_population": population,
        "substances": substances,
        "numerics": {
            "time_step": {"value": 300, "unit": "s"},
            "integrator": {"model": "rk2_spherical/v0"},
            "boundaries": {
                "policies": [
                    "surface_reflect/v0",
                    "model_top_terminate/v0",
                    horizontal_policy,
                ]
            },
            "random_seed": RANDOM_SEEDS[cell.population],
        },
        "outputs": [
            {
                "product": "particle_state/v1",
                "schedule": {"mode": "endpoints"},
                "sink": {"model": "particle_state_sqlite/v1"},
            }
        ],
    }


def profile_document(cell: Cell) -> dict[str, Any]:
    return {
        "schema_version": 0,
        "kind": "run_profile",
        "metadata": {"name": "product"},
        "case_path": "../cases/cell.json",
        "output_root": "../runs",
        "datasets": [
            {
                "dataset": cell.family,
                "lockfile": f"../locks/{cell.family}-{cell.backend}.lock.json",
                "data_roots": {"met": "../data"},
                "reader_backend": cell.backend,
            }
        ],
        "execution": {
            "worker_threads": cell.worker_threads,
            "memory_budget_bytes": 2_147_483_648 if cell.particles == 10_000 else 1_073_741_824,
            "executor": "cpu",
            "meteorology_reader": cell.backend,
        },
    }


def project_document(cell: Cell) -> dict[str, Any]:
    return {
        "schema_version": "trajecta.project-index/v1",
        "name": f"m5-a4-{cell.id}",
        "cases": {"cell": "cases/cell.json"},
        "profiles": {
            "product": {
                "path": "profiles/product.json",
                "dataset_profiles": {cell.family: FAMILIES[cell.family].profile},
            }
        },
        "default_profile": "product",
    }


def prepare_project(
    root: Path, cell: Cell, data_root: Path, duration_seconds: int = 600
) -> Path:
    project_root = root / "project"
    for directory in ("cases", "profiles", "locks", "runs", "data"):
        (project_root / directory).mkdir(parents=True, exist_ok=False)
    for name, _ in FAMILIES[cell.family].files:
        source = data_root.resolve() / name
        destination = project_root / "data" / name
        try:
            os.link(source, destination)
        except OSError:
            shutil.copy2(source, destination)
    package.write_json(
        project_root / "cases" / "cell.json", case_document(cell, duration_seconds)
    )
    package.write_json(project_root / "profiles" / "product.json", profile_document(cell))
    package.write_json(project_root / "trajecta-project.yaml", project_document(cell))
    return project_root


def clean_environment(identity: ProductIdentity, root: Path) -> dict[str, str]:
    environment = os.environ.copy()
    blocked_prefixes = (
        "CARGO",
        "RUST",
        "TRAJECTA",
        "ECCODES",
        "NETCDF",
        "HDF5",
        "PKG_CONFIG",
        "LIBCLANG",
        "DYLD",
    )
    for key in list(environment):
        if key.upper().startswith(blocked_prefixes) or key.upper() in {
            "LIBRARY_PATH",
            "LD_LIBRARY_PATH",
        }:
            environment.pop(key, None)
    home = root / "home"
    home.mkdir(parents=True, exist_ok=True)
    environment["HOME"] = str(home)
    if sys.platform == "win32":
        temporary = root / "tmp"
        temporary.mkdir(parents=True, exist_ok=True)
        system_root = Path(environment.get("SystemRoot", r"C:\Windows"))
        environment["PATH"] = os.pathsep.join(
            [str(identity.package_root), str(system_root / "System32"), str(system_root)]
        )
        environment["USERPROFILE"] = str(home)
        environment["LOCALAPPDATA"] = str(home / "AppData" / "Local")
        environment["APPDATA"] = str(home / "AppData" / "Roaming")
        environment["TEMP"] = str(temporary)
        environment["TMP"] = str(temporary)
    else:
        environment["PATH"] = os.pathsep.join([str(identity.package_root), "/usr/bin", "/bin"])
        environment["TMPDIR"] = "/tmp"
        environment["XDG_CONFIG_HOME"] = str(home / ".config")
        environment["XDG_DATA_HOME"] = str(home / ".local" / "share")
    definitions = identity.package_root / "share" / "eccodes" / "definitions"
    if definitions.is_dir() and not (definitions / "EMBEDDED-MEMFS.txt").is_file():
        environment["ECCODES_DEFINITION_PATH"] = str(definitions)
    return environment


class CommandRecorder:
    def __init__(self, identity: ProductIdentity, root: Path, timeout_seconds: int) -> None:
        self.binary = identity.binary
        self.root = root
        self.logs = root / "commands"
        self.logs.mkdir(parents=True, exist_ok=False)
        self.environment = clean_environment(identity, root)
        self.timeout_seconds = timeout_seconds
        self.sequence = 0

    def run(self, name: str, arguments: list[str], *, parse_json: bool = True) -> Any:
        self.sequence += 1
        stem = f"{self.sequence:02d}-{name}"
        command = [str(self.binary), *arguments]
        started = time.monotonic()
        try:
            completed = subprocess.run(
                command,
                cwd=self.root,
                env=self.environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=self.timeout_seconds,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout or b""
            stderr = error.stderr or b""
            (self.logs / f"{stem}.stdout").write_bytes(stdout)
            (self.logs / f"{stem}.stderr").write_bytes(stderr)
            package.write_json(
                self.logs / f"{stem}.command.json",
                {
                    "arguments": arguments,
                    "elapsed_milliseconds": round((time.monotonic() - started) * 1_000),
                    "exit_code": None,
                    "timed_out": True,
                },
            )
            raise CommandFailure(f"packaged command timed out: {name}") from error
        elapsed = round((time.monotonic() - started) * 1_000)
        (self.logs / f"{stem}.stdout").write_bytes(completed.stdout)
        (self.logs / f"{stem}.stderr").write_bytes(completed.stderr)
        package.write_json(
            self.logs / f"{stem}.command.json",
            {
                "arguments": arguments,
                "elapsed_milliseconds": elapsed,
                "exit_code": completed.returncode,
                "timed_out": False,
                "stdout_sha256": package.sha256(self.logs / f"{stem}.stdout"),
                "stderr_sha256": package.sha256(self.logs / f"{stem}.stderr"),
            },
        )
        value = None
        if parse_json:
            try:
                value = json.loads(completed.stdout)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise CommandFailure(f"packaged command returned invalid JSON: {name}") from error
        if completed.returncode != 0:
            detail = diagnostic_detail(value) if isinstance(value, dict) else ""
            raise CommandFailure(
                f"packaged command failed ({completed.returncode}): {name}{detail}"
            )
        if completed.stderr:
            raise CommandFailure(f"successful packaged command wrote stderr: {name}")
        return value if parse_json else completed.stdout

    def json(
        self,
        name: str,
        arguments: list[str],
        *,
        config: Path | None = None,
        project: Path | None = None,
    ) -> dict[str, Any]:
        command = ["--format", "json"]
        if config is not None:
            command.extend(["--config", str(config)])
        if project is not None:
            command.extend(["--project", str(project)])
        command.extend(arguments)
        value = self.run(name, command)
        if not isinstance(value, dict) or value.get("ok") is not True:
            raise CommandFailure(f"packaged command did not return a success envelope: {name}")
        return value


def diagnostic_detail(value: dict[str, Any]) -> str:
    diagnostics = value.get("diagnostics")
    if not isinstance(diagnostics, list):
        return ""
    details = [
        f"{item.get('code')}: {item.get('message')}"
        for item in diagnostics
        if isinstance(item, dict)
    ]
    return f" ({'; '.join(details)})" if details else ""


def configure_runtime(recorder: CommandRecorder, config: Path) -> None:
    recorder.json("config-init", ["config", "init"], config=config)
    for key, value in (
        ("resources.memory_reserve_mib", "0"),
        ("resources.memory_pool_mib", "4096"),
        ("resources.memory_reserve_mib", "512"),
        ("resources.cpu_slots", "4"),
        ("daemon.idle_shutdown_seconds", "2"),
    ):
        recorder.json(
            f"config-set-{key.replace('.', '-')}",
            ["config", "set", key, value],
            config=config,
        )
    recorder.json("config-validate", ["config", "validate"], config=config)
    recorder.json("doctor-deep", ["doctor", "--deep"], config=config)


def catalog_path(config: Path) -> Path:
    return config.parent / "runtime" / "jobs.sqlite3"


def daemon_lease(config: Path) -> dict[str, Any] | None:
    path = catalog_path(config)
    if not path.is_file():
        return None
    with closing(sqlite3.connect(os.fspath(path))) as connection:
        connection.execute("PRAGMA query_only = ON")
        row = connection.execute(
            "SELECT daemon_instance_id, daemon_pid, daemon_start_token FROM daemon_lease "
            "WHERE singleton = 1"
        ).fetchone()
    if row is None:
        return None
    return {"daemon_instance_id": row[0], "daemon_pid": row[1], "daemon_start_token": row[2]}


def wait_for_daemon_lease(config: Path, timeout_seconds: float = 3.0) -> dict[str, Any] | None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        lease = daemon_lease(config)
        if lease is not None:
            return lease
        time.sleep(0.05)
    return None


def wait_for_idle(config: Path, timeout_seconds: float = 20.0) -> bool:
    path = catalog_path(config)
    if not path.is_file():
        return True
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        with closing(sqlite3.connect(os.fspath(path))) as connection:
            connection.execute("PRAGMA query_only = ON")
            daemon_count = connection.execute("SELECT COUNT(*) FROM daemon_lease").fetchone()[0]
            worker_count = connection.execute("SELECT COUNT(*) FROM worker_leases").fetchone()[0]
        if daemon_count == 0 and worker_count == 0:
            return True
        time.sleep(0.1)
    return False


def artifact_fingerprint(root: Path) -> dict[str, tuple[int, str]]:
    return {
        path.relative_to(root).as_posix(): (path.stat().st_size, package.sha256(path))
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def timestamp_ns(value: dict[str, Any]) -> int:
    return int(value["seconds_since_unix_epoch"]) * 1_000_000_000 + int(value["nanosecond"])


def expected_provenance_inputs(cell: Cell) -> set[str]:
    files = FAMILIES[cell.family].files
    if cell.family == "cfsr-pressure":
        files = files[:3] if cell.direction == "forward" else files[1:]
    return {name for name, _digest in files}


def expected_manifest_identity(project: Path, cell: Cell) -> dict[str, dict[str, str]]:
    lock_path = project / "locks" / f"{cell.family}-{cell.backend}.lock.json"
    lock = json.loads(lock_path.read_text(encoding="utf-8"))
    files = lock.get("files")
    profile = lock.get("profile")
    if not isinstance(files, list) or not files or not isinstance(profile, dict):
        raise ProductError("generated DatasetLock identity is incomplete")
    frozen = dict(FAMILIES[cell.family].files)
    content: dict[str, str] = {}
    locked_names: set[str] = set()
    for entry in files:
        if not isinstance(entry, dict):
            raise ProductError("generated DatasetLock file entry is invalid")
        name = entry.get("relative_path")
        digest = entry.get("sha256")
        if (
            not isinstance(name, str)
            or not isinstance(digest, str)
            or frozen.get(name) != digest
            or name in locked_names
        ):
            raise ProductError("generated DatasetLock content identity drifted")
        locked_names.add(name)
        content[f"{cell.family}:{name}"] = digest
    if not expected_provenance_inputs(cell) <= locked_names:
        raise ProductError("generated DatasetLock omits a required runtime source")
    profile_sha256 = profile.get("sha256")
    if not isinstance(profile_sha256, str) or not SHA256_RE.fullmatch(profile_sha256):
        raise ProductError("generated DatasetLock profile identity is invalid")
    return {
        "dataset_lock_sha256": {cell.family: package.sha256(lock_path)},
        "dataset_profile_sha256": {cell.family: profile_sha256},
        "dataset_content_sha256": content,
    }


def observed_provenance_inputs(bundle: Path, cell: Cell) -> set[str]:
    tokens = {
        name: name.encode("utf-8")
        for name, _digest in FAMILIES[cell.family].files
    }
    overlap = max(len(token) for token in tokens.values()) - 1
    found: set[str] = set()
    carry = b""
    with bundle.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            block = carry + chunk
            found.update(name for name, token in tokens.items() if token in block)
            carry = block[-overlap:]
    return found


def sqlite_audit(path: Path, manifest_counts: dict[str, Any]) -> tuple[bool, int | None]:
    if not path.is_file():
        return False, None
    # Windows may return an extended-length path (\\?\...) for a run artifact.
    # Pass it to SQLite as a native filesystem path: treating it as a file URI
    # makes the question mark look like an invalid URI authority.
    with closing(sqlite3.connect(os.fspath(path))) as connection:
        connection.execute("PRAGMA query_only = ON")
        integrity = connection.execute("PRAGMA integrity_check").fetchone()
        if integrity is None or str(integrity[0]).lower() != "ok":
            return False, None
        for table, expected in manifest_counts.items():
            actual = connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
            if actual != expected:
                return False, None
        particle_id = connection.execute("SELECT MIN(particle_id) FROM particle").fetchone()[0]
        for table in ("particle", "particle_state", "particle_mass", "termination"):
            real_columns = [
                row[1]
                for row in connection.execute(f'PRAGMA table_info("{table}")')
                if str(row[2]).upper() == "REAL"
            ]
            for column in real_columns:
                nonfinite = connection.execute(
                    f'SELECT COUNT(*) FROM "{table}" WHERE "{column}" IS NOT NULL '
                    f'AND ("{column}" != "{column}" OR abs("{column}") > 1.7976931348623157e308)'
                ).fetchone()[0]
                if nonfinite != 0:
                    return False, None
    return particle_id is not None, int(particle_id) if particle_id is not None else None


def quality_is_valid(quality: Any, particles: Any) -> bool:
    if not isinstance(quality, dict) or set(quality) != {"wind", "pressure", "temperature"}:
        return False
    if not isinstance(particles, dict):
        return False
    state_count = particles.get("state_count")
    normal_terminations = particles.get("normal_termination_count")
    if (
        type(state_count) is not int
        or state_count <= 0
        or type(normal_terminations) is not int
        or normal_terminations < 0
    ):
        return False

    missing_counts = []
    for field in ("wind", "pressure", "temperature"):
        rows = quality[field]
        if not isinstance(rows, list) or not rows:
            return False
        seen = set()
        total = 0
        missing = 0
        for entry in rows:
            if not isinstance(entry, dict):
                return False
            count = entry.get("count")
            validity = entry.get("validity")
            provenance_quality = entry.get("quality")
            if type(count) is not int or count <= 0:
                return False
            if validity not in {"ok", "missing"}:
                return False
            if provenance_quality not in {"source", "derived"}:
                return False
            bucket = (validity, provenance_quality)
            if bucket in seen:
                return False
            seen.add(bucket)
            total += count
            if validity == "missing":
                missing += count
        if total != state_count or missing > normal_terminations:
            return False
        missing_counts.append(missing)
    return len(set(missing_counts)) == 1


def mass_ledger_is_valid(manifest: dict[str, Any], inspection: dict[str, Any]) -> bool:
    records = manifest.get("mass_ledger")
    if not isinstance(records, list):
        return False
    for record in records:
        imbalance = record.get("imbalance_kg")
        tolerance = record.get("tolerance_kg")
        if not isinstance(imbalance, (int, float)) or not isinstance(tolerance, (int, float)):
            return False
        if not math.isfinite(float(imbalance)) or not math.isfinite(float(tolerance)):
            return False
        if abs(float(imbalance)) > float(tolerance):
            return False
    fraction = inspection.get("mass_ledger", {}).get("maximum_tolerance_fraction")
    return isinstance(fraction, (int, float)) and math.isfinite(float(fraction)) and fraction <= 1.0


def verify_run(
    recorder: CommandRecorder,
    config: Path,
    project: Path,
    cell: Cell,
    final_envelope: dict[str, Any],
    checks: dict[str, bool],
) -> tuple[dict[str, Any], Path]:
    final = final_envelope.get("data")
    if not isinstance(final, dict):
        raise ProductError("terminal job envelope has no data object")
    checks["job_complete"] = final.get("state") == "complete" and final_envelope.get("run_success") is True
    series = final.get("job_series_id")
    run_id = final.get("run_id")
    attempt = final.get("attempt")
    output_text = final.get("output_directory")
    if not isinstance(series, str) or not isinstance(run_id, str) or not isinstance(output_text, str):
        raise ProductError("terminal job envelope is missing stable identity")
    output = Path(output_text)
    manifest_path = output / "run-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    checks["manifest_complete"] = manifest.get("status") == "complete"
    checks["job_identity"] = (
        manifest.get("job_series_id") == series
        and manifest.get("run_id") == run_id
        and manifest.get("attempt") == attempt == 1
    )
    checks["abnormal_zero"] = manifest.get("terminations", {}).get("abnormal_count") == 0
    checks["population_model"] = (
        manifest.get("numerical", {}).get("population") == EXPECTED_POPULATION_MODEL[cell.population]
    )
    checks["reader_backend"] = manifest.get("execution", {}).get("reader_backends") == {
        cell.family: cell.backend
    }
    checks["worker_threads"] = manifest.get("execution", {}).get("worker_threads") == cell.worker_threads
    manifest_inputs = manifest.get("inputs", {})
    expected_inputs = expected_manifest_identity(project, cell)
    checks["input_identity"] = all(
        manifest_inputs.get(name) == value for name, value in expected_inputs.items()
    )

    status = recorder.json("job-status", ["job", "status", series], config=config)
    events = recorder.json("job-events", ["job", "events", series], config=config)
    status_data = status.get("data", {})
    checks["catalog_identity"] = (
        status_data.get("job_series_id") == series
        and status_data.get("run_id") == run_id
        and status_data.get("attempt") == 1
        and status_data.get("state") == "complete"
    )
    event_rows = events.get("data")
    checks["terminal_event"] = isinstance(event_rows, list) and any(
        isinstance(event, dict)
        and event.get("run_id") == run_id
        and event.get("kind") == "state_transition"
        and event.get("state") == "complete"
        for event in event_rows
    )

    inspection_envelope = recorder.json(
        "result-inspect-path", ["result", "inspect", str(output)], config=config
    )
    inspection = inspection_envelope.get("data", {})
    checks["lifecycle"] = (
        inspection_envelope.get("run_success") is True
        and inspection.get("lifecycle", {}).get("status") == "complete"
        and inspection.get("identity", {}).get("run_id") == run_id
    )
    checks["quality"] = quality_is_valid(
        inspection.get("quality"), inspection.get("particles")
    )
    checks["mass_ledger"] = mass_ledger_is_valid(manifest, inspection)
    checks["particle_count"] = inspection.get("particles", {}).get("particle_count") == cell.particles

    verification = recorder.json(
        "result-verify-full", ["result", "verify", series, "--full"], config=config
    )
    verified = verification.get("data", {})
    provenance = manifest.get("provenance", {})
    checks["full_verify"] = (
        verification.get("run_success") is True
        and verified.get("run_success") is True
        and verified.get("mode") == "full"
        and verified.get("status") == "complete"
        and verified.get("run_id") == run_id
        and verified.get("canonical_output_sha256") == provenance.get("canonical_output_sha256")
        and verified.get("sqlite_sql_sha256") == provenance.get("sqlite_sql_sha256")
        and verified.get("provenance_content_sha256") == provenance.get("content_sha256")
    )

    catalog_inspection = recorder.json(
        "result-inspect-catalog", ["result", "inspect", series], config=config
    )
    catalog_data = catalog_inspection.get("data", {})
    full_verification = catalog_data.get("catalog", {}).get("full_verification")
    checks["catalog_full_verification"] = (
        catalog_data.get("identity", {}).get("run_id") == run_id
        and catalog_data.get("catalog", {}).get("visible_in_routine_list") is True
        and isinstance(full_verification, dict)
        and full_verification.get("canonical_output_sha256") == provenance.get("canonical_output_sha256")
    )

    sqlite_path = output / manifest.get("sqlite", {}).get("relative_path", "particles.sqlite")
    sqlite_ok, particle_id = sqlite_audit(sqlite_path, manifest.get("sqlite", {}).get("row_counts", {}))
    checks["sqlite_integrity_and_counts"] = sqlite_ok
    wal = sqlite_path.with_name(sqlite_path.name + "-wal")
    checks["terminal_wal"] = not wal.exists() or wal.stat().st_size == 0
    bundle = output / provenance.get("relative_path", "provenance-bundle.json")
    checks["provenance_identity"] = (
        bundle.is_file()
        and package.sha256(bundle) == provenance.get("sha256")
        and package.sha256(sqlite_path) == provenance.get("sqlite_sha256")
    )
    checks["provenance_input_usage"] = (
        bundle.is_file()
        and observed_provenance_inputs(bundle, cell) == expected_provenance_inputs(cell)
    )

    if particle_id is None:
        raise ProductError("completed run has no particle identity for trajectory inspection")
    trajectory = recorder.json(
        "result-trajectory",
        ["result", "trajectory", str(output), "--particle-id", str(particle_id)],
        config=config,
    )
    trajectory_data = trajectory.get("data", {})
    checks["trajectory"] = (
        trajectory_data.get("run_id") == run_id
        and isinstance(trajectory_data.get("records"), list)
        and bool(trajectory_data.get("records"))
    )

    report_path = output / "run-report.md"
    checks["automatic_report"] = report_path.is_file()
    manifest_sha_before = package.sha256(manifest_path)
    recorder.json("run-report-first", ["run", "report", "--result", run_id], config=config)
    report_sha = package.sha256(report_path)
    recorder.json("run-report-second", ["run", "report", "--result", run_id], config=config)
    checks["report_idempotent"] = report_sha == package.sha256(report_path)
    verify_after = recorder.json(
        "result-verify-after-report", ["result", "verify", series, "--full"], config=config
    ).get("data", {})
    checks["report_digest_excluded"] = (
        manifest_sha_before == package.sha256(manifest_path)
        and verify_after.get("provenance_content_sha256") == provenance.get("content_sha256")
        and verify_after.get("sqlite_sql_sha256") == provenance.get("sqlite_sql_sha256")
        and verify_after.get("canonical_output_sha256") == provenance.get("canonical_output_sha256")
    )

    resolved_profile = json.loads((output / "resolved-run-profile.json").read_text(encoding="utf-8"))
    checks["resolved_profile_backend"] = (
        resolved_profile.get("execution", {}).get("meteorology_reader") == cell.backend
        and resolved_profile.get("datasets", [{}])[0].get("reader_backend") == cell.backend
    )
    elapsed = max(0, timestamp_ns(manifest["finished_at"]) - timestamp_ns(manifest["started_at"]))
    run = {
        "job_series_id": series,
        "run_id": run_id,
        "attempt": 1,
        "run_directory": str(output),
        "runner_milliseconds": elapsed // 1_000_000,
        "digests": {
            "content_sha256": provenance["content_sha256"],
            "sqlite_sql_sha256": provenance["sqlite_sql_sha256"],
            "canonical_output_sha256": provenance["canonical_output_sha256"],
        },
    }
    return run, output


REQUIRED_CHECKS = {
    "package_identity",
    "fixture_identity",
    "project_finalized",
    "job_complete",
    "job_identity",
    "manifest_complete",
    "abnormal_zero",
    "population_model",
    "reader_backend",
    "worker_threads",
    "input_identity",
    "catalog_identity",
    "terminal_event",
    "lifecycle",
    "quality",
    "mass_ledger",
    "particle_count",
    "full_verify",
    "catalog_full_verification",
    "sqlite_integrity_and_counts",
    "terminal_wal",
    "provenance_identity",
    "provenance_input_usage",
    "trajectory",
    "automatic_report",
    "report_idempotent",
    "report_digest_excluded",
    "resolved_profile_backend",
    "daemon_idle",
}


def validate_product_cell(value: dict[str, Any]) -> None:
    required = {
        "schema_version",
        "cell",
        "source_tree_sha256",
        "build_manifest_sha256",
        "binary_sha256",
        "result",
        "checks",
        "failures",
    }
    if not required <= set(value) or set(value) - (required | {"run"}):
        raise ProductError("product cell top-level shape drifted")
    if value["schema_version"] != "trajecta.m5-a4-product-cell/v1":
        raise ProductError("product cell schema identity drifted")
    cell = value["cell"]
    if set(cell) != {
        "id",
        "platform",
        "phase",
        "family",
        "population",
        "direction",
        "backend",
        "particles",
        "worker_threads",
    }:
        raise ProductError("product cell identity shape drifted")
    if cell["platform"] not in {"windows-x86_64", "linux-x86_64"}:
        raise ProductError("product cell platform drifted")
    if cell["phase"] not in {"rust-1k", "rust-10k", "native-1k"}:
        raise ProductError("product cell phase drifted")
    if cell["family"] not in FAMILIES or cell["population"] not in POPULATIONS:
        raise ProductError("product cell scientific selection drifted")
    if cell["direction"] not in DIRECTIONS or cell["backend"] not in {"rust", "native"}:
        raise ProductError("product cell direction/backend drifted")
    if cell["particles"] not in {1_000, 10_000} or not isinstance(cell["worker_threads"], int):
        raise ProductError("product cell scale drifted")
    for key in ("source_tree_sha256", "build_manifest_sha256", "binary_sha256"):
        if not isinstance(value[key], str) or not SHA256_RE.fullmatch(value[key]):
            raise ProductError(f"product cell {key} is invalid")
    if value["result"] not in {"passed", "failed"}:
        raise ProductError("product cell result drifted")
    if not isinstance(value["checks"], dict) or not all(
        isinstance(item, bool) for item in value["checks"].values()
    ):
        raise ProductError("product cell checks must be booleans")
    if not isinstance(value["failures"], list) or not all(
        isinstance(item, str) and item for item in value["failures"]
    ):
        raise ProductError("product cell failures are invalid")
    if value["result"] == "passed":
        if value["failures"] or not value["checks"] or not all(value["checks"].values()):
            raise ProductError("passed product cell contains a failed check")
        run = value.get("run")
        if not isinstance(run, dict):
            raise ProductError("passed product cell is missing run identity")
        if not UUID7_RE.fullmatch(run.get("job_series_id", "")) or not UUID7_RE.fullmatch(
            run.get("run_id", "")
        ):
            raise ProductError("product cell run UUID identity is invalid")
        if run.get("attempt") != 1 or not isinstance(run.get("runner_milliseconds"), int):
            raise ProductError("product cell run attempt/timing drifted")
        digests = run.get("digests")
        if not isinstance(digests, dict) or set(digests) != {
            "content_sha256",
            "sqlite_sql_sha256",
            "canonical_output_sha256",
        }:
            raise ProductError("product cell digest shape drifted")
        if not all(isinstance(item, str) and SHA256_RE.fullmatch(item) for item in digests.values()):
            raise ProductError("product cell digest identity is invalid")
    elif not value["failures"]:
        raise ProductError("failed product cell must explain its failure")


def run_cell(
    identity: ProductIdentity,
    artifact_root: Path,
    cell: Cell,
    data_root: Path,
    fixture_identity: dict[str, str],
    timeout_seconds: int,
    *,
    detach: bool = False,
    restart_after_idle: bool = False,
    duration_seconds: int = 600,
) -> tuple[dict[str, Any], Path]:
    attempt = artifact_root / "cells" / cell.id / "attempt-1"
    if attempt.exists():
        raise ProductError(f"refusing to overwrite product cell attempt: {attempt}")
    attempt.mkdir(parents=True)
    checks = {name: False for name in sorted(REQUIRED_CHECKS)}
    checks["package_identity"] = True
    checks["fixture_identity"] = fixture_identity == dict(FAMILIES[cell.family].files)
    checks["detached_submission" if detach else "foreground_default"] = False
    if restart_after_idle:
        checks["daemon_restart"] = False
        checks["completed_not_rerun"] = False
    failures: list[str] = []
    run: dict[str, Any] | None = None
    config = attempt / "config.toml"
    recorder = CommandRecorder(identity, attempt, timeout_seconds)
    package.write_json(
        attempt / "CELL_INPUT.json",
        {
            "cell": cell.as_json(),
            "package_root": str(identity.package_root),
            "data_root": str(data_root.resolve()),
            "fixture_identity": fixture_identity,
            "duration_seconds": duration_seconds,
        },
    )
    try:
        configure_runtime(recorder, config)
        project = prepare_project(attempt, cell, data_root, duration_seconds)
        finalized = recorder.json(
            "project-finalize", ["project", "finalize"], config=config, project=project
        )
        checks["project_finalized"] = finalized.get("data", {}).get("state") == "finalized"
        arguments = ["run", "--profile", "product"]
        if detach:
            arguments.append("--detach")
        submitted = recorder.json("run-submit", arguments, config=config, project=project)
        if detach:
            checks["detached_submission"] = submitted.get("data", {}).get("state") == "queued"
            series = submitted.get("data", {}).get("job_series_id")
            if not isinstance(series, str):
                raise ProductError("detached run did not return a job series identity")
            final = recorder.json("job-wait", ["job", "wait", series], config=config)
        else:
            checks["foreground_default"] = submitted.get("data", {}).get("state") == "complete"
            final = submitted
        run, output = verify_run(recorder, config, project, cell, final, checks)
        if restart_after_idle:
            old_lease = wait_for_daemon_lease(config)
            before = artifact_fingerprint(output)
            if old_lease is None or not wait_for_idle(config):
                raise ProductError("daemon did not expose and release its first idle lease")
            status = recorder.json(
                "job-status-after-idle", ["job", "status", run["job_series_id"]], config=config
            )
            new_lease = wait_for_daemon_lease(config)
            checks["daemon_restart"] = (
                new_lease is not None
                and old_lease["daemon_instance_id"] != new_lease["daemon_instance_id"]
            )
            checks["completed_not_rerun"] = (
                status.get("data", {}).get("run_id") == run["run_id"]
                and status.get("data", {}).get("attempt") == 1
                and artifact_fingerprint(output) == before
            )
    except (
        CommandFailure,
        ProductError,
        OSError,
        sqlite3.Error,
        json.JSONDecodeError,
        KeyError,
        TypeError,
        ValueError,
    ) as error:
        failures.append(str(error))
    checks["daemon_idle"] = wait_for_idle(config) if config.exists() else True
    failed_checks = [name for name, passed in checks.items() if not passed]
    failures.extend(f"check failed: {name}" for name in failed_checks if f"check failed: {name}" not in failures)
    result = "passed" if not failures else "failed"
    summary: dict[str, Any] = {
        "schema_version": "trajecta.m5-a4-product-cell/v1",
        "cell": cell.as_json(),
        "source_tree_sha256": identity.source_tree_sha256,
        "build_manifest_sha256": identity.build_manifest_sha256,
        "binary_sha256": identity.binary_sha256,
        "result": result,
        "checks": checks,
        "failures": failures,
    }
    if run is not None:
        summary["run"] = run
    validate_product_cell(summary)
    path = attempt / CELL_RESULT_NAME
    package.write_json(path, summary)
    return summary, path


def run_clean_preflight(
    identity: ProductIdentity, artifact_root: Path, timeout_seconds: int
) -> tuple[bool, Path]:
    root = artifact_root / "clean-preflight" / "attempt-1"
    if root.exists():
        raise ProductError(f"refusing to overwrite clean preflight: {root}")
    root.mkdir(parents=True)
    recorder = CommandRecorder(identity, root, timeout_seconds)
    checks = {
        "human_help": False,
        "json_help": False,
        "config": False,
        "doctor": False,
        "minimal_configured": False,
        "minimal_pending": False,
    }
    failures: list[str] = []
    config = root / "config.toml"
    try:
        help_bytes = recorder.run("human-help", ["--help"], parse_json=False)
        checks["human_help"] = b"Usage:" in help_bytes
        json_help = recorder.run("json-help", ["--format", "json", "--help"])
        checks["json_help"] = json_help.get("ok") is True and "help" in json_help.get("data", {})
        configure_runtime(recorder, config)
        checks["config"] = True
        doctor = recorder.json("doctor-repeat", ["doctor", "--deep"], config=config)
        checks["doctor"] = doctor.get("ok") is True
        minimal = root / "minimal-project"
        shutil.copytree(identity.package_root / "examples" / "minimal", minimal)
        status = recorder.json(
            "minimal-status", ["project", "status"], config=config, project=minimal
        )
        validated = recorder.json(
            "minimal-validate", ["project", "validate"], config=config, project=minimal
        )
        diagnostics = [
            item.get("code")
            for item in status.get("diagnostics", [])
            if isinstance(item, dict)
        ]
        checks["minimal_configured"] = (
            status.get("data", {}).get("state") == "configured"
            and validated.get("data", {}).get("state") == "configured"
            and validated.get("data", {}).get("valid") is True
        )
        checks["minimal_pending"] = {
            "project.lock_missing",
            "project.finalize_pending",
        } <= set(diagnostics)
    except (CommandFailure, ProductError, OSError, KeyError, TypeError, ValueError) as error:
        failures.append(str(error))
    failures.extend(f"check failed: {name}" for name, passed in checks.items() if not passed)
    value = {
        "schema_version": "trajecta.m5-a4-clean-extraction-preflight/v1",
        "package_root": str(identity.package_root),
        "source_tree_sha256": identity.source_tree_sha256,
        "build_manifest_sha256": identity.build_manifest_sha256,
        "binary_sha256": identity.binary_sha256,
        "result": "passed" if not failures else "failed",
        "checks": checks,
        "failures": failures,
    }
    path = root / PREFLIGHT_NAME
    package.write_json(path, value)
    return not failures, path


def aggregate_value(
    mode: str,
    identity: ProductIdentity,
    selected: list[Cell],
    completed: list[tuple[dict[str, Any], Path]],
    started_at: str,
    preflight: Path | None,
    result: str,
) -> dict[str, Any]:
    completed_ids = {summary["cell"]["id"] for summary, _ in completed}
    return {
        "schema_version": "trajecta.m5-a4-product-matrix/v1",
        "mode": mode,
        "platform": identity.platform,
        "started_at_utc": started_at,
        "finished_at_utc": utc_now(),
        "package": {
            "root": str(identity.package_root),
            "source_tree_sha256": identity.source_tree_sha256,
            "build_manifest_sha256": identity.build_manifest_sha256,
            "binary_sha256": identity.binary_sha256,
        },
        "stop_on_first_failure": True,
        "selected_cell_ids": [cell.id for cell in selected],
        "completed": [
            {
                "id": summary["cell"]["id"],
                "result": summary["result"],
                "summary": str(path),
                "summary_sha256": package.sha256(path),
            }
            for summary, path in completed
        ],
        "not_run_cell_ids": [cell.id for cell in selected if cell.id not in completed_ids],
        "passed_cell_count": sum(summary["result"] == "passed" for summary, _ in completed),
        "failed_cell_count": sum(summary["result"] == "failed" for summary, _ in completed),
        "clean_preflight": str(preflight) if preflight is not None else None,
        "result": result,
    }


def required_data_roots(arguments: argparse.Namespace) -> dict[str, Path | None]:
    return {
        "era5-pressure": arguments.era5_pressure_dir,
        "era5-hybrid": arguments.era5_hybrid_dir,
        "cfsr-pressure": arguments.cfsr_dir,
    }


def execute(arguments: argparse.Namespace) -> int:
    identity = validate_product_root(arguments.package_root)
    artifact_root = arguments.artifact_root.resolve()
    if artifact_root.exists():
        raise ProductError(f"refusing to overwrite M5-A4 artifact root: {artifact_root}")
    artifact_root.mkdir(parents=True)
    started = utc_now()
    cells = smoke_cells(identity.platform) if arguments.mode == "smoke" else formal_cells(identity.platform)
    if arguments.cell is not None:
        matches = [cell for cell in cells if cell.id == arguments.cell]
        if not matches:
            raise ProductError(f"unknown selected product cell: {arguments.cell}")
        cells = matches
    roots = required_data_roots(arguments)
    fixture_identities: dict[str, dict[str, str]] = {}
    try:
        for family in sorted({cell.family for cell in cells}):
            root = roots[family]
            if root is None:
                flags = {
                    "era5-pressure": "--era5-pressure-dir",
                    "era5-hybrid": "--era5-hybrid-dir",
                    "cfsr-pressure": "--cfsr-dir",
                }
                raise FixtureUnavailable(f"missing {flags[family]}")
            fixture_identities[family] = validate_fixture(FAMILIES[family], root)
    except FixtureUnavailable as error:
        summary = aggregate_value(
            arguments.mode, identity, cells, [], started, None, "external_blocked"
        )
        summary["external_blocker"] = str(error)
        package.write_json(artifact_root / MATRIX_SUMMARY_NAME, summary)
        return 2
    except ProductError as error:
        summary = aggregate_value(arguments.mode, identity, cells, [], started, None, "failed")
        summary["failure"] = str(error)
        package.write_json(artifact_root / MATRIX_SUMMARY_NAME, summary)
        return 1

    preflight_path: Path | None = None
    if arguments.mode == "smoke":
        passed, preflight_path = run_clean_preflight(
            identity, artifact_root, arguments.timeout_seconds
        )
        if not passed:
            package.write_json(
                artifact_root / MATRIX_SUMMARY_NAME,
                aggregate_value(
                    arguments.mode,
                    identity,
                    cells,
                    [],
                    started,
                    preflight_path,
                    "failed",
                ),
            )
            return 1

    completed: list[tuple[dict[str, Any], Path]] = []
    for index, cell in enumerate(cells):
        summary, path = run_cell(
            identity,
            artifact_root,
            cell,
            roots[cell.family],
            fixture_identities[cell.family],
            arguments.timeout_seconds,
            detach=arguments.mode == "smoke" and index == 1,
            restart_after_idle=arguments.mode == "smoke" and index == 0,
            duration_seconds=0 if arguments.mode == "smoke" else 600,
        )
        completed.append((summary, path))
        result = "failed" if summary["result"] == "failed" else "running"
        package.write_json(
            artifact_root / MATRIX_SUMMARY_NAME,
            aggregate_value(
                arguments.mode,
                identity,
                cells,
                completed,
                started,
                preflight_path,
                result,
            ),
        )
        if summary["result"] == "failed":
            return 1
    package.write_json(
        artifact_root / MATRIX_SUMMARY_NAME,
        aggregate_value(
            arguments.mode,
            identity,
            cells,
            completed,
            started,
            preflight_path,
            "passed",
        ),
    )
    return 0


def parser() -> argparse.ArgumentParser:
    product = argparse.ArgumentParser(description=__doc__)
    product.add_argument("mode", choices=["smoke", "matrix"])
    product.add_argument("--package-root", type=Path, required=True)
    product.add_argument("--artifact-root", type=Path, required=True)
    product.add_argument("--era5-pressure-dir", type=Path)
    product.add_argument("--era5-hybrid-dir", type=Path)
    product.add_argument("--cfsr-dir", type=Path)
    product.add_argument("--cell")
    product.add_argument("--timeout-seconds", type=int, default=14_400)
    return product


def main() -> int:
    arguments = parser().parse_args()
    if arguments.timeout_seconds <= 0:
        print("M5-A4 timeout must be positive", file=sys.stderr)
        return 2
    try:
        return execute(arguments)
    except (ProductError, OSError, sqlite3.Error) as error:
        print(f"M5-A4 product execution failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
