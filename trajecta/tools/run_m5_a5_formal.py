#!/usr/bin/env python3
"""Run the frozen M5-A5 FLEXPART/Trajecta comparison matrix."""

from __future__ import annotations

import argparse
import csv
import hashlib
import html
import json
import os
import platform
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator

import m5_a4_package as package
import run_m5_a4_product_matrix as product
import run_m5_a5_flexpart_comparison as comparison


ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "testdata" / "M5_FLEXPART_COMPARISON.schema.json"
FLEXPART_COMMIT = "dace3affa2ba71677f12f3858b04aaf59f8ee51e"
FLEXPART_BINARY_SHA256 = "5b6aacf1ac653c26dca2fd14e498d5e705bfdf7a38896a789869c3c11d24c629"
FLEXPART_PATCH_SHA256 = "da025642552e240c09d85429f8083c504c125b502e1a272be5fac5e30751fac4"
FLEXPART_SPECIES_SHA256 = "aabf59fdd1f51192a4843272b1b5b5d67a9d15cef05663d5fb4fca94a904f723"
COUNTS = (10_000, 50_000)
TIME_STEP_SECONDS = 600
OUTPUT_INTERVAL_SECONDS = 1_200
FORMAL_REPETITIONS = 3
WARMUP_ORDER = ("flexpart-core", "trajecta-product", "flexpart-product")
FORMAL_ORDERS = (
    ("flexpart-core", "flexpart-product", "trajecta-product"),
    ("trajecta-product", "flexpart-core", "flexpart-product"),
    ("flexpart-product", "trajecta-product", "flexpart-core"),
)


class FormalError(RuntimeError):
    """Stable M5-A5 formal execution failure."""


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _sha256_json(value: object) -> str:
    payload = json.dumps(
        value,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def path_normalized_provenance_digests(
    bundle_path: Path,
    data_root: Path,
    sqlite_sql_sha256: str,
) -> dict[str, str]:
    bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
    prefix = data_root.resolve().as_posix().rstrip("/") + "/"
    record_map: dict[str, str] = {}
    normalized_records = []
    for entry in bundle["records"]:
        body = entry["record"]
        sources = []
        for source in body["sources"]:
            if source.startswith(prefix):
                source = "dataset://" + source[len(prefix):]
            elif source.startswith("/"):
                raise FormalError(f"provenance source escapes prepared data root: {source}")
            sources.append(source)
        normalized = {**body, "sources": sources}
        digest = _sha256_json(normalized)
        exact = entry["sha256"]
        if exact in record_map:
            raise FormalError(f"duplicate provenance record SHA: {exact}")
        record_map[exact] = digest
        normalized_records.append(digest)

    field_set_map: dict[str, str] = {}
    normalized_field_sets = []
    for entry in bundle["field_sets"]:
        fields = {name: record_map[value] for name, value in entry["fields"].items()}
        digest = _sha256_json(fields)
        exact = entry["sha256"]
        if exact in field_set_map:
            raise FormalError(f"duplicate provenance field-set SHA: {exact}")
        field_set_map[exact] = digest
        normalized_field_sets.append(digest)

    content = hashlib.sha256()
    content.update(b"trajecta.m5-a5-path-normalized-provenance/v1\n")
    content.update(b"record_hash_algorithm=sha256-json-sorted-utf8\n")
    for digest in sorted(normalized_records):
        content.update(f"record={digest}\n".encode())
    for digest in sorted(normalized_field_sets):
        content.update(f"field_set={digest}\n".encode())
    samples = sorted(
        bundle["samples"],
        key=lambda sample: (sample["particle_id"], sample["sample_sequence"]),
    )
    for sample in samples:
        field_set = field_set_map[sample["field_set_sha256"]]
        content.update(
            f"sample={sample['particle_id']},{sample['sample_sequence']},{field_set}\n".encode()
        )
    provenance = content.hexdigest()
    canonical = hashlib.sha256()
    canonical.update(b"trajecta.m5-a5-path-normalized-canonical-output/v1\n")
    canonical.update(f"sqlite_sql_sha256={sqlite_sql_sha256}\n".encode())
    canonical.update(f"provenance_content_sha256={provenance}\n".encode())
    return {
        "provenance_content_sha256": provenance,
        "canonical_output_sha256": canonical.hexdigest(),
    }


def command_output(command: list[str], *, cwd: Path | None = None) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise FormalError(f"command failed ({completed.returncode}): {' '.join(command)}: {detail}")
    return completed.stdout.strip()


def require_sha(path: Path, expected: str, label: str) -> dict[str, object]:
    if not path.is_file():
        raise FormalError(f"{label} missing: {path}")
    actual = package.sha256(path)
    if actual != expected:
        raise FormalError(f"{label} identity mismatch: {actual} != {expected}")
    return {"path": str(path.resolve()), "size_bytes": path.stat().st_size, "sha256": actual}


def flexpart_identity(arguments: argparse.Namespace) -> dict[str, object]:
    source = arguments.flexpart_source.resolve()
    if command_output(["git", "-C", str(source), "rev-parse", "HEAD"]) != FLEXPART_COMMIT:
        raise FormalError("FLEXPART source commit drifted")
    if command_output(["git", "-C", str(source), "status", "--porcelain"]):
        raise FormalError("FLEXPART source checkout is dirty")
    binary = require_sha(arguments.flexpart_binary.resolve(), FLEXPART_BINARY_SHA256, "FLEXPART binary")
    patch = require_sha(
        (ROOT / "tools" / "flexpart_11_1_native_omega.patch").resolve(),
        FLEXPART_PATCH_SHA256,
        "FLEXPART omega adapter patch",
    )
    species = require_sha(arguments.species_file.resolve(), FLEXPART_SPECIES_SHA256, "FLEXPART species")
    return {
        "commit": FLEXPART_COMMIT,
        "source": str(source),
        "binary": binary,
        "adapter_patch": patch,
        "species": species,
        "compiler": command_output(["gfortran", "--version"]).splitlines()[0],
        "eccodes": command_output(["pkg-config", "--modversion", "eccodes"]),
        "netcdf_fortran": command_output(["nf-config", "--version"]),
    }


def host_identity(cpu_set: str) -> dict[str, object]:
    os_release = {}
    for line in Path("/etc/os-release").read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            os_release[key] = value.strip('"')
    affinity = sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else []
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "os_release": os_release,
        "host_affinity": affinity,
        "formal_cpu_set": cpu_set,
    }


def run_flexpart(
    arguments: argparse.Namespace,
    root: Path,
    particles: int,
    mode: str,
) -> dict[str, object]:
    prepared = argparse.Namespace(
        output=root,
        flexpart_options=arguments.flexpart_options,
        species_file=arguments.species_file,
        meteorology=arguments.meteorology,
        particles=particles,
        mode=mode,
        time_step_seconds=TIME_STEP_SECONDS,
        output_interval_seconds=OUTPUT_INTERVAL_SECONDS,
    )
    if comparison.prepare(prepared) != 0:
        raise FormalError(f"FLEXPART {mode} preparation failed")
    executed = argparse.Namespace(
        binary=arguments.flexpart_binary,
        case=root,
        cpu_set=arguments.cpu_set,
        timeout_seconds=arguments.timeout_seconds,
    )
    result_code = comparison.run_flexpart(executed)
    result_path = root / "M5_A5_FLEXPART_RUN.json"
    result = json.loads(result_path.read_text(encoding="utf-8"))
    if result_code != 0 or result.get("status") != "passed":
        raise FormalError(f"FLEXPART {mode} run failed: {result_path}")
    return {
        "result_path": str(result_path),
        "result_sha256": package.sha256(result_path),
        "case_root": str(root),
        "time_verbose": result["time_verbose"],
        "wall_nanoseconds": result["wall_nanoseconds"],
        "binary_sha256": result["binary_sha256"],
        "artifact_bytes": sum(int(item["size_bytes"]) for item in result["artifacts"]),
        "artifact_sha256": {item["path"]: item["sha256"] for item in result["artifacts"]},
    }


def run_trajecta(
    arguments: argparse.Namespace,
    root: Path,
    particles: int,
) -> dict[str, object]:
    prepared = argparse.Namespace(
        output=root,
        data=arguments.trajecta_data,
        particles=particles,
        time_step_seconds=TIME_STEP_SECONDS,
        output_interval_seconds=OUTPUT_INTERVAL_SECONDS,
    )
    if comparison.prepare_trajecta(prepared) != 0:
        raise FormalError("Trajecta case preparation failed")
    executed = argparse.Namespace(
        package_root=arguments.package_root,
        case=root,
        cpu_set=arguments.cpu_set,
        timeout_seconds=arguments.timeout_seconds,
        require_performance=True,
    )
    result_code = comparison.run_trajecta(executed)
    result_path = root / "M5_A5_TRAJECTA_RUN.json"
    result = json.loads(result_path.read_text(encoding="utf-8"))
    if result_code != 0 or result.get("status") != "passed":
        raise FormalError(f"Trajecta product run failed: {result_path}")
    output = Path(result["run"]["output_directory"])
    manifest = json.loads((output / "run-manifest.json").read_text(encoding="utf-8"))
    provenance = manifest["provenance"]
    path_normalized = path_normalized_provenance_digests(
        output / "provenance-bundle.json",
        root / "project" / "data",
        provenance["sqlite_sql_sha256"],
    )
    return {
        "result_path": str(result_path),
        "result_sha256": package.sha256(result_path),
        "case_root": str(root),
        "time_verbose": result["run"]["time_verbose"],
        "wall_nanoseconds": result["run"]["wall_nanoseconds"],
        "runner_milliseconds": result["run"]["runner_milliseconds"],
        "performance": result["run"]["performance_attribution"],
        "output_directory": str(output),
        "digests": {
            "content_sha256": provenance["content_sha256"],
            "sqlite_sql_sha256": provenance["sqlite_sql_sha256"],
            "canonical_output_sha256": provenance["canonical_output_sha256"],
        },
        "path_normalized_digests": path_normalized,
        "terminations": manifest["terminations"],
        "sqlite": manifest["sqlite"],
    }


def execute_mode(
    arguments: argparse.Namespace,
    root: Path,
    particles: int,
    mode: str,
) -> dict[str, object]:
    if mode == "flexpart-core":
        result = run_flexpart(arguments, root, particles, "core")
    elif mode == "flexpart-product":
        result = run_flexpart(arguments, root, particles, "product")
    elif mode == "trajecta-product":
        result = run_trajecta(arguments, root, particles)
    else:
        raise FormalError(f"unknown formal mode: {mode}")
    return {"mode": mode, "particles": particles, **result}


def compare_science(
    flexpart: dict[str, object],
    trajecta: dict[str, object],
    output: Path,
) -> dict[str, object]:
    arguments = argparse.Namespace(
        flexpart_case=Path(str(flexpart["case_root"])),
        trajecta_run=Path(str(trajecta["result_path"])),
        output=output,
    )
    if comparison.scientific_comparison(arguments) != 0:
        raise FormalError(f"scientific comparison failed: {output}")
    value = json.loads(output.read_text(encoding="utf-8"))
    if value.get("status") != "passed":
        raise FormalError(f"scientific comparison did not pass: {output}")
    return {"path": str(output), "sha256": package.sha256(output), "value": value}


def run_record(
    particles: int,
    phase: str,
    repetition: int,
    order_index: int,
    result: dict[str, object],
) -> dict[str, object]:
    return {
        "particles": particles,
        "phase": phase,
        "repetition": repetition,
        "order_index": order_index,
        **result,
    }


def timing_seconds(record: dict[str, object], kind: str) -> float:
    if record["mode"] == "flexpart-core" and kind == "core":
        return float(record["time_verbose"]["elapsed_seconds"])
    if record["mode"] == "flexpart-product" and kind == "product":
        return float(record["time_verbose"]["elapsed_seconds"])
    if record["mode"] == "trajecta-product":
        performance = record["performance"]
        if kind == "core":
            return float(performance["equivalent_core_nanoseconds"]) / 1_000_000_000
        if kind == "product":
            return float(performance["runner_total_nanoseconds"]) / 1_000_000_000
    raise FormalError(f"timing kind {kind} is unavailable for {record['mode']}")


def peak_rss_bytes(record: dict[str, object]) -> int:
    if record["mode"].startswith("flexpart-"):
        return int(record["time_verbose"]["peak_rss_kib"]) * 1024
    value = record["performance"].get("process_peak_rss_bytes")
    if not isinstance(value, int) or value <= 0:
        raise FormalError("Trajecta worker peak RSS is missing")
    return value


def formal_records(records: list[dict[str, object]], particles: int, mode: str) -> list[dict[str, object]]:
    return [
        record
        for record in records
        if record["phase"] == "formal"
        and record["particles"] == particles
        and record["mode"] == mode
    ]


def median_metric(records: list[dict[str, object]], kind: str) -> float:
    return float(statistics.median(timing_seconds(record, kind) for record in records))


def count_core_gate(records: list[dict[str, object]], particles: int) -> dict[str, object]:
    flex_core = formal_records(records, particles, "flexpart-core")
    trajecta = formal_records(records, particles, "trajecta-product")
    if len(flex_core) != FORMAL_REPETITIONS or len(trajecta) != FORMAL_REPETITIONS:
        raise FormalError(f"incomplete core timing rows for {particles}")
    flexpart_median = median_metric(flex_core, "core")
    trajecta_median = median_metric(trajecta, "core")
    ratio = trajecta_median / flexpart_median
    return {
        "flexpart_core_median_seconds": flexpart_median,
        "trajecta_equivalent_core_median_seconds": trajecta_median,
        "equivalent_core_ratio_trajecta_over_flexpart": ratio,
        "limit": 1.5,
        "passed": ratio <= 1.5,
    }


def performance_summary(records: list[dict[str, object]]) -> dict[str, object]:
    result = {}
    for particles in COUNTS:
        flex_core = formal_records(records, particles, "flexpart-core")
        flex_product = formal_records(records, particles, "flexpart-product")
        trajecta = formal_records(records, particles, "trajecta-product")
        if not all(len(rows) == FORMAL_REPETITIONS for rows in (flex_core, flex_product, trajecta)):
            raise FormalError(f"incomplete formal timing rows for {particles}")
        core_gate = count_core_gate(records, particles)
        flex_core_median = float(core_gate["flexpart_core_median_seconds"])
        flex_product_median = median_metric(flex_product, "product")
        trajecta_core_median = float(core_gate["trajecta_equivalent_core_median_seconds"])
        trajecta_product_median = median_metric(trajecta, "product")
        ratio = float(core_gate["equivalent_core_ratio_trajecta_over_flexpart"])
        result[str(particles)] = {
            "flexpart_core_median_seconds": flex_core_median,
            "trajecta_equivalent_core_median_seconds": trajecta_core_median,
            "equivalent_core_ratio_trajecta_over_flexpart": ratio,
            "equivalent_core_gate_passed": ratio <= 1.5,
            "flexpart_product_median_seconds": flex_product_median,
            "trajecta_product_median_seconds": trajecta_product_median,
            "flexpart_core_throughput_particles_per_second": particles / flex_core_median,
            "trajecta_core_throughput_particles_per_second": particles / trajecta_core_median,
            "flexpart_product_throughput_particles_per_second": particles / flex_product_median,
            "trajecta_product_throughput_particles_per_second": particles / trajecta_product_median,
            "flexpart_core_peak_rss_median_bytes": int(
                statistics.median(peak_rss_bytes(record) for record in flex_core)
            ),
            "flexpart_product_peak_rss_median_bytes": int(
                statistics.median(peak_rss_bytes(record) for record in flex_product)
            ),
            "trajecta_product_peak_rss_median_bytes": int(
                statistics.median(peak_rss_bytes(record) for record in trajecta)
            ),
            "formal_repetitions": FORMAL_REPETITIONS,
        }
    result["scaling"] = {
        "flexpart_core_50k_over_10k": result["50000"]["flexpart_core_median_seconds"]
        / result["10000"]["flexpart_core_median_seconds"],
        "trajecta_core_50k_over_10k": result["50000"]["trajecta_equivalent_core_median_seconds"]
        / result["10000"]["trajecta_equivalent_core_median_seconds"],
        "flexpart_product_50k_over_10k": result["50000"]["flexpart_product_median_seconds"]
        / result["10000"]["flexpart_product_median_seconds"],
        "trajecta_product_50k_over_10k": result["50000"]["trajecta_product_median_seconds"]
        / result["10000"]["trajecta_product_median_seconds"],
    }
    return result


def scientific_summary(reports: list[dict[str, object]]) -> dict[str, object]:
    result = {}
    for particles in COUNTS:
        selected = [report["value"] for report in reports if report["particles"] == particles]
        if len(selected) != FORMAL_REPETITIONS:
            raise FormalError(f"incomplete scientific reports for {particles}")
        rows = []
        for index, physical_time in enumerate(selected[0]["common_output_times_unix"]):
            fields = (
                "horizontal_centroid_separation_m",
                "vertical_centroid_difference_m",
                "transport_distance_difference_m",
                "dispersion_width_ratio_trajecta_over_flexpart",
                "occupancy_ratio_trajecta_over_flexpart",
            )
            row = {"physical_time_unix": physical_time}
            for field in fields:
                values = [report["cross_model"][index][field] for report in selected]
                row[field] = float(statistics.median(values)) if all(
                    isinstance(value, (int, float)) for value in values
                ) else None
            rows.append(row)
        result[str(particles)] = {"median_cross_model_by_time": rows}
    result["comparability"] = reports[0]["value"]["comparability"]
    return result


def determinism_gates(records: list[dict[str, object]]) -> dict[str, bool]:
    gates = {}
    for particles in COUNTS:
        trajecta = formal_records(records, particles, "trajecta-product")
        gates[f"trajecta_{particles}_sqlite_sql_sha256"] = len(
            {record["digests"]["sqlite_sql_sha256"] for record in trajecta}
        ) == 1
        for digest in ("provenance_content_sha256", "canonical_output_sha256"):
            gates[f"trajecta_{particles}_path_normalized_{digest}"] = len(
                {record["path_normalized_digests"][digest] for record in trajecta}
            ) == 1
    return gates


def validate_matrix_records(
    records: list[dict[str, object]], reports: list[dict[str, object]]
) -> None:
    expected_total = len(COUNTS) * (
        len(WARMUP_ORDER) + FORMAL_REPETITIONS * len(WARMUP_ORDER)
    )
    if len(records) != expected_total:
        raise FormalError(f"formal record count {len(records)} != {expected_total}")
    for particles in COUNTS:
        warmups = [
            record
            for record in records
            if record["particles"] == particles and record["phase"] == "warmup"
        ]
        warmups.sort(key=lambda record: int(record["order_index"]))
        if [record["mode"] for record in warmups] != list(WARMUP_ORDER) or any(
            record["repetition"] != 0 for record in warmups
        ):
            raise FormalError(f"warm-up order drifted for {particles}")
        for repetition, expected_order in enumerate(FORMAL_ORDERS, start=1):
            selected = [
                record
                for record in records
                if record["particles"] == particles
                and record["phase"] == "formal"
                and record["repetition"] == repetition
            ]
            selected.sort(key=lambda record: int(record["order_index"]))
            if [record["mode"] for record in selected] != list(expected_order):
                raise FormalError(
                    f"formal order drifted for {particles} repetition {repetition}"
                )
    expected_reports = {
        (particles, repetition)
        for particles in COUNTS
        for repetition in range(1, FORMAL_REPETITIONS + 1)
    }
    actual_reports = {
        (int(report["particles"]), int(report["repetition"])) for report in reports
    }
    if len(reports) != len(expected_reports) or actual_reports != expected_reports:
        raise FormalError("scientific report coverage is incomplete or duplicated")


def write_timing_csv(path: Path, records: list[dict[str, object]]) -> None:
    fields = (
        "particles",
        "phase",
        "repetition",
        "order_index",
        "mode",
        "core_seconds",
        "product_seconds",
        "peak_rss_bytes",
        "user_seconds",
        "system_seconds",
        "filesystem_inputs",
        "filesystem_outputs",
        "result_path",
    )
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        for record in records:
            writer.writerow(
                {
                    "particles": record["particles"],
                    "phase": record["phase"],
                    "repetition": record["repetition"],
                    "order_index": record["order_index"],
                    "mode": record["mode"],
                    "core_seconds": (
                        timing_seconds(record, "core")
                        if record["mode"] in {"flexpart-core", "trajecta-product"}
                        else ""
                    ),
                    "product_seconds": (
                        timing_seconds(record, "product")
                        if record["mode"] in {"flexpart-product", "trajecta-product"}
                        else ""
                    ),
                    "peak_rss_bytes": peak_rss_bytes(record),
                    "user_seconds": record["time_verbose"]["user_seconds"],
                    "system_seconds": record["time_verbose"]["system_seconds"],
                    "filesystem_inputs": record["time_verbose"]["filesystem_inputs"],
                    "filesystem_outputs": record["time_verbose"]["filesystem_outputs"],
                    "result_path": record["result_path"],
                }
            )


def write_science_csv(path: Path, reports: list[dict[str, object]]) -> None:
    fields = (
        "particles",
        "repetition",
        "physical_time_unix",
        "horizontal_centroid_separation_m",
        "vertical_centroid_difference_m",
        "transport_distance_difference_m",
        "dispersion_width_ratio_trajecta_over_flexpart",
        "occupancy_ratio_trajecta_over_flexpart",
    )
    with path.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields, lineterminator="\n")
        writer.writeheader()
        for report in reports:
            for row in report["value"]["cross_model"]:
                writer.writerow(
                    {
                        "particles": report["particles"],
                        "repetition": report["repetition"],
                        **row,
                    }
                )


def svg_bar_chart(path: Path, title: str, groups: list[str], series: list[tuple[str, str, list[float]]], unit: str) -> None:
    width, height = 920, 540
    left, top, right, bottom = 90, 70, 30, 90
    plot_width = width - left - right
    plot_height = height - top - bottom
    maximum = max(value for _name, _color, values in series for value in values) * 1.12
    bar_width = plot_width / (len(groups) * (len(series) + 1))
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{width/2}" y="35" text-anchor="middle" font-family="sans-serif" font-size="22" font-weight="600">{html.escape(title)}</text>',
        f'<line x1="{left}" y1="{top}" x2="{left}" y2="{top+plot_height}" stroke="#334155"/>',
        f'<line x1="{left}" y1="{top+plot_height}" x2="{left+plot_width}" y2="{top+plot_height}" stroke="#334155"/>',
    ]
    for tick in range(6):
        value = maximum * tick / 5
        y = top + plot_height - plot_height * tick / 5
        parts.append(f'<line x1="{left}" y1="{y:.2f}" x2="{left+plot_width}" y2="{y:.2f}" stroke="#e2e8f0"/>')
        parts.append(f'<text x="{left-10}" y="{y+4:.2f}" text-anchor="end" font-family="sans-serif" font-size="12">{value:.2f}</text>')
    for group_index, group in enumerate(groups):
        group_start = left + plot_width * group_index / len(groups)
        for series_index, (name, color, values) in enumerate(series):
            value = values[group_index]
            x = group_start + bar_width * (series_index + 0.6)
            bar_height = plot_height * value / maximum if maximum else 0
            y = top + plot_height - bar_height
            parts.append(f'<rect x="{x:.2f}" y="{y:.2f}" width="{bar_width*0.82:.2f}" height="{bar_height:.2f}" fill="{color}" rx="3"/>')
            parts.append(f'<text x="{x+bar_width*0.41:.2f}" y="{y-6:.2f}" text-anchor="middle" font-family="sans-serif" font-size="11">{value:.2f}</text>')
        parts.append(f'<text x="{group_start+plot_width/len(groups)/2:.2f}" y="{top+plot_height+28}" text-anchor="middle" font-family="sans-serif" font-size="14">{html.escape(group)}</text>')
    legend_x = left
    for name, color, _values in series:
        parts.append(f'<rect x="{legend_x}" y="{height-34}" width="14" height="14" fill="{color}"/>')
        parts.append(f'<text x="{legend_x+20}" y="{height-22}" font-family="sans-serif" font-size="13">{html.escape(name)}</text>')
        legend_x += 220
    parts.append(f'<text x="18" y="{top+plot_height/2}" transform="rotate(-90 18 {top+plot_height/2})" text-anchor="middle" font-family="sans-serif" font-size="13">{html.escape(unit)}</text>')
    parts.append("</svg>")
    path.write_text("\n".join(parts) + "\n", encoding="utf-8")


def science_svg(path: Path, scientific: dict[str, object]) -> None:
    width, height = 920, 620
    panels = (
        ("Horizontal centroid separation", "horizontal_centroid_separation_m", "m", 70),
        ("Vertical centroid difference", "vertical_centroid_difference_m", "m", 340),
    )
    colors = {"10000": "#2563eb", "50000": "#dc2626"}
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">',
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        '<text x="460" y="34" text-anchor="middle" font-family="sans-serif" font-size="22" font-weight="600">M5-A5 ensemble scientific comparison</text>',
    ]
    for title, field, unit, top in panels:
        left, plot_width, plot_height = 90, 790, 200
        all_values = [
            float(row[field])
            for count in ("10000", "50000")
            for row in scientific[count]["median_cross_model_by_time"]
        ]
        low, high = min(all_values), max(all_values)
        margin = max((high - low) * 0.12, 1.0)
        low -= margin
        high += margin
        parts.append(f'<text x="{left}" y="{top-18}" font-family="sans-serif" font-size="16" font-weight="600">{html.escape(title)}</text>')
        parts.append(f'<rect x="{left}" y="{top}" width="{plot_width}" height="{plot_height}" fill="#f8fafc" stroke="#cbd5e1"/>')
        for count in ("10000", "50000"):
            rows = scientific[count]["median_cross_model_by_time"]
            points = []
            for index, row in enumerate(rows):
                x = left + plot_width * index / (len(rows) - 1)
                y = top + plot_height - plot_height * (float(row[field]) - low) / (high - low)
                points.append(f"{x:.2f},{y:.2f}")
            parts.append(f'<polyline points="{" ".join(points)}" fill="none" stroke="{colors[count]}" stroke-width="3"/>')
        parts.append(f'<text x="{left-12}" y="{top+8}" text-anchor="end" font-family="sans-serif" font-size="12">{high:.1f}</text>')
        parts.append(f'<text x="{left-12}" y="{top+plot_height}" text-anchor="end" font-family="sans-serif" font-size="12">{low:.1f}</text>')
        parts.append(f'<text x="24" y="{top+plot_height/2}" transform="rotate(-90 24 {top+plot_height/2})" text-anchor="middle" font-family="sans-serif" font-size="12">{unit}</text>')
        for index, minute in enumerate((10, 20, 30, 40, 50, 60)):
            x = left + plot_width * index / 5
            parts.append(f'<text x="{x:.2f}" y="{top+plot_height+22}" text-anchor="middle" font-family="sans-serif" font-size="12">{minute}</text>')
    parts.extend(
        [
            '<line x1="90" y1="594" x2="112" y2="594" stroke="#2563eb" stroke-width="3"/><text x="120" y="598" font-family="sans-serif" font-size="13">10k median</text>',
            '<line x1="260" y1="594" x2="282" y2="594" stroke="#dc2626" stroke-width="3"/><text x="290" y="598" font-family="sans-serif" font-size="13">50k median</text>',
            '<text x="760" y="598" font-family="sans-serif" font-size="12">minutes since release</text>',
            "</svg>",
        ]
    )
    path.write_text("\n".join(parts) + "\n", encoding="utf-8")


def create_charts(root: Path, performance: dict[str, object], scientific: dict[str, object]) -> dict[str, str]:
    charts = root / "charts"
    charts.mkdir()
    groups = ["10k", "50k"]
    core = charts / "core-wall-time.svg"
    svg_bar_chart(
        core,
        "Equivalent core median wall time",
        groups,
        [
            ("FLEXPART core", "#0f766e", [performance["10000"]["flexpart_core_median_seconds"], performance["50000"]["flexpart_core_median_seconds"]]),
            ("Trajecta equivalent core", "#2563eb", [performance["10000"]["trajecta_equivalent_core_median_seconds"], performance["50000"]["trajecta_equivalent_core_median_seconds"]]),
        ],
        "seconds",
    )
    product_chart = charts / "complete-product-wall-time.svg"
    svg_bar_chart(
        product_chart,
        "Complete product median wall time",
        groups,
        [
            ("FLEXPART particle NetCDF", "#0f766e", [performance["10000"]["flexpart_product_median_seconds"], performance["50000"]["flexpart_product_median_seconds"]]),
            ("Trajecta SQLite/provenance", "#2563eb", [performance["10000"]["trajecta_product_median_seconds"], performance["50000"]["trajecta_product_median_seconds"]]),
        ],
        "seconds",
    )
    throughput = charts / "throughput-scaling.svg"
    svg_bar_chart(
        throughput,
        "Equivalent core throughput",
        groups,
        [
            ("FLEXPART", "#0f766e", [performance["10000"]["flexpart_core_throughput_particles_per_second"], performance["50000"]["flexpart_core_throughput_particles_per_second"]]),
            ("Trajecta", "#2563eb", [performance["10000"]["trajecta_core_throughput_particles_per_second"], performance["50000"]["trajecta_core_throughput_particles_per_second"]]),
        ],
        "particles / second",
    )
    rss = charts / "peak-rss.svg"
    svg_bar_chart(
        rss,
        "Median peak resident memory",
        groups,
        [
            ("FLEXPART core", "#0f766e", [performance["10000"]["flexpart_core_peak_rss_median_bytes"] / 2**20, performance["50000"]["flexpart_core_peak_rss_median_bytes"] / 2**20]),
            ("FLEXPART product", "#65a30d", [performance["10000"]["flexpart_product_peak_rss_median_bytes"] / 2**20, performance["50000"]["flexpart_product_peak_rss_median_bytes"] / 2**20]),
            ("Trajecta product", "#2563eb", [performance["10000"]["trajecta_product_peak_rss_median_bytes"] / 2**20, performance["50000"]["trajecta_product_peak_rss_median_bytes"] / 2**20]),
        ],
        "MiB",
    )
    science = charts / "scientific-comparison.svg"
    science_svg(science, scientific)
    return {
        path.name: package.sha256(path)
        for path in (core, product_chart, throughput, rss, science)
    }


def validate_final(value: dict[str, object]) -> None:
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    Draft202012Validator.check_schema(schema)
    Draft202012Validator(schema).validate(value)


def execute(arguments: argparse.Namespace) -> int:
    root = arguments.artifact_root.resolve()
    if root.exists():
        raise FormalError(f"refusing to overwrite formal artifact root: {root}")
    root.mkdir(parents=True)
    aggregate_path = root / "M5_A5_FLEXPART_COMPARISON.json"
    records: list[dict[str, object]] = []
    science_reports: list[dict[str, object]] = []
    partial: dict[str, object] = {
        "schema_version": "trajecta.m5-a5-flexpart-comparison/v1",
        "status": "running",
        "records": records,
        "scientific_reports": [],
    }
    write_json(aggregate_path, partial)
    try:
        identity = product.validate_product_root(arguments.package_root)
        build_manifest = json.loads((identity.package_root / package.MANIFEST_NAME).read_text(encoding="utf-8"))
        if build_manifest.get("version") != "0.0.0":
            raise FormalError("A5 formal comparison must run at version 0.0.0")
        flexpart = flexpart_identity(arguments)
        field_path = root / "M5_A5_METEOROLOGY_EQUIVALENCE.json"
        field_args = argparse.Namespace(
            source=arguments.source,
            trajecta_data=arguments.trajecta_data,
            meteorology=arguments.meteorology,
            output=field_path,
        )
        if comparison.audit_meteorology(field_args) != 0:
            raise FormalError("formal meteorology equivalence audit failed")
        for particles in COUNTS:
            for order_index, mode in enumerate(WARMUP_ORDER):
                run_root = root / "runs" / f"p{particles}" / "warmup" / mode
                result = execute_mode(arguments, run_root, particles, mode)
                records.append(run_record(particles, "warmup", 0, order_index, result))
                write_json(aggregate_path, partial)
            for repetition, order in enumerate(FORMAL_ORDERS, start=1):
                by_mode = {}
                for order_index, mode in enumerate(order):
                    run_root = root / "runs" / f"p{particles}" / f"repeat-{repetition}" / mode
                    result = execute_mode(arguments, run_root, particles, mode)
                    record = run_record(particles, "formal", repetition, order_index, result)
                    records.append(record)
                    by_mode[mode] = record
                    write_json(aggregate_path, partial)
                report = compare_science(
                    by_mode["flexpart-product"],
                    by_mode["trajecta-product"],
                    root / "science" / f"p{particles}-repeat-{repetition}.json",
                )
                science_reports.append(
                    {"particles": particles, "repetition": repetition, **report}
                )
                partial["scientific_reports"] = [
                    {key: value for key, value in item.items() if key != "value"}
                    for item in science_reports
                ]
                write_json(aggregate_path, partial)
            core_gate = count_core_gate(records, particles)
            partial.setdefault("performance_gates", {})[str(particles)] = core_gate
            write_json(aggregate_path, partial)
            if not core_gate["passed"]:
                raise FormalError(
                    f"{particles} equivalent-core ratio "
                    f"{core_gate['equivalent_core_ratio_trajecta_over_flexpart']:.6f} "
                    f"exceeds {core_gate['limit']:.1f}"
                )

        validate_matrix_records(records, science_reports)
        performance = performance_summary(records)
        scientific = scientific_summary(science_reports)
        determinism = determinism_gates(records)
        gates = {
            "meteorology_equivalence": True,
            "all_runs_passed": True,
            "all_scientific_reports_passed": True,
            "core_ratio_10k": performance["10000"]["equivalent_core_gate_passed"],
            "core_ratio_50k": performance["50000"]["equivalent_core_gate_passed"],
            **determinism,
        }
        raw = root / "raw"
        raw.mkdir()
        timing_csv = raw / "M5_A5_TIMINGS.csv"
        science_csv = raw / "M5_A5_SCIENCE.csv"
        write_timing_csv(timing_csv, records)
        write_science_csv(science_csv, science_reports)
        charts = create_charts(root, performance, scientific)
        value = {
            "schema_version": "trajecta.m5-a5-flexpart-comparison/v1",
            "status": "passed" if all(gates.values()) else "failed",
            "contract": {
                "particle_counts": list(COUNTS),
                "warmups_per_count": 1,
                "formal_repetitions_per_count": FORMAL_REPETITIONS,
                "simulation_seconds": 3_600,
                "time_step_seconds": TIME_STEP_SECONDS,
                "output_interval_seconds": OUTPUT_INTERVAL_SECONDS,
                "cpu_set": arguments.cpu_set,
                "workers_or_threads": 4,
                "core_ratio_limit": 1.5,
                "version": "0.0.0",
            },
            "host": host_identity(arguments.cpu_set),
            "trajecta": {
                "package_root": str(identity.package_root),
                "source_tree_sha256": identity.source_tree_sha256,
                "build_manifest_sha256": identity.build_manifest_sha256,
                "binary_sha256": identity.binary_sha256,
            },
            "flexpart": flexpart,
            "meteorology_equivalence": {
                "path": str(field_path),
                "sha256": package.sha256(field_path),
            },
            "orders": {
                "warmup": list(WARMUP_ORDER),
                "formal": [list(order) for order in FORMAL_ORDERS],
            },
            "records": records,
            "scientific_reports": [
                {key: item[key] for key in ("particles", "repetition", "path", "sha256")}
                for item in science_reports
            ],
            "performance": performance,
            "scientific": scientific,
            "gates": gates,
            "artifacts": {
                "timing_csv": {"path": str(timing_csv), "sha256": package.sha256(timing_csv)},
                "science_csv": {"path": str(science_csv), "sha256": package.sha256(science_csv)},
                "charts": charts,
            },
        }
        validate_final(value)
        write_json(aggregate_path, value)
        print(aggregate_path)
        return 0 if value["status"] == "passed" else 1
    except Exception as error:
        partial["status"] = "failed"
        partial["failure"] = str(error)
        partial["scientific_reports"] = [
            {key: value for key, value in item.items() if key != "value"}
            for item in science_reports
        ]
        write_json(aggregate_path, partial)
        raise


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--artifact-root", type=Path, required=True)
    result.add_argument("--package-root", type=Path, required=True)
    result.add_argument("--flexpart-source", type=Path, required=True)
    result.add_argument("--flexpart-binary", type=Path, required=True)
    result.add_argument("--flexpart-options", type=Path, required=True)
    result.add_argument("--species-file", type=Path, required=True)
    result.add_argument("--source", type=Path, required=True)
    result.add_argument("--meteorology", type=Path, required=True)
    result.add_argument("--trajecta-data", type=Path, required=True)
    result.add_argument("--cpu-set", default="0-3")
    result.add_argument("--timeout-seconds", type=int, default=3_600)
    return result


def main() -> int:
    try:
        return execute(parser().parse_args())
    except (FormalError, comparison.ComparisonError, OSError, ValueError, KeyError) as error:
        print(f"m5-a5 formal: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
