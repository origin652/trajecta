#!/usr/bin/env python3
"""Plan or fetch meteorology declared by a Trajecta project data plan."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import shutil
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
TOOLS = Path(__file__).resolve().parent
TUTORIAL_AREA = [53.0, 0.0, 45.0, 10.0]
WORK_OWNER = "trajecta fetch helper 0.1.0-alpha.1\n"
DEMO_CFSR = {
    "2009010100": (5465079, "fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c"),
    "2009010106": (5436947, "00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b"),
    "2009010112": (5452202, "f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5"),
    "2009010118": (5494468, "a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade"),
}
GMTED2010_PROFILE = "gmted2010_30arcsec_mean_std"
GMTED2010_RELEASE_BASE = (
    "https://github.com/origin652/trajecta/releases/download/m6-gmted2010-v1"
)
GMTED2010_FILES = (
    (
        "gmted2010-30arcsec-mean.tgrid",
        274_915_129,
        "54070d724ab6aee76944b6fc10a029bd865efbf4c90747088c8f6d9404dabe3e",
    ),
    (
        "gmted2010-30arcsec-standard-deviation.tgrid",
        143_929_399,
        "f2359a29c5790fbd8bf0298470269453a45b329d1312cab88666c92355fadb97",
    ),
    (
        "gmted2010-source-manifest.json",
        8_792,
        "52e81a30ec3cdd44c7202325896bf40f8b6b4a9d0f5f8dd8fd29c4b0d2c97011",
    ),
)


class FetchError(RuntimeError):
    """A safe, user-actionable helper failure."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def locate_binary() -> Path:
    configured = os.environ.get("TRAJECTA_BIN")
    if configured:
        return Path(configured).expanduser().resolve()
    name = "trajecta-cli.exe" if os.name == "nt" else "trajecta-cli"
    source_binary = ROOT / "target" / "debug" / name
    if source_binary.is_file():
        return source_binary.resolve()
    package_name = "trajecta.exe" if os.name == "nt" else "trajecta"
    package_binary = ROOT / package_name
    if package_binary.is_file():
        return package_binary.resolve()
    discovered = shutil.which("trajecta")
    if discovered:
        return Path(discovered).resolve()
    raise FetchError("cannot find trajecta; set TRAJECTA_BIN to the product binary")


def cli_json(binary: Path, arguments: list[str]) -> dict[str, Any]:
    completed = subprocess.run(
        [str(binary), "--format", "json", *arguments],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.stderr:
        raise FetchError("trajecta wrote unexpected stderr while resolving the project")
    try:
        envelope = json.loads(completed.stdout)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FetchError("trajecta returned an invalid JSON envelope") from error
    if completed.returncode != 0 or not envelope.get("ok"):
        diagnostics = envelope.get("diagnostics", [])
        summary = "; ".join(
            f"{item.get('code', 'unknown')}: {item.get('message', '')}"
            for item in diagnostics
            if isinstance(item, dict)
        )
        raise FetchError(summary or f"trajecta exited {completed.returncode}")
    data = envelope.get("data")
    if not isinstance(data, dict):
        raise FetchError("trajecta JSON envelope has no object data")
    return data


def read_plan(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise FetchError(f"cannot read data plan {path}: {error}") from error
    if isinstance(value, dict) and value.get("schema_version") == "trajecta.cli-output/v1":
        value = value.get("data")
    if not isinstance(value, dict) or value.get("schema_version") != "trajecta.data-plan/v1":
        raise FetchError("plan must be a trajecta.data-plan/v1 object")
    requirements = value.get("requirements")
    if not isinstance(requirements, list):
        raise FetchError("data plan requirements must be an array")
    return value


def stable_requirement(requirement: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in requirement.items() if key != "status"}


def project_context(
    binary: Path, project: Path, supplied_plan: dict[str, Any]
) -> tuple[Path, dict[str, Any], dict[str, dict[str, Any]]]:
    show = cli_json(binary, ["--project", str(project), "project", "show"])
    current = cli_json(binary, ["--project", str(project), "project", "data-plan"])
    if current.get("project_sha256") != supplied_plan.get("project_sha256"):
        raise FetchError("data plan is stale for the selected project; regenerate it")
    supplied = [stable_requirement(item) for item in supplied_plan["requirements"]]
    active = [stable_requirement(item) for item in current.get("requirements", [])]
    if supplied != active:
        raise FetchError("data plan requirements differ from the selected project")

    root_value = show.get("root")
    index = show.get("index")
    if not isinstance(root_value, str) or not isinstance(index, dict):
        raise FetchError("project show returned an invalid root or index")
    root = Path(root_value).resolve()
    cases = index.get("cases")
    if not isinstance(cases, dict):
        raise FetchError("project index has no Case map")
    resolved: dict[str, dict[str, Any]] = {}
    for requirement in supplied_plan["requirements"]:
        case_name = requirement.get("case_name")
        relative = cases.get(case_name)
        if not isinstance(case_name, str) or not isinstance(relative, str):
            raise FetchError(f"data plan names an unknown Case: {case_name!r}")
        case_path = jailed_path(root, relative)
        resolved[case_name] = cli_json(binary, ["case", "resolve", str(case_path)])
    return root, index, resolved


def jailed_path(root: Path, relative: str) -> Path:
    pure = PurePosixPath(relative)
    if pure.is_absolute() or relative in {"", "."} or ".." in pure.parts or "\\" in relative:
        raise FetchError(f"project path is not a contained portable path: {relative!r}")
    root = root.resolve()
    candidate = (root / Path(*pure.parts)).resolve(strict=False)
    try:
        candidate.relative_to(root)
    except ValueError as error:
        raise FetchError(f"project path escapes its root: {relative!r}") from error
    return candidate


def anchors(start: int, end: int, interval: int) -> list[int]:
    lower = (min(start, end) // interval) * interval - interval
    upper = math.ceil(max(start, end) / interval) * interval + interval
    return list(range(lower, upper + 1, interval))


def grouped_anchors(values: list[int]) -> list[tuple[str, list[str]]]:
    grouped: dict[str, list[str]] = {}
    for value in values:
        instant = datetime.fromtimestamp(value, UTC)
        grouped.setdefault(instant.strftime("%Y-%m-%d"), []).append(instant.strftime("%H:%M"))
    return sorted(grouped.items())


def coordinate_pairs(value: Any) -> list[tuple[float, float]]:
    if (
        isinstance(value, list)
        and len(value) >= 2
        and isinstance(value[0], (int, float))
        and isinstance(value[1], (int, float))
    ):
        return [(float(value[0]), float(value[1]))]
    if isinstance(value, list):
        return [pair for child in value for pair in coordinate_pairs(child)]
    return []


def case_coordinates(value: Any) -> list[tuple[float, float]]:
    if isinstance(value, dict):
        pairs = coordinate_pairs(value["coordinates"]) if "coordinates" in value else []
        return pairs + [pair for child in value.values() for pair in case_coordinates(child)]
    if isinstance(value, list):
        return [pair for child in value for pair in case_coordinates(child)]
    return []


def request_area(case: dict[str, Any], profile: str) -> tuple[list[float], str]:
    pairs = case_coordinates(case)
    if pairs:
        longitudes = [pair[0] for pair in pairs]
        latitudes = [pair[1] for pair in pairs]
        west, east = min(longitudes) - 2.0, max(longitudes) + 2.0
        south, north = min(latitudes) - 2.0, max(latitudes) + 2.0
        if west < -180 or east > 180 or east - west > 180:
            return [90.0, -180.0, -90.0, 180.0], "Case geometry crosses longitude seam"
        return [min(90.0, north), west, max(-90.0, south), east], "Case geometry with 2-degree halo"
    text = json.dumps(case, sort_keys=True)
    if "global_periodic/v0" in text:
        return [90.0, -180.0, -90.0, 180.0], "global periodic Case"
    if profile in {"era5-cf-pressure-netcdf-v0", "era5-cds-hybrid137-v0"}:
        return TUTORIAL_AREA.copy(), "built-in tutorial domain"
    raise FetchError(
        f"Case has no spatial geometry and profile {profile!r} has no helper area default"
    )


def family(profile: str) -> str:
    lowered = profile.casefold()
    if lowered == GMTED2010_PROFILE:
        return "gmted2010"
    if lowered.startswith("cfsr-"):
        return "cfsr-pressure"
    if "era5" in lowered and "pressure" in lowered:
        return "era5-pressure"
    if "era5" in lowered and "hybrid" in lowered:
        return "era5-hybrid"
    raise FetchError(f"unsupported dataset profile: {profile}")


def target_root(project_root: Path, requirement: dict[str, Any]) -> tuple[Path, str]:
    roots = requirement.get("data_roots")
    if not isinstance(roots, dict) or not roots:
        raise FetchError("data-plan requirement has no data root")
    role = "met" if "met" in roots else sorted(roots)[0]
    relative = roots[role]
    if not isinstance(relative, str):
        raise FetchError("data root must be a project-relative string")
    return jailed_path(project_root, relative), relative


def cfsr_requests(values: list[int], root_relative: str) -> list[dict[str, Any]]:
    from fetch_cfsr_pgbl import URL_CANDIDATES

    requests = []
    for value in values:
        instant = datetime.fromtimestamp(value, UTC)
        stamp = instant.strftime("%Y%m%d%H")
        parts = {
            "yyyy": instant.strftime("%Y"),
            "yyyymm": instant.strftime("%Y%m"),
            "yyyymmdd": instant.strftime("%Y%m%d"),
            "stamp": stamp,
        }
        frozen = DEMO_CFSR.get(stamp)
        requests.append(
            {
                "provider": "NOAA NCEI",
                "dataset": "CFSR 6-hourly pressure-level GRIB2",
                "stamp": stamp,
                "urls": [template.format(**parts) for template in URL_CANDIDATES],
                "target": f"{root_relative.rstrip('/')}/pgbl00.gdas.{stamp}.grb2",
                "expected_size": frozen[0] if frozen else None,
                "expected_sha256": frozen[1] if frozen else None,
            }
        )
    return requests


def gmted2010_requests(root_relative: str) -> list[dict[str, Any]]:
    return [
        {
            "provider": "Trajecta frozen auxiliary-data release",
            "dataset": "USGS GMTED2010 30 arc-second mean and standard deviation",
            "urls": [f"{GMTED2010_RELEASE_BASE}/{name}"],
            "target": f"{root_relative.rstrip('/')}/{name}",
            "expected_size": size_bytes,
            "expected_sha256": sha256,
        }
        for name, size_bytes, sha256 in GMTED2010_FILES
    ]


def requirement_plan(
    project_root: Path, requirement: dict[str, Any], case: dict[str, Any]
) -> dict[str, Any]:
    profile = requirement["dataset_profile"]
    selected_family = family(profile)
    root, root_relative = target_root(project_root, requirement)
    if selected_family == "gmted2010":
        return {
            "profile_name": requirement["profile_name"],
            "case_name": requirement["case_name"],
            "dataset_id": requirement["dataset_id"],
            "dataset_profile": profile,
            "family": selected_family,
            "target_root": root_relative,
            "area_nswe": None,
            "area_source": "global prepared auxiliary dataset",
            "anchors_utc": [],
            "requests": gmted2010_requests(root_relative),
            "_target_root": root,
            "_anchor_values": [],
        }
    start = int(requirement["coverage_start"]["seconds_since_unix_epoch"])
    end = int(requirement["coverage_end"]["seconds_since_unix_epoch"])
    interval = 10_800 if selected_family == "era5-hybrid" else 21_600
    values = anchors(start, end, interval)
    area = None
    area_source = None
    requests: list[dict[str, Any]]
    if selected_family == "cfsr-pressure":
        requests = cfsr_requests(values, root_relative)
    else:
        area, area_source = request_area(case, profile)
        request_builder = (
            __import__("fetch_era5_pressure_cds").request_plan
            if selected_family == "era5-pressure"
            else __import__("era5_hybrid_official_pipeline").request_plan
        )
        requests = []
        for date, times in grouped_anchors(values):
            for item in request_builder(date, times, area):
                requests.append(
                    {
                        "provider": "Copernicus Climate Data Store",
                        "dataset": item["dataset"],
                        "request": item["request"],
                        "target": f"{root_relative.rstrip('/')}/{item['target']}",
                    }
                )
    return {
        "profile_name": requirement["profile_name"],
        "case_name": requirement["case_name"],
        "dataset_id": requirement["dataset_id"],
        "dataset_profile": profile,
        "family": selected_family,
        "target_root": root_relative,
        "area_nswe": area,
        "area_source": area_source,
        "anchors_utc": [datetime.fromtimestamp(value, UTC).isoformat() for value in values],
        "requests": requests,
        "_target_root": root,
        "_anchor_values": values,
    }


def public_plan(project_root: Path, planned: list[dict[str, Any]], execute: bool) -> dict[str, Any]:
    requirements = []
    for item in planned:
        clean = {key: value for key, value in item.items() if not key.startswith("_")}
        clean["files"] = file_states(project_root, clean["requests"])
        requirements.append(clean)
    return {
        "tool": "fetch_trajecta_data.py",
        "mode": "execute" if execute else "dry-run",
        "project": project_root.name,
        "requirements": requirements,
    }


def file_states(project_root: Path, requests: list[dict[str, Any]]) -> list[dict[str, Any]]:
    states = []
    for request in requests:
        target = jailed_path(project_root, request["target"])
        state: dict[str, Any] = {"path": request["target"], "state": "missing"}
        if target.is_file():
            digest = sha256_file(target)
            state |= {"state": "present", "size_bytes": target.stat().st_size, "sha256": digest}
            expected_size = request.get("expected_size")
            expected_sha = request.get("expected_sha256")
            if (expected_size is not None and target.stat().st_size != expected_size) or (
                expected_sha is not None and digest != expected_sha
            ):
                state["state"] = "conflict"
        states.append(state)
    return states


def fetch_fixed_files(project_root: Path, item: dict[str, Any]) -> list[dict[str, Any]]:
    from fetch_cfsr_pgbl import download

    completed = []
    for request in item["requests"]:
        target = jailed_path(project_root, request["target"])
        if target.is_file():
            digest = sha256_file(target)
            if (
                request["expected_size"] is not None
                and target.stat().st_size != request["expected_size"]
            ) or (
                request["expected_sha256"] is not None
                and digest != request["expected_sha256"]
            ):
                raise FetchError(f"existing file conflicts with frozen identity: {request['target']}")
            completed.append(file_identity(target, project_root))
            continue
        last_error: Exception | None = None
        for url in request["urls"]:
            try:
                download(
                    url,
                    target,
                    expected_size=request["expected_size"],
                    expected_sha256=request["expected_sha256"],
                )
                last_error = None
                break
            except Exception as error:  # provider candidates are deliberately sequential
                last_error = error
        if last_error is not None:
            label = request.get("stamp", request["target"])
            raise FetchError(f"all provider URLs failed for {label}: {last_error}")
        completed.append(file_identity(target, project_root))
    return completed


def helper_work(root: Path) -> Path:
    work = root / ".trajecta-fetch"
    marker = work / ".owner"
    if work.exists():
        if not marker.is_file() or marker.read_text(encoding="utf-8") != WORK_OWNER:
            raise FetchError(f"refusing unowned helper work directory: {work}")
    else:
        work.mkdir(parents=True)
        marker.write_text(WORK_OWNER, encoding="utf-8", newline="\n")
    return work


def run_checked(arguments: list[str]) -> None:
    completed = subprocess.run(
        arguments,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise FetchError(f"provider preparation command exited {completed.returncode}")


def promote_ready(work: Path, target_root: Path, project_root: Path) -> list[dict[str, Any]]:
    ready = work / "ready"
    if not ready.is_dir():
        raise FetchError("provider preparation produced no ready directory")
    destination = target_root / "ready"
    destination.mkdir(parents=True, exist_ok=True)
    completed = []
    for source in sorted(ready.glob("*.nc")):
        target = destination / source.name
        source_digest = sha256_file(source)
        if target.exists():
            if not target.is_file() or sha256_file(target) != source_digest:
                raise FetchError(f"prepared target already exists with different content: {target}")
            source.unlink()
        else:
            os.replace(source, target)
        completed.append(file_identity(target, project_root))
    if not completed:
        raise FetchError("provider preparation produced no NetCDF files")
    return completed


def fetch_era5(
    project_root: Path, item: dict[str, Any]
) -> list[dict[str, Any]]:
    root = item["_target_root"]
    work = helper_work(root)
    area = [str(value) for value in item["area_nswe"]]
    grouped = grouped_anchors(item["_anchor_values"])
    for date, times in grouped:
        if item["family"] == "era5-pressure":
            run_checked(
                [
                    sys.executable,
                    str(TOOLS / "fetch_era5_pressure_cds.py"),
                    "--out-dir",
                    str(work),
                    "--date",
                    date,
                    "--times",
                    *times,
                    "--area",
                    *area,
                ]
            )
            run_checked(
                [
                    sys.executable,
                    str(TOOLS / "prepare_era5_pressure_anchors.py"),
                    "--root",
                    str(work),
                    "--classic-dir",
                    str(work / "classic-unused"),
                    "--skip-classic",
                    "--date",
                    date,
                    "--times",
                    *(time.removesuffix(":00") for time in times),
                ]
            )
        else:
            run_checked(
                [
                    sys.executable,
                    str(TOOLS / "era5_hybrid_official_pipeline.py"),
                    "--out-dir",
                    str(work),
                    "--date",
                    date,
                    "--times",
                    *times,
                    "--area",
                    *area,
                ]
            )
    completed = promote_ready(work, root, project_root)
    shutil.rmtree(work)
    return completed


def file_identity(path: Path, project_root: Path) -> dict[str, Any]:
    return {
        "path": path.resolve().relative_to(project_root.resolve()).as_posix(),
        "size_bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def execute_plan(project_root: Path, planned: list[dict[str, Any]]) -> list[dict[str, Any]]:
    results = []
    for item in planned:
        root = item["_target_root"]
        root.mkdir(parents=True, exist_ok=True)
        files = (
            fetch_era5(project_root, item)
            if item["family"] in {"era5-pressure", "era5-hybrid"}
            else fetch_fixed_files(project_root, item)
        )
        manifest = {
            "tool": "fetch_trajecta_data.py",
            "dataset_profile": item["dataset_profile"],
            "requests": item["requests"],
            "files": files,
            "credentials_recorded": False,
            "dataset_lock_created": False,
        }
        manifest_path = root / "TRAJECTA_FETCH_MANIFEST.json"
        temporary = root / "TRAJECTA_FETCH_MANIFEST.json.tmp"
        temporary.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        os.replace(temporary, manifest_path)
        results.append({"dataset_id": item["dataset_id"], "files": files})
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    try:
        binary = locate_binary()
        supplied_plan = read_plan(args.plan.resolve())
        project_root, _index, cases = project_context(
            binary, args.project.resolve(), supplied_plan
        )
        planned = [
            requirement_plan(project_root, requirement, cases[requirement["case_name"]])
            for requirement in supplied_plan["requirements"]
        ]
        output = public_plan(project_root, planned, args.execute)
        if args.execute:
            output["completed"] = execute_plan(project_root, planned)
        print(json.dumps(output, indent=2, sort_keys=True))
        return 0
    except FetchError as error:
        print(f"fetch error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
