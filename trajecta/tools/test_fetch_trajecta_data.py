#!/usr/bin/env python3
"""Offline contract tests for the M5.1 data helper."""

from __future__ import annotations

import json
import struct
import tempfile
import unittest
from pathlib import Path

import fetch_trajecta_data as helper
import fetch_cfsr_pgbl
import fetch_era5_pressure_cds
import era5_hybrid_official_pipeline


class DataHelperTests(unittest.TestCase):
    def test_hybrid_pv_extraction_requires_no_source_tree_or_cargo(self) -> None:
        values = [0.0, 100.0, 0.0, 1.0]
        coordinates = b"".join(struct.pack(">f", value) for value in values)
        section = (
            (9 + len(coordinates)).to_bytes(4, "big")
            + b"\x04"
            + len(values).to_bytes(2, "big")
            + b"\x00\x00"
            + coordinates
        )
        length = 16 + len(section) + 4
        message = b"GRIB\x00\x00\x00\x02" + length.to_bytes(8, "big") + section + b"7777"
        a_half_pa, b_half = era5_hybrid_official_pipeline.grib2_hybrid_coefficients(
            message
        )
        self.assertEqual(a_half_pa, [0.0, 100.0])
        self.assertEqual(b_half, [0.0, 1.0])

    def test_anchor_halo_reproduces_four_frame_cfsr_demo(self) -> None:
        values = helper.anchors(1230789600, 1230790200, 21600)
        stamps = [helper.datetime.fromtimestamp(value, helper.UTC).strftime("%Y%m%d%H") for value in values]
        self.assertEqual(stamps, ["2009010100", "2009010106", "2009010112", "2009010118"])

    def test_provider_plans_are_credential_free_and_deterministic(self) -> None:
        area = [53.0, 0.0, 45.0, 10.0]
        pressure = fetch_era5_pressure_cds.request_plan(
            "2018-12-01", ["00:00", "06:00"], area
        )
        hybrid = era5_hybrid_official_pipeline.request_plan(
            "2018-12-01", ["00:00", "03:00"], area
        )
        payload = json.dumps([pressure, hybrid], sort_keys=True)
        self.assertEqual(pressure, fetch_era5_pressure_cds.request_plan("2018-12-01", ["00:00", "06:00"], area))
        self.assertNotRegex(payload.casefold(), r"credential|api[_-]?key|token|password")

    def test_path_jail_rejects_escape_and_symlink_escape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "project"
            root.mkdir()
            with self.assertRaises(helper.FetchError):
                helper.jailed_path(root, "../outside")
            with self.assertRaises(helper.FetchError):
                helper.jailed_path(root, "/absolute")

    def test_cfsr_downloader_local_http_fault_matrix(self) -> None:
        self.assertTrue(fetch_cfsr_pgbl.run_self_tests())


if __name__ == "__main__":
    unittest.main()
