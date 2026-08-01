#!/usr/bin/env python3
"""Run the documented domain-fill quickstart against a source tree or package."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path
from typing import Any


FILES = {
    "pgbl00.gdas.2009010100.grb2": "fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c",
    "pgbl00.gdas.2009010106.grb2": "00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b",
    "pgbl00.gdas.2009010112.grb2": "f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5",
    "pgbl00.gdas.2009010118.grb2": "a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade",
}


class QuickstartError(RuntimeError):
    """The executable quickstart contract failed."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )


class Runner:
    def __init__(self, binary: Path, package_root: Path, artifact_root: Path) -> None:
        self.binary = binary
        self.package_root = package_root
        self.artifact_root = artifact_root
        self.transcripts = artifact_root / "transcripts"
        self.transcripts.mkdir()

    def command(
        self,
        name: str,
        arguments: list[str],
        *,
        timeout: int = 900,
    ) -> dict[str, Any]:
        command = [str(self.binary), "--format", "json", *arguments]
        started = time.monotonic()
        completed = subprocess.run(
            command,
            cwd=self.package_root,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
        prefix = self.transcripts / name
        prefix.with_suffix(".stdout").write_bytes(completed.stdout)
        prefix.with_suffix(".stderr").write_bytes(completed.stderr)
        write_json(
            prefix.with_suffix(".command.json"),
            {
                "arguments": arguments,
                "elapsed_seconds": time.monotonic() - started,
                "return_code": completed.returncode,
            },
        )
        if completed.returncode != 0 or completed.stderr:
            raise QuickstartError(f"{name} failed; inspect {prefix}")
        text = completed.stdout.decode("utf-8")
        try:
            envelope = json.loads(text)
        except json.JSONDecodeError as error:
            raise QuickstartError(f"{name} returned invalid JSON") from error
        if not isinstance(envelope, dict) or envelope.get("ok") is not True:
            raise QuickstartError(f"{name} returned an unsuccessful envelope")
        return envelope

    def helper(self, project: Path, plan: Path, execute: bool) -> dict[str, Any]:
        script = self.package_root / "tools" / "fetch_trajecta_data.py"
        environment = os.environ.copy()
        environment["TRAJECTA_BIN"] = str(self.binary)
        command = [
            sys.executable,
            str(script),
            "--project",
            str(project),
            "--plan",
            str(plan),
        ]
        if execute:
            command.append("--execute")
        completed = subprocess.run(
            command,
            cwd=self.package_root,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=120,
            check=False,
        )
        (self.transcripts / "data-helper.stdout").write_bytes(completed.stdout)
        (self.transcripts / "data-helper.stderr").write_bytes(completed.stderr)
        if completed.returncode != 0 or completed.stderr:
            raise QuickstartError("data helper dry-run failed")
        value = json.loads(completed.stdout)
        if not execute:
            states = [
                record["state"]
                for requirement in value["requirements"]
                for record in requirement["files"]
            ]
            if states != ["present"] * len(FILES):
                raise QuickstartError(f"data helper did not find all frozen files: {states}")
        return value


def locate_binary(package_root: Path, supplied: Path | None) -> Path:
    if supplied is not None:
        binary = supplied.resolve()
    else:
        name = "trajecta.exe" if os.name == "nt" else "trajecta"
        binary = package_root / name
    if not binary.is_file():
        raise QuickstartError(f"Trajecta binary is missing: {binary}")
    return binary


def copy_data(source: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    for name, expected in FILES.items():
        source_file = source / name
        if not source_file.is_file() or sha256(source_file) != expected:
            raise QuickstartError(f"frozen CFSR identity mismatch: {source_file}")
        shutil.copy2(source_file, destination / name)


def run(
    package_root: Path,
    binary: Path,
    example: str,
    data: Path | None,
    fetch: bool,
    artifact: Path,
) -> dict[str, Any]:
    if artifact.exists():
        raise QuickstartError(f"refusing to overwrite artifact root: {artifact}")
    artifact.mkdir(parents=True)
    project = artifact / "project"
    shutil.copytree(package_root / "examples" / example, project)
    for name in ("locks", "runs"):
        (project / name).mkdir(exist_ok=True)
    if data is not None:
        copy_data(data, project / "data")

    runner = Runner(binary, package_root, artifact)
    config = artifact / "quickstart.toml"
    plan = artifact / "data-plan.json"
    common = ["--config", str(config)]
    selected = [*common, "--project", str(project)]
    started = time.monotonic()

    runner.command("01-config-init", [*common, "config", "init"])
    for index, (selector, value) in enumerate(
        (
            ("resources.cpu_slots", "1"),
            ("resources.memory_reserve_mib", "256"),
            ("resources.memory_pool_mib", "1536"),
        ),
        start=2,
    ):
        runner.command(
            f"{index:02d}-config-set",
            [*common, "config", "set", selector, value],
        )
    runner.command("05-config-validate", [*common, "config", "validate"])
    runner.command("06-project-validate", [*selected, "project", "validate"])
    runner.command(
        "07-data-plan",
        [*selected, "project", "data-plan", "--output", str(plan)],
    )
    runner.helper(project, plan, fetch)
    runner.command("09-project-finalize", [*selected, "project", "finalize"])
    runner.command("10-doctor", [*selected, "doctor", "--deep"])
    run_envelope = runner.command(
        "11-run",
        [*selected, "run", "--profile", "product"],
    )
    run_data = run_envelope.get("data", {})
    if not isinstance(run_data, dict) or run_data.get("state") != "complete":
        raise QuickstartError("foreground run did not reach complete")
    result = Path(str(run_data.get("output_directory", ""))).resolve()
    if not result.is_dir():
        raise QuickstartError("foreground run returned no result directory")

    runner.command("12-verify", [*common, "result", "verify", str(result), "--full"])
    inspect = runner.command("13-inspect", [*common, "result", "inspect", str(result)])
    with sqlite3.connect(result / "particles.sqlite") as connection:
        particle = connection.execute(
            "SELECT particle_id FROM particle ORDER BY particle_id LIMIT 1"
        ).fetchone()
    if particle is None:
        raise QuickstartError("result contains no particle")
    runner.command(
        "14-trajectory",
        [*common, "result", "trajectory", str(result), "--particle-id", str(particle[0])],
    )
    runner.command("15-report", [*common, "run", "report", "--result", str(result)])

    manifest = json.loads((result / "run-manifest.json").read_text(encoding="utf-8"))
    if manifest.get("status") != "complete":
        raise QuickstartError("run manifest is not complete")
    if manifest.get("terminations", {}).get("abnormal_count") != 0:
        raise QuickstartError("quickstart has abnormal particle terminations")
    elapsed = time.monotonic() - started
    if example == "domain-fill-cfsr" and elapsed > 900:
        raise QuickstartError(f"quickstart exceeded 15 minutes: {elapsed:.3f} seconds")
    identity = inspect.get("data", {}).get("identity", {})
    return {
        "schema_version": "trajecta.m5-1-example-run/v1",
        "status": "passed",
        "example": example,
        "elapsed_seconds": elapsed,
        "job_series_id": identity.get("job_series_id"),
        "run_id": identity.get("run_id"),
        "result": str(result),
        "manifest_sha256": sha256(result / "run-manifest.json"),
        "sqlite_sha256": sha256(result / "particles.sqlite"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-root", type=Path, required=True)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--example", default="domain-fill-cfsr")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--data", type=Path)
    source.add_argument("--fetch", action="store_true")
    parser.add_argument("--artifact-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        package_root = args.package_root.resolve()
        summary = run(
            package_root,
            locate_binary(package_root, args.binary),
            args.example,
            args.data.resolve() if args.data is not None else None,
            args.fetch,
            args.artifact_root.resolve(),
        )
        write_json(args.artifact_root.resolve() / "M5_1_QUICKSTART_SUMMARY.json", summary)
        print(json.dumps(summary, sort_keys=True))
        return 0
    except (
        OSError,
        json.JSONDecodeError,
        QuickstartError,
        subprocess.TimeoutExpired,
        sqlite3.Error,
    ) as error:
        print(f"quickstart error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
