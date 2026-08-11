#!/usr/bin/env python3
"""Prepare the two USGS GMTED2010 30 arc-second fields for Trajecta.

Rasterio is used only by this offline preparation tool. The generated grids
are read by Trajecta's pure-Rust runtime without GDAL.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import sys
import tempfile
import zipfile
import zlib
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import BinaryIO

import numpy as np

try:
    import rasterio
    from rasterio.windows import Window
except ImportError as error:  # pragma: no cover - exercised by clean-package tests
    raise SystemExit(
        "Rasterio is required to prepare GMTED2010; install rasterio>=1.4 first"
    ) from error


MAGIC = b"TRAJECTA-TGRID1\0"
FORMAT_VERSION = 1
HEADER_BYTES = 128
INDEX_ENTRY_BYTES = 16
COMPRESSION_ZLIB = 1
TILE_WIDTH = 256
TILE_HEIGHT = 256
DATASET_ID = "gmted2010-30arcsec-mean-std/v1"
GRID_FORMAT_ID = "trajecta.gmted2010-grid/v1"
PRODUCT_URL = "https://pubs.usgs.gov/of/2011/1073/"
CATALOG_URL = (
    "https://dmsdata.cr.usgs.gov/geoserver/wfs?service=WFS&version=2.0.0&"
    "request=GetFeature&typeNames=gmted2010%3AViewer_Layers_Extent&"
    "outputFormat=application%2Fjson"
)


@dataclass(frozen=True)
class FieldSpec:
    role: str
    field_id: int
    archive_member: str
    output_name: str


MEAN = FieldSpec(
    role="mean_elevation",
    field_id=1,
    archive_member="mn30_grd",
    output_name="gmted2010-30arcsec-mean.tgrid",
)
STANDARD_DEVIATION = FieldSpec(
    role="standard_deviation",
    field_id=2,
    archive_member="sd30_grd",
    output_name="gmted2010-30arcsec-standard-deviation.tgrid",
)


class PreparationError(RuntimeError):
    """Input or generated-data contract failure."""


def sha256_file(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            size += len(chunk)
            digest.update(chunk)
    return size, digest.hexdigest()


def archive_identity(path: Path, role: str) -> dict[str, object]:
    size, sha256 = sha256_file(path)
    with zipfile.ZipFile(path) as archive:
        members = [
            {
                "path": item.filename.replace("\\", "/"),
                "size_bytes": item.file_size,
                "compressed_size_bytes": item.compress_size,
                "crc32": f"{item.CRC:08x}",
            }
            for item in sorted(archive.infolist(), key=lambda item: item.filename)
            if not item.is_dir()
        ]
    member_bytes = json.dumps(
        members, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    return {
        "role": role,
        "file_name": path.name,
        "size_bytes": size,
        "sha256": sha256,
        "member_manifest_sha256": hashlib.sha256(member_bytes).hexdigest(),
        "members": members,
    }


def extract_source_archive(archive: Path, destination: Path, field: FieldSpec) -> Path:
    root = destination.resolve()
    with zipfile.ZipFile(archive) as source:
        for item in source.infolist():
            portable = PurePosixPath(item.filename.replace("\\", "/"))
            if (
                portable.is_absolute()
                or not portable.parts
                or any(part in {"", ".", ".."} or ":" in part for part in portable.parts)
                or (item.external_attr >> 16) & 0o170000 == 0o120000
            ):
                raise PreparationError(
                    f"{field.role} archive contains an unsafe member: {item.filename}"
                )
            target = root.joinpath(*portable.parts)
            if item.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(item) as input_stream, target.open("xb") as output_stream:
                shutil.copyfileobj(input_stream, output_stream, length=1024 * 1024)
    dataset_path = root / field.archive_member
    if not dataset_path.is_dir():
        raise PreparationError(
            f"{field.role} archive lacks dataset member {field.archive_member}"
        )
    return dataset_path


def validate_source(dataset: rasterio.DatasetReader, field: FieldSpec) -> None:
    transform = dataset.transform
    if (
        dataset.driver != "AIG"
        or dataset.count != 1
        or dataset.dtypes != ("int16",)
        or dataset.nodata != -32768.0
        or dataset.crs is None
        or dataset.crs.to_epsg() != 4326
        or transform.b != 0.0
        or transform.d != 0.0
        or transform.a <= 0.0
        or transform.e >= 0.0
        or abs(transform.a * dataset.width - 360.0) > 5.0e-6
    ):
        raise PreparationError(f"{field.role} source raster identity is invalid")


def build_header(
    dataset: rasterio.DatasetReader,
    field: FieldSpec,
    tile_columns: int,
    tile_rows: int,
) -> bytearray:
    tile_count = tile_columns * tile_rows
    data_offset = HEADER_BYTES + tile_count * INDEX_ENTRY_BYTES
    header = bytearray(HEADER_BYTES)
    header[0:16] = MAGIC
    struct.pack_into("<I", header, 16, FORMAT_VERSION)
    struct.pack_into("<I", header, 20, HEADER_BYTES)
    struct.pack_into("<I", header, 24, dataset.width)
    struct.pack_into("<I", header, 28, dataset.height)
    struct.pack_into("<I", header, 32, TILE_WIDTH)
    struct.pack_into("<I", header, 36, TILE_HEIGHT)
    struct.pack_into("<h", header, 40, int(dataset.nodata))
    struct.pack_into("<B", header, 42, field.field_id)
    struct.pack_into("<B", header, 43, COMPRESSION_ZLIB)
    struct.pack_into("<d", header, 48, dataset.transform.c)
    struct.pack_into("<d", header, 56, dataset.transform.f)
    struct.pack_into("<d", header, 64, dataset.transform.a)
    struct.pack_into("<d", header, 72, dataset.transform.e)
    struct.pack_into("<I", header, 80, tile_columns)
    struct.pack_into("<I", header, 84, tile_rows)
    struct.pack_into("<Q", header, 88, tile_count)
    struct.pack_into("<Q", header, 96, HEADER_BYTES)
    struct.pack_into("<Q", header, 104, data_offset)
    return header


def copy_payload(source: BinaryIO, destination: BinaryIO) -> None:
    source.seek(0)
    shutil.copyfileobj(source, destination, length=1024 * 1024)


def prepare_grid(
    archive: Path,
    output_dir: Path,
    field: FieldSpec,
    replace: bool,
) -> dict[str, object]:
    output = output_dir / field.output_name
    if output.exists() and not replace:
        raise PreparationError(f"destination exists: {output}")
    payload_name: str | None = None
    final_name: str | None = None
    try:
        with tempfile.TemporaryDirectory(
            prefix=f".{field.archive_member}.", dir=output_dir
        ) as source_directory:
            source_path = extract_source_archive(
                archive, Path(source_directory), field
            )
            with rasterio.open(source_path) as dataset:
                validate_source(dataset, field)
                tile_columns = (dataset.width + TILE_WIDTH - 1) // TILE_WIDTH
                tile_rows = (dataset.height + TILE_HEIGHT - 1) // TILE_HEIGHT
                header = build_header(dataset, field, tile_columns, tile_rows)
                index: list[tuple[int, int, int]] = []
                with tempfile.NamedTemporaryFile(
                    mode="w+b",
                    prefix=f".{field.output_name}.",
                    suffix=".payload",
                    dir=output_dir,
                    delete=False,
                ) as payload:
                    payload_name = payload.name
                    payload_offset = 0
                    raw_bytes = TILE_WIDTH * TILE_HEIGHT * 2
                    data_offset = (
                        HEADER_BYTES + tile_columns * tile_rows * INDEX_ENTRY_BYTES
                    )
                    for tile_row in range(tile_rows):
                        for tile_column in range(tile_columns):
                            column_offset = tile_column * TILE_WIDTH
                            row_offset = tile_row * TILE_HEIGHT
                            window_width = min(TILE_WIDTH, dataset.width - column_offset)
                            window_height = min(TILE_HEIGHT, dataset.height - row_offset)
                            window = Window(
                                column_offset,
                                row_offset,
                                window_width,
                                window_height,
                            )
                            source_values = dataset.read(
                                1,
                                window=window,
                                boundless=False,
                                out_dtype="int16",
                            )
                            if source_values.shape != (window_height, window_width):
                                raise PreparationError(
                                    "Rasterio returned an invalid tile shape"
                                )
                            values = np.full(
                                (TILE_HEIGHT, TILE_WIDTH),
                                int(dataset.nodata),
                                dtype="<i2",
                            )
                            values[:window_height, :window_width] = source_values
                            raw = np.asarray(
                                values, dtype="<i2", order="C"
                            ).tobytes(order="C")
                            if len(raw) != raw_bytes:
                                raise PreparationError(
                                    "prepared tile has an invalid byte count"
                                )
                            compressed = zlib.compress(raw, level=6)
                            payload.write(compressed)
                            index.append(
                                (data_offset + payload_offset, len(compressed), raw_bytes)
                            )
                            payload_offset += len(compressed)
                    payload.flush()
                    os.fsync(payload.fileno())

                with tempfile.NamedTemporaryFile(
                    mode="w+b",
                    prefix=f".{field.output_name}.",
                    suffix=".tmp",
                    dir=output_dir,
                    delete=False,
                ) as final:
                    final_name = final.name
                    final.write(header)
                    for offset, compressed_bytes, uncompressed_bytes in index:
                        final.write(
                            struct.pack(
                                "<QII", offset, compressed_bytes, uncompressed_bytes
                            )
                        )
                    with open(payload_name, "rb") as payload:
                        copy_payload(payload, final)
                    final.flush()
                    os.fsync(final.fileno())
                os.replace(final_name, output)
                final_name = None
                size, sha256 = sha256_file(output)
                return {
                    "role": field.role,
                    "relative_path": field.output_name,
                    "size_bytes": size,
                    "sha256": sha256,
                    "grid": {
                        "width": dataset.width,
                        "height": dataset.height,
                        "tile_width": TILE_WIDTH,
                        "tile_height": TILE_HEIGHT,
                        "longitude_origin_degrees": dataset.transform.c,
                        "latitude_origin_degrees": dataset.transform.f,
                        "longitude_spacing_degrees": dataset.transform.a,
                        "latitude_spacing_degrees": dataset.transform.e,
                        "nodata": int(dataset.nodata),
                    },
                }
    finally:
        for temporary in (payload_name, final_name):
            if temporary:
                Path(temporary).unlink(missing_ok=True)


def write_json_atomic(path: Path, document: dict[str, object], replace: bool) -> None:
    if path.exists() and not replace:
        raise PreparationError(f"destination exists: {path}")
    content = (json.dumps(document, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode(
        "utf-8"
    )
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary_name, path)
    finally:
        Path(temporary_name).unlink(missing_ok=True)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mean-archive", type=Path, required=True)
    parser.add_argument("--standard-deviation-archive", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--replace", action="store_true")
    parser.add_argument(
        "--transfer-source",
        help="Optional archive or mirror URL used for this local transfer",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    arguments = parse_args(sys.argv[1:] if argv is None else argv)
    mean_archive = arguments.mean_archive.resolve()
    standard_deviation_archive = arguments.standard_deviation_archive.resolve()
    for archive in (mean_archive, standard_deviation_archive):
        if not archive.is_file():
            raise PreparationError(f"source archive is missing: {archive}")
    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    sources = [
        archive_identity(mean_archive, "mean_source_archive"),
        archive_identity(standard_deviation_archive, "standard_deviation_source_archive"),
    ]
    prepared = [
        prepare_grid(mean_archive, output_dir, MEAN, arguments.replace),
        prepare_grid(
            standard_deviation_archive,
            output_dir,
            STANDARD_DEVIATION,
            arguments.replace,
        ),
    ]
    manifest = {
        "schema_version": "trajecta.gmted2010-source/v1",
        "dataset_id": DATASET_ID,
        "prepared_format": GRID_FORMAT_ID,
        "source": {
            "provider": "U.S. Geological Survey",
            "product": "Global Multi-resolution Terrain Elevation Data 2010",
            "product_url": PRODUCT_URL,
            "catalog_url": CATALOG_URL,
            "transfer_source": arguments.transfer_source,
            "attribution": (
                "U.S. Geological Survey Global Multi-resolution Terrain Elevation Data 2010 "
                "(GMTED2010), DOI 10.3133/ofr20111073"
            ),
        },
        "sources": sources,
        "prepared_files": prepared,
    }
    manifest_path = output_dir / "gmted2010-source-manifest.json"
    write_json_atomic(manifest_path, manifest, arguments.replace)
    manifest_size, manifest_sha256 = sha256_file(manifest_path)
    print(
        json.dumps(
            {
                "manifest": str(manifest_path),
                "manifest_size_bytes": manifest_size,
                "manifest_sha256": manifest_sha256,
                "prepared_files": prepared,
            },
            indent=2,
            ensure_ascii=False,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PreparationError as error:
        raise SystemExit(f"GMTED2010 preparation failed: {error}") from error
