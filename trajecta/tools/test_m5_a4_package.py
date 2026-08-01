#!/usr/bin/env python3
"""Focused standard-library tests for the M5-A4 package boundary."""

from __future__ import annotations

import argparse
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

import m5_a4_package as package


NATIVE_COMPONENTS = [
    {
        "name": "ecCodes",
        "version": "2.47.0",
        "license": "Apache-2.0",
        "source": "https://example.invalid/eccodes",
    },
    {
        "name": "HDF5",
        "version": "1.14.6",
        "license": "LicenseRef-HDF5",
        "source": "https://example.invalid/hdf5",
    },
    {
        "name": "netCDF-C",
        "version": "4.9.3",
        "license": "BSD-3-Clause",
        "source": "https://example.invalid/netcdf",
    },
]


def write(path: Path, value: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value)


def fake_stage(root: Path) -> tuple[Path, dict[str, object]]:
    stage = root / "trajecta-0.1.0-alpha.1-linux-x86_64"
    stage.mkdir()
    write(stage / "trajecta", b"#!/bin/sh\nexit 0\n")
    (stage / "trajecta").chmod(0o755)
    write(stage / "LICENSE", b"MIT\n")
    write(stage / "README.md", b"quick start\n")
    write(stage / "examples/minimal/trajecta-project.yaml", b"name: demo\n")
    write(stage / "lib/libeccodes.so", b"eccodes\n")
    write(stage / "share/eccodes/definitions/boot.def", b"boot\n")
    package.write_json(
        stage / package.SBOM_NAME,
        {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1},
    )
    package.write_json(
        stage / package.LICENSE_INVENTORY_NAME,
        {
            "schema_version": "trajecta.third-party-license-inventory/v1",
            "components": [],
        },
    )
    payload = package.payload_inventory(stage, "trajecta")
    binary = package.file_identity(stage / "trajecta", "trajecta")
    sbom = package.file_identity(stage / package.SBOM_NAME, package.SBOM_NAME)
    sbom["spec_version"] = "CycloneDX 1.5"
    manifest: dict[str, object] = {
        "schema_version": "trajecta.build-manifest/v1",
        "product": "trajecta",
        "version": "0.1.0-alpha.1",
        "platform": {
            "label": "linux-x86_64",
            "target_triple": "x86_64-unknown-linux-gnu",
            "minimum_os": "Ubuntu 24.04",
        },
        "source": {
            "git_head": "0" * 40,
            "source_tree_sha256": "1" * 64,
            "file_count": 1,
            "dirty_paths": [],
        },
        "build": {
            "cargo_profile": "release",
            "cargo": "cargo test",
            "rustc": "rustc test",
            "features": package.FEATURES,
            "default_reader_backend": "rust",
            "native_reader_included": True,
            "source_date_epoch": 1_700_000_000,
        },
        "archive": {
            "file_name": stage.name + ".tar.gz",
            "format": "tar.gz",
            "root_directory": stage.name,
        },
        "binary": binary,
        "sbom": sbom,
        "license_inventory": package.file_identity(
            stage / package.LICENSE_INVENTORY_NAME, package.LICENSE_INVENTORY_NAME
        ),
        "native_components": NATIVE_COMPONENTS,
        "payload": payload,
    }
    package.write_json(stage / package.MANIFEST_NAME, manifest)
    return stage, manifest


class PackageTests(unittest.TestCase):
    def test_porcelain_parser_preserves_first_path_and_expands_records(self) -> None:
        self.assertEqual(
            package.parse_porcelain_paths(
                b" M crates/first.rs\0?? packaging/examples/demo.yaml\0R  new.rs\0old.rs\0"
            ),
            ["crates/first.rs", "new.rs", "old.rs", "packaging/examples/demo.yaml"],
        )
        self.assertEqual(
            package.parse_porcelain_paths(b" M trajecta/crates/first.rs\0", "trajecta"),
            ["crates/first.rs"],
        )

    def test_relative_paths_reject_escape_and_backslash(self) -> None:
        for value in ["../x", "/x", "a\\b", "."]:
            with self.assertRaises(package.PackageError):
                package.normalized_relative(value)
        self.assertEqual(package.normalized_relative("a/b"), "a/b")

    def test_definitions_copy_dereferences_only_internal_file_links(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "definitions"
            source.mkdir()
            write(source / "base.def", b"definition\n")
            internal = source / "alias.def"
            try:
                internal.symlink_to("base.def")
            except OSError as error:
                self.skipTest(f"symbolic links unavailable: {error}")

            copied = root / "copied"
            package.copy_definitions_tree(source, copied)
            self.assertFalse((copied / "alias.def").is_symlink())
            self.assertEqual((copied / "alias.def").read_bytes(), b"definition\n")

            outside = root / "outside.def"
            write(outside, b"outside\n")
            escaping = source / "escaping.def"
            escaping.symlink_to(outside)
            with self.assertRaises(package.PackageError):
                package.copy_definitions_tree(source, root / "rejected")

    def test_tar_is_deterministic_and_verifies(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage, _ = fake_stage(root)
            first = root / (stage.name + ".tar.gz")
            second = root / "second.tar.gz"
            package.create_archive(stage, first, stage.name, "tar.gz", 1_700_000_000)
            package.create_archive(stage, second, stage.name, "tar.gz", 1_700_000_000)
            self.assertEqual(package.sha256(first), package.sha256(second))
            checksum = first.with_name(first.name + ".sha256")
            checksum.write_text(f"{package.sha256(first)}  {first.name}\n", encoding="ascii")
            with mock.patch.object(
                package,
                "probe_native_versions",
                return_value={
                    component["name"]: component["version"]
                    for component in NATIVE_COMPONENTS
                },
            ):
                result = package.verify_package(
                    argparse.Namespace(
                        archive=first,
                        checksum=None,
                        extract_root=root / "extract",
                        result=None,
                    )
                )
            self.assertEqual(result["status"], "passed")

    def test_native_component_version_mismatch_is_rejected(self) -> None:
        actual = {
            component["name"]: component["version"] for component in NATIVE_COMPONENTS
        }
        package.verify_native_component_versions(NATIVE_COMPONENTS, actual)
        actual["HDF5"] = "2.1.0"
        with self.assertRaises(package.PackageError):
            package.verify_native_component_versions(NATIVE_COMPONENTS, actual)

    def test_zip_traversal_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "bad.zip"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("root/../../escape", b"bad")
            with self.assertRaises(package.PackageError):
                package.inspect_zip(archive)

    def test_payload_tamper_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            stage, _ = fake_stage(Path(temporary))
            (stage / "README.md").write_text("changed\n", encoding="utf-8")
            with self.assertRaises(package.PackageError):
                package.validate_build_manifest(stage)


if __name__ == "__main__":
    unittest.main()
