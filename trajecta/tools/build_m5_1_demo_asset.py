#!/usr/bin/env python3
"""Build the deterministic, separately published M5.1 demonstration dataset."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import zipfile


ROOT = Path(__file__).resolve().parents[1]
METADATA = ROOT / "packaging" / "demo-data"
ARCHIVE_ROOT = "trajecta-demo-cfsr-20090101-v1"
ZIP_TIME = (1980, 1, 1, 0, 0, 0)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def add_bytes(archive: zipfile.ZipFile, relative: str, content: bytes) -> None:
    info = zipfile.ZipInfo(f"{ARCHIVE_ROOT}/{relative}", ZIP_TIME)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = 0o100644 << 16
    archive.writestr(info, content, compresslevel=9)


def build(source: Path, output: Path) -> dict[str, object]:
    manifest_path = METADATA / "MANIFEST.json"
    manifest_bytes = manifest_path.read_bytes()
    manifest = json.loads(manifest_bytes)
    files = manifest["files"]
    if not isinstance(files, list) or not files:
        raise ValueError("demo manifest must contain files")

    inputs: list[tuple[str, Path]] = []
    for record in files:
        relative = Path(record["path"])
        path = source / relative.name
        if not path.is_file():
            raise FileNotFoundError(path)
        if path.stat().st_size != record["size_bytes"]:
            raise ValueError(f"size mismatch: {path}")
        if sha256(path) != record["sha256"]:
            raise ValueError(f"SHA-256 mismatch: {path}")
        inputs.append((relative.as_posix(), path))

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(output.suffix + ".tmp")
    if temporary.exists():
        temporary.unlink()
    with zipfile.ZipFile(temporary, "w", allowZip64=True) as archive:
        add_bytes(archive, "README.md", (METADATA / "README.md").read_bytes())
        add_bytes(archive, "MANIFEST.json", manifest_bytes)
        for relative, path in sorted(inputs):
            add_bytes(archive, relative, path.read_bytes())
    temporary.replace(output)
    result = {
        "schema_version": "trajecta.demo-data-build/v1",
        "archive": output.name,
        "size_bytes": output.stat().st_size,
        "sha256": sha256(output),
        "file_count": len(inputs),
    }
    output.with_suffix(output.suffix + ".sha256").write_text(
        f"{result['sha256']}  {output.name}\n", encoding="ascii", newline="\n"
    )
    output.with_suffix(output.suffix + ".json").write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(build(args.source.resolve(), args.output.resolve()), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
