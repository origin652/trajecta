#!/usr/bin/env python3
"""Offline contract tests for the M5.1 data helper."""

from __future__ import annotations

import json
import hashlib
import http.server
import struct
import tempfile
import threading
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

    def test_gmted_plan_downloads_only_prepared_runtime_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            requirement = {
                "profile_name": "terrain",
                "case_name": "case",
                "dataset_id": "gmted2010-30arcsec-mean-std/v1",
                "dataset_profile": helper.GMTED2010_PROFILE,
                "data_roots": {"auxiliary": "shared/gmted2010"},
            }
            planned = helper.requirement_plan(root, requirement, {})
        self.assertEqual(planned["family"], "gmted2010")
        self.assertEqual(planned["anchors_utc"], [])
        self.assertEqual(len(planned["requests"]), 3)
        targets = [request["target"] for request in planned["requests"]]
        self.assertTrue(all(target.startswith("shared/gmted2010/") for target in targets))
        self.assertFalse(any(target.endswith(("mn30_grd.zip", "sd30_grd.zip")) for target in targets))
        self.assertTrue(all(request["expected_sha256"] for request in planned["requests"]))

    def test_fixed_file_fetch_reuses_only_matching_existing_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "data" / "prepared.bin"
            target.parent.mkdir()
            payload = b"prepared terrain"
            target.write_bytes(payload)
            request = {
                "urls": ["https://invalid.example/prepared.bin"],
                "target": "data/prepared.bin",
                "expected_size": len(payload),
                "expected_sha256": hashlib.sha256(payload).hexdigest(),
            }
            files = helper.fetch_fixed_files(root, {"requests": [request]})
            self.assertEqual(files[0]["path"], "data/prepared.bin")
            request["expected_sha256"] = "0" * 64
            with self.assertRaises(helper.FetchError):
                helper.fetch_fixed_files(root, {"requests": [request]})

    def test_fixed_file_fetch_resumes_a_local_http_partial_and_then_reuses_it(self) -> None:
        payload = b"GMTED2010-PREPARED-" + bytes(range(256)) * 4_096
        digest = hashlib.sha256(payload).hexdigest()
        range_starts: list[int] = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, _format: str, *_args: object) -> None:
                return

            def do_GET(self) -> None:  # noqa: N802
                header = self.headers.get("Range")
                start = 0
                if header is not None:
                    start = int(header.removeprefix("bytes=").removesuffix("-"))
                    range_starts.append(start)
                    self.send_response(206)
                    self.send_header(
                        "Content-Range", f"bytes {start}-{len(payload) - 1}/{len(payload)}"
                    )
                else:
                    self.send_response(200)
                body = payload[start:]
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                target = root / "shared" / "gmted2010" / "mean.tgrid"
                target.parent.mkdir(parents=True)
                partial = target.with_suffix(target.suffix + ".part")
                prefix = len(payload) // 3
                partial.write_bytes(payload[:prefix])
                request = {
                    "urls": [f"http://127.0.0.1:{server.server_address[1]}/mean.tgrid"],
                    "target": "shared/gmted2010/mean.tgrid",
                    "expected_size": len(payload),
                    "expected_sha256": digest,
                }
                item = {"requests": [request]}
                first = helper.fetch_fixed_files(root, item)
                self.assertEqual(target.read_bytes(), payload)
                self.assertFalse(partial.exists())
                self.assertEqual(range_starts, [prefix])
                requests_after_first = len(range_starts)
                second = helper.fetch_fixed_files(root, item)
                self.assertEqual(second, first)
                self.assertEqual(len(range_starts), requests_after_first)
        finally:
            server.shutdown()
            server.server_close()

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
