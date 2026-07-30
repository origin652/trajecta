#!/usr/bin/env python3
"""Focused standard-library tests for the M5-A4 product matrix runner."""

from __future__ import annotations

from contextlib import closing
import hashlib
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import run_m5_a4_product_matrix as product


class ProductMatrixTests(unittest.TestCase):
    def test_linux_environment_keeps_unix_sockets_on_native_tmp(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "attempt"
            package_root = Path(temporary) / "package"
            package_root.mkdir()
            identity = product.ProductIdentity(
                package_root=package_root,
                binary=package_root / "trajecta",
                platform="linux-x86_64",
                source_tree_sha256="1" * 64,
                build_manifest_sha256="2" * 64,
                binary_sha256="3" * 64,
            )
            with mock.patch.object(product.sys, "platform", "linux"):
                environment = product.clean_environment(identity, root)
            self.assertEqual(environment["TMPDIR"], "/tmp")
            self.assertFalse((root / "tmp").exists())

    @unittest.skipUnless(sys.platform == "win32", "Windows extended paths are platform-specific")
    def test_sqlite_audit_accepts_windows_extended_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            database = Path(temporary) / "particles.sqlite"
            with closing(sqlite3.connect(database)) as connection:
                connection.executescript(
                    """
                    CREATE TABLE particle (particle_id INTEGER PRIMARY KEY, value REAL);
                    CREATE TABLE particle_state (particle_id INTEGER, value REAL);
                    CREATE TABLE particle_mass (particle_id INTEGER, value REAL);
                    CREATE TABLE termination (particle_id INTEGER, value REAL);
                    INSERT INTO particle VALUES (7, 1.0);
                    INSERT INTO particle_state VALUES (7, 2.0);
                    INSERT INTO particle_mass VALUES (7, 3.0);
                    """
                )
                connection.commit()

            extended = Path("\\\\?\\" + str(database.resolve()))
            self.assertEqual(
                product.sqlite_audit(
                    extended,
                    {
                        "particle": 1,
                        "particle_state": 1,
                        "particle_mass": 1,
                        "termination": 0,
                    },
                ),
                (True, 7),
            )

    def test_formal_matrix_has_the_frozen_thirty_cells_in_order(self) -> None:
        cells = product.formal_cells("windows-x86_64")
        self.assertEqual(len(cells), 30)
        self.assertEqual(len({cell.id for cell in cells}), 30)
        self.assertEqual([cell.phase for cell in cells[:18]], ["rust-1k"] * 18)
        self.assertEqual([cell.phase for cell in cells[18:24]], ["rust-10k"] * 6)
        self.assertEqual([cell.phase for cell in cells[24:]], ["native-1k"] * 6)
        self.assertEqual(
            [(cell.family, cell.population, cell.direction) for cell in cells[18:24]],
            [
                ("era5-pressure", "release", "forward"),
                ("era5-pressure", "air-mass", "backward"),
                ("era5-hybrid", "air-mass", "forward"),
                ("era5-hybrid", "ozone", "backward"),
                ("cfsr-pressure", "ozone", "forward"),
                ("cfsr-pressure", "release", "backward"),
            ],
        )

    def test_case_documents_keep_direction_population_and_boundary_semantics(self) -> None:
        for population, strategy in (
            ("release", "release_driven"),
            ("air-mass", "domain_fill_air_mass"),
            ("ozone", "domain_fill_stratospheric_ozone"),
        ):
            forward = product.Cell(
                "linux-x86_64",
                "rust-1k",
                "era5-pressure",
                population,
                "forward",
                "rust",
                1_000,
                1,
            )
            backward = product.Cell(
                "linux-x86_64",
                "rust-1k",
                "cfsr-pressure",
                population,
                "backward",
                "rust",
                1_000,
                1,
            )
            forward_doc = product.case_document(forward)
            backward_doc = product.case_document(backward)
            self.assertEqual(forward_doc["particle_population"]["strategy"], strategy)
            self.assertEqual(backward_doc["particle_population"]["strategy"], strategy)
            self.assertGreater(
                forward_doc["time"]["end"]["seconds_since_unix_epoch"],
                forward_doc["time"]["start"]["seconds_since_unix_epoch"],
            )
            self.assertLess(
                backward_doc["time"]["end"]["seconds_since_unix_epoch"],
                backward_doc["time"]["start"]["seconds_since_unix_epoch"],
            )
            self.assertNotEqual(
                forward_doc["time"]["start"]["seconds_since_unix_epoch"],
                backward_doc["time"]["start"]["seconds_since_unix_epoch"],
            )
            self.assertIn(
                "limited_domain_terminate/v0",
                forward_doc["numerics"]["boundaries"]["policies"],
            )
            self.assertIn(
                "global_periodic/v0",
                backward_doc["numerics"]["boundaries"]["policies"],
            )
            if population == "ozone":
                self.assertEqual(
                    forward_doc["substances"],
                    [{"id": "ozone", "display_name": "Ozone"}],
                )
                self.assertEqual(
                    forward_doc["particle_population"]["ozone_substance"],
                    forward_doc["substances"][0]["id"],
                )
                self.assertEqual(forward_doc["substances"], backward_doc["substances"])

    def test_fixture_validation_is_exact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = b"frozen meteorology"
            digest = hashlib.sha256(payload).hexdigest()
            (root / "fixture.bin").write_bytes(payload)
            family = product.Family(
                "fixture",
                "profile",
                0,
                1,
                False,
                (0.0, 0.0),
                (("fixture.bin", digest),),
            )
            self.assertEqual(product.validate_fixture(family, root), {"fixture.bin": digest})
            (root / "fixture.bin").write_bytes(b"changed")
            with self.assertRaises(product.ProductError):
                product.validate_fixture(family, root)

    def test_quality_gate_accepts_derived_surface_values_and_normal_terminal_missing(self) -> None:
        quality = {
            "wind": [
                {"validity": "missing", "quality": "derived", "count": 19},
                {"validity": "ok", "quality": "derived", "count": 1_981},
            ],
            "pressure": [
                {"validity": "missing", "quality": "derived", "count": 19},
                {"validity": "ok", "quality": "derived", "count": 1_981},
            ],
            "temperature": [
                {"validity": "missing", "quality": "source", "count": 19},
                {"validity": "ok", "quality": "derived", "count": 49},
                {"validity": "ok", "quality": "source", "count": 1_932},
            ],
        }
        particles = {"state_count": 2_000, "normal_termination_count": 19}
        self.assertTrue(product.quality_is_valid(quality, particles))

        for field, validity, provenance_quality in (
            ("temperature", "ok", "estimated"),
            ("wind", "partial", "derived"),
            ("pressure", "ok", "unknown"),
        ):
            invalid = {name: [dict(row) for row in rows] for name, rows in quality.items()}
            invalid[field][1]["validity"] = validity
            invalid[field][1]["quality"] = provenance_quality
            with self.subTest(field=field, validity=validity, quality=provenance_quality):
                self.assertFalse(product.quality_is_valid(invalid, particles))

        self.assertFalse(
            product.quality_is_valid(
                quality,
                {"state_count": 2_000, "normal_termination_count": 18},
            )
        )

    def test_manifest_freezes_lock_while_provenance_follows_directional_window(self) -> None:
        forward = product.Cell(
            "windows-x86_64",
            "rust-1k",
            "cfsr-pressure",
            "release",
            "forward",
            "rust",
            1_000,
            1,
        )
        backward = product.Cell(
            "windows-x86_64",
            "rust-1k",
            "cfsr-pressure",
            "release",
            "backward",
            "rust",
            1_000,
            1,
        )
        self.assertEqual(
            product.expected_provenance_inputs(forward),
            {
                "pgbl00.gdas.2009010100.grb2",
                "pgbl00.gdas.2009010106.grb2",
                "pgbl00.gdas.2009010112.grb2",
            },
        )
        self.assertEqual(
            product.expected_provenance_inputs(backward),
            {
                "pgbl00.gdas.2009010106.grb2",
                "pgbl00.gdas.2009010112.grb2",
                "pgbl00.gdas.2009010118.grb2",
            },
        )
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary) / "project"
            locks = project / "locks"
            locks.mkdir(parents=True)
            lock_path = locks / "cfsr-pressure-rust.lock.json"
            frozen = dict(product.FAMILIES["cfsr-pressure"].files)
            lock_path.write_text(
                json.dumps(
                    {
                        "profile": {"sha256": "d" * 64},
                        "files": [
                            {"relative_path": name, "sha256": frozen[name]}
                            for name in (
                                "pgbl00.gdas.2009010100.grb2",
                                "pgbl00.gdas.2009010106.grb2",
                                "pgbl00.gdas.2009010112.grb2",
                            )
                        ],
                    }
                ),
                encoding="utf-8",
            )
            identity = product.expected_manifest_identity(project, forward)
            self.assertEqual(
                list(identity["dataset_content_sha256"]),
                [
                    "cfsr-pressure:pgbl00.gdas.2009010100.grb2",
                    "cfsr-pressure:pgbl00.gdas.2009010106.grb2",
                    "cfsr-pressure:pgbl00.gdas.2009010112.grb2",
                ],
            )
            self.assertEqual(
                identity["dataset_profile_sha256"], {"cfsr-pressure": "d" * 64}
            )
            bundle = Path(temporary) / "provenance-bundle.json"
            prefix = b"pgbl00.gdas.2009010100.grb2\n"
            split_name = b"pgbl00.gdas.2009010112.grb2"
            split_at = 8
            bundle.write_bytes(
                prefix
                + b"x" * (1024 * 1024 - len(prefix) - split_at)
                + split_name[:split_at]
                + split_name[split_at:]
                + b"\npgbl00.gdas.2009010106.grb2"
            )
            self.assertEqual(
                product.observed_provenance_inputs(bundle, forward),
                product.expected_provenance_inputs(forward),
            )

    def test_product_cell_validation_requires_a_real_passed_run(self) -> None:
        cell = product.Cell(
            "windows-x86_64",
            "rust-1k",
            "cfsr-pressure",
            "release",
            "forward",
            "rust",
            1_000,
            1,
        )
        value = {
            "schema_version": "trajecta.m5-a4-product-cell/v1",
            "cell": cell.as_json(),
            "source_tree_sha256": "1" * 64,
            "build_manifest_sha256": "2" * 64,
            "binary_sha256": "3" * 64,
            "result": "passed",
            "checks": {"complete": True},
            "failures": [],
            "run": {
                "job_series_id": "018f0000-0000-7000-8000-000000000000",
                "run_id": "018f0000-0000-7000-8000-000000000001",
                "attempt": 1,
                "run_directory": "runs/example",
                "runner_milliseconds": 1,
                "digests": {
                    "content_sha256": "4" * 64,
                    "sqlite_sql_sha256": "5" * 64,
                    "canonical_output_sha256": "6" * 64,
                },
            },
        }
        product.validate_product_cell(value)
        value["checks"]["complete"] = False
        with self.assertRaises(product.ProductError):
            product.validate_product_cell(value)


if __name__ == "__main__":
    unittest.main()
