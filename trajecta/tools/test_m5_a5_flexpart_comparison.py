#!/usr/bin/env python3
"""Focused tests for the M5-A5 FLEXPART comparison configuration."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

import numpy

import run_m5_a5_flexpart_comparison as comparison
import run_m5_a5_formal as formal


class FlexpartComparisonTests(unittest.TestCase):
    def test_command_modes_differ_only_in_particle_output(self) -> None:
        core = comparison.command_document(particle_output=False)
        product = comparison.command_document(particle_output=True)
        self.assertIn("IPOUT=0", core)
        self.assertIn("IPOUT=1", product)
        self.assertEqual(core.replace("IPOUT=0", "IPOUT=1"), product)
        for disabled in (
            "LCONVECTION=0",
            "LTURBULENCE=0",
            "LTURBULENCE_MESO=0",
            "LSUBGRID=0",
            "IOUT=0",
        ):
            self.assertIn(disabled, core)

    def test_symmetric_600_second_contract_is_valid_for_flexpart(self) -> None:
        command = comparison.command_document(
            particle_output=True,
            time_step_seconds=600,
            output_interval_seconds=1_200,
        )
        self.assertIn("LOUTSTEP=1200", command)
        self.assertIn("LOUTAVER=1200", command)
        self.assertIn("LOUTSAMPLE=600", command)
        self.assertIn("LSYNCTIME=600", command)
        with self.assertRaises(comparison.ComparisonError):
            comparison.command_document(
                particle_output=True,
                time_step_seconds=600,
                output_interval_seconds=600,
            )

    def test_release_contract_is_frozen(self) -> None:
        release = comparison.releases_document(50_000)
        for value in (
            "LON1=5.00",
            "LON2=5.50",
            "LAT1=49.00",
            "LAT2=49.50",
            "Z1=500.0",
            "Z2=1000.0",
            "ZKIND=2",
            "MASS=1.0",
            "PARTS=50000",
        ):
            self.assertIn(value, release)

    def test_particle_product_includes_topography_for_asl_alignment(self) -> None:
        options = comparison.partoptions_document()
        self.assertIn(" HEIGHT=.true.,", options)
        self.assertIn(" TOPOGRAPHY=.true.,", options)

    def test_trajecta_case_matches_the_frozen_box_and_schedule(self) -> None:
        case = comparison.trajecta_case_document(10_000, 600, 1_200)
        event = case["particle_population"]["events"][0]
        self.assertEqual(event["particle_count"], 10_000)
        self.assertEqual(
            event["geometry"]["geometry"]["coordinates"],
            [[[5.0, 49.0], [5.5, 49.0], [5.5, 49.5], [5.0, 49.5], [5.0, 49.0]]],
        )
        self.assertEqual(event["vertical"]["lower"], {"value": 500, "unit": "m"})
        self.assertEqual(event["vertical"]["upper"], {"value": 1_000, "unit": "m"})
        self.assertEqual(
            case["outputs"][0]["schedule"],
            {"mode": "interval", "interval": {"value": 1_200, "unit": "s"}},
        )

    def test_path_normalized_provenance_digest_ignores_only_data_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            digests = []
            for index in (1, 2):
                data = root / f"repeat-{index}" / "project" / "data"
                data.mkdir(parents=True)
                record_sha = str(index) * 64
                field_set_sha = str(index + 2) * 64
                bundle = {
                    "records": [
                        {
                            "sha256": record_sha,
                            "record": {
                                "field": "eastward_wind",
                                "quality": "source",
                                "sources": [
                                    f"{data.as_posix()}/met.nc#variable=u",
                                    "a" * 64,
                                ],
                                "transforms": [],
                                "fallback_reason": None,
                                "profile_sha256": "b" * 64,
                            },
                        }
                    ],
                    "field_sets": [
                        {"sha256": field_set_sha, "fields": {"eastward_wind": record_sha}}
                    ],
                    "samples": [
                        {
                            "particle_id": 1,
                            "sample_sequence": 0,
                            "field_set_sha256": field_set_sha,
                        }
                    ],
                }
                path = root / f"bundle-{index}.json"
                path.write_text(json.dumps(bundle), encoding="utf-8")
                digests.append(
                    formal.path_normalized_provenance_digests(path, data, "c" * 64)
                )
            self.assertEqual(digests[0], digests[1])

    def test_available_inventory_is_sorted_and_complete(self) -> None:
        available = comparison.available_document().splitlines()[3:]
        self.assertEqual(len(available), 4)
        self.assertEqual(
            [line.split()[2] for line in available], list(comparison.METEOROLOGY_NAMES)
        )

    def test_official_anchor_times_map_to_flexpart_names(self) -> None:
        self.assertEqual(comparison.meteorology_name(1_543_622_400), "EA18120100")
        self.assertEqual(comparison.meteorology_name(1_543_633_200), "EA18120103")
        self.assertEqual(comparison.meteorology_name(1_543_644_000), "EA18120106")
        self.assertEqual(comparison.meteorology_name(1_543_654_800), "EA18120109")
        with self.assertRaises(comparison.ComparisonError):
            comparison.meteorology_name(1_543_622_401)

    def test_pathnames_are_absolute_and_directories_have_trailing_slashes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            lines = comparison.pathnames_document(
                root / "options", root / "output", root / "met", root / "AVAILABLE"
            ).splitlines()
        self.assertEqual(len(lines), 4)
        self.assertTrue(all(Path(value.rstrip("/")).is_absolute() for value in lines))
        self.assertTrue(all(value.endswith("/") for value in lines[:3]))
        self.assertFalse(lines[3].endswith("/"))

    def test_time_verbose_parser_accepts_minute_and_hour_elapsed_forms(self) -> None:
        template = """
 User time (seconds): 1.25
 System time (seconds): 0.50
 Elapsed (wall clock) time (h:mm:ss or m:ss): {elapsed}
 Maximum resident set size (kbytes): 1234
 File system inputs: 8
 File system outputs: 16
 Exit status: 0
"""
        minute = comparison.parse_time_verbose(template.format(elapsed="2:03.50"))
        hour = comparison.parse_time_verbose(template.format(elapsed="1:02:03"))
        self.assertEqual(minute["elapsed_seconds"], 123.5)
        self.assertEqual(hour["elapsed_seconds"], 3723.0)
        self.assertEqual(minute["peak_rss_kib"], 1234)

    def test_ensemble_metrics_use_spherical_centroid_and_fixed_audit_grid(self) -> None:
        metrics = comparison._ensemble_metrics(
            numpy,
            [5.0, 5.0],
            [49.0, 49.0],
            [500.0, 750.0],
        )
        self.assertAlmostEqual(metrics["centroid_longitude_degrees"], 5.0)
        self.assertAlmostEqual(metrics["centroid_latitude_degrees"], 49.0)
        self.assertEqual(metrics["centroid_height_asl_m"], 625.0)
        self.assertEqual(metrics["occupied_audit_cells"], 2)
        self.assertEqual(metrics["nonfinite_count"], 0)

    def test_formal_matrix_uses_each_rotated_order_once(self) -> None:
        records = []
        reports = []
        for particles in formal.COUNTS:
            records.extend(
                {
                    "particles": particles,
                    "phase": "warmup",
                    "repetition": 0,
                    "order_index": index,
                    "mode": mode,
                }
                for index, mode in enumerate(formal.WARMUP_ORDER)
            )
            for repetition, order in enumerate(formal.FORMAL_ORDERS, start=1):
                records.extend(
                    {
                        "particles": particles,
                        "phase": "formal",
                        "repetition": repetition,
                        "order_index": index,
                        "mode": mode,
                    }
                    for index, mode in enumerate(order)
                )
                reports.append({"particles": particles, "repetition": repetition})

        formal.validate_matrix_records(records, reports)
        records[-1]["mode"] = "trajecta-product"
        with self.assertRaises(formal.FormalError):
            formal.validate_matrix_records(records, reports)

    def test_formal_core_gate_is_evaluated_per_particle_count(self) -> None:
        records = []
        for repetition, (flexpart_seconds, trajecta_seconds) in enumerate(
            ((1.0, 1.2), (1.1, 1.3), (1.2, 1.4)), start=1
        ):
            records.extend(
                (
                    {
                        "particles": 10_000,
                        "phase": "formal",
                        "repetition": repetition,
                        "mode": "flexpart-core",
                        "time_verbose": {"elapsed_seconds": flexpart_seconds},
                    },
                    {
                        "particles": 10_000,
                        "phase": "formal",
                        "repetition": repetition,
                        "mode": "trajecta-product",
                        "performance": {
                            "equivalent_core_nanoseconds": int(
                                trajecta_seconds * 1_000_000_000
                            ),
                            "runner_total_nanoseconds": int(
                                trajecta_seconds * 1_000_000_000
                            ),
                        },
                    },
                )
            )
        gate = formal.count_core_gate(records, 10_000)
        self.assertTrue(gate["passed"])
        records[-1]["performance"]["equivalent_core_nanoseconds"] = 3_000_000_000
        records[-3]["performance"]["equivalent_core_nanoseconds"] = 3_000_000_000
        self.assertFalse(formal.count_core_gate(records, 10_000)["passed"])


if __name__ == "__main__":
    unittest.main()
