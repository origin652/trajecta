"""Tests for the deterministic M5.1 demonstration archive."""

from __future__ import annotations

import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import zipfile

import build_m5_1_demo_asset as demo


class DemoAssetTests(unittest.TestCase):
    def test_build_is_deterministic_and_rejects_changed_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            records = []
            for index, content in enumerate((b"frame-a", b"frame-b")):
                name = f"frame-{index}.grb2"
                path = source / name
                path.write_bytes(content)
                records.append(
                    {
                        "path": f"data/{name}",
                        "size_bytes": len(content),
                        "sha256": demo.sha256(path),
                    }
                )
            metadata = root / "metadata"
            metadata.mkdir()
            (metadata / "README.md").write_text("source terms\n", encoding="utf-8")
            (metadata / "MANIFEST.json").write_text(
                json.dumps({"files": records}) + "\n", encoding="utf-8"
            )
            with mock.patch.object(demo, "METADATA", metadata):
                first = demo.build(source, root / "first.zip")
                second = demo.build(source, root / "second.zip")
                self.assertEqual(first["sha256"], second["sha256"])
                with zipfile.ZipFile(root / "first.zip") as archive:
                    self.assertEqual(len(archive.namelist()), 4)
                self.assertEqual(
                    (root / "first.zip.sha256").read_text(encoding="ascii"),
                    f"{first['sha256']}  first.zip\n",
                )
                (source / "frame-0.grb2").write_bytes(b"changed")
                with self.assertRaises(ValueError):
                    demo.build(source, root / "bad.zip")


if __name__ == "__main__":
    unittest.main()
