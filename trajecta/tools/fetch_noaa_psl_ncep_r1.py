#!/usr/bin/env python3
"""Download official NOAA PSL NCEP/NCAR Reanalysis 1 multi-file NetCDF anchors.

Reproducible download:
  - write to .part
  - HTTP Range resume only when server returns 206 + valid Content-Range;
    otherwise delete .part and full-download on the next attempt
  - verify size + frozen SHA-256 before atomic replace
  - return (size, sha256) so the main loop does not re-hash multi-GB files
  - create compatible-17level-with-pres via hardlinks only (no absolute symlinks)

Does not rewrite NetCDF attributes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

BASE_PRESSURE = "https://downloads.psl.noaa.gov/Datasets/ncep.reanalysis/pressure"
BASE_SURFACE = "https://downloads.psl.noaa.gov/Datasets/ncep.reanalysis/surface"
USER_AGENT = "trajecta-fetch/0.3"

# Frozen SHA-256 for the official 2009 anchors used by Trajecta tests.
# Update only after re-fetching from PSL and dual-checking size.
FROZEN_SHA256: dict[str, str] = {
    "air.2009.nc": "9ba1f9a4cb75452d690ac63cf6d7cb943cb665c670393be75e5f334b58a50980",
    "uwnd.2009.nc": "3f1dae9b6d19e1f2deafb751a5c8b6509866868a43718709437052551ecf9412",
    "vwnd.2009.nc": "6a5188130207b8112778e5d37ca98b7c194dee562c7d40f7020dae3de33880ef",
    "omega.2009.nc": "8ce8bccbba60e3ab2751c702f0119dd3c3cb179487778be3286daba48a29ace6",
    "shum.2009.nc": "4b0af73070e2f41dac343f0ef31c4654e8d3ed7c9d2ecf495bbd9f37485e8f50",
    "hgt.2009.nc": "236df18af023d82794ccfff0bc3175a7c3ded4cad18a010e284b1e80743352dc",
    "pres.sfc.2009.nc": "b870f345f4743c784642f0e70627c1c38e75d35b115610d991cfd12f4f9b9bf1",
    "hgt.sfc.nc": "ca400fd56fc2c2f45352a7d80aefff7b349e7093ead14d4e788eb0d122835de3",
}

DEFAULT_FILES = [
    ("pressure", "air.2009.nc"),
    ("pressure", "uwnd.2009.nc"),
    ("pressure", "vwnd.2009.nc"),
    ("pressure", "omega.2009.nc"),
    ("pressure", "shum.2009.nc"),
    ("pressure", "hgt.2009.nc"),
    ("surface", "pres.sfc.2009.nc"),
    ("surface", "hgt.sfc.nc"),
]

COMPATIBLE_MEMBERS = [
    "air.2009.nc",
    "uwnd.2009.nc",
    "vwnd.2009.nc",
    "hgt.2009.nc",
    "pres.sfc.2009.nc",
]


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while True:
            chunk = f.read(1 << 20)
            if not chunk:
                break
            h.update(chunk)
    return h.hexdigest()


def url_for(kind: str, name: str) -> str:
    base = BASE_PRESSURE if kind == "pressure" else BASE_SURFACE
    return f"{base}/{name}"


def head_size(url: str) -> int | None:
    req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=60) as resp:
        value = resp.headers.get("Content-Length")
        return int(value) if value else None


def download(
    url: str, dest: Path, expected_sha: str | None, retries: int = 6
) -> tuple[int, str]:
    """Download to dest. Returns (size, sha256) of the final file."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")
    expected_size = head_size(url)

    if dest.is_file():
        size = dest.stat().st_size
        digest = sha256_file(dest)
        size_ok = expected_size is None or size == expected_size
        sha_ok = expected_sha is None or digest == expected_sha
        if size_ok and sha_ok and size > 0:
            print(f"reuse complete {dest.name} ({size} bytes)", flush=True)
            return size, digest
        if not sha_ok:
            print(
                f"hash mismatch for {dest.name}: got {digest} expected {expected_sha}; re-download",
                flush=True,
            )
            dest.unlink(missing_ok=True)
            part.unlink(missing_ok=True)

    for attempt in range(1, retries + 1):
        existing = part.stat().st_size if part.is_file() else 0
        headers: dict[str, str] = {"User-Agent": USER_AGENT}
        mode = "wb"
        use_range = existing > 0 and (expected_size is None or existing < expected_size)
        if use_range:
            headers["Range"] = f"bytes={existing}-"
            mode = "ab"
            print(f"resume {dest.name} from {existing} (attempt {attempt})", flush=True)
        else:
            existing = 0
            mode = "wb"
            if part.is_file():
                part.unlink()
            print(f"download {dest.name} attempt {attempt}", flush=True)
        req = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=600) as resp, part.open(mode) as out:
                status = getattr(resp, "status", None) or resp.getcode()
                if use_range:
                    content_range = resp.headers.get("Content-Range") or ""
                    if status != 206 or not content_range.startswith(f"bytes {existing}-"):
                        # Server ignored Range or returned a bad range: discard partial and full-get next.
                        print(
                            f"range resume rejected for {dest.name} "
                            f"(status={status}, Content-Range={content_range!r}); "
                            f"deleting .part for full re-download",
                            flush=True,
                        )
                        try:
                            out.close()
                        except Exception:
                            pass
                        part.unlink(missing_ok=True)
                        raise RuntimeError(
                            f"Range resume failed for {dest.name}: status={status} "
                            f"Content-Range={content_range!r}"
                        )
                while True:
                    chunk = resp.read(1 << 20)
                    if not chunk:
                        break
                    out.write(chunk)
            size = part.stat().st_size
            if expected_size is not None and size != expected_size:
                part.unlink(missing_ok=True)
                raise RuntimeError(
                    f"size mismatch for {dest.name}: got {size} expected {expected_size}"
                )
            digest = sha256_file(part)
            if expected_sha is not None and digest != expected_sha:
                part.unlink(missing_ok=True)
                raise RuntimeError(
                    f"sha256 mismatch for {dest.name}: got {digest} expected {expected_sha}"
                )
            os.replace(part, dest)
            print(f"done {dest.name} ({size} bytes) {digest}", flush=True)
            return size, digest
        except (urllib.error.URLError, TimeoutError, RuntimeError, OSError) as error:
            print(f"error {dest.name}: {error}", flush=True)
            # On range rejection we already deleted .part; other errors keep partial for retry.
            if attempt == retries:
                raise
            time.sleep(min(30, 2**attempt))
    raise RuntimeError(f"download failed for {dest}")


def ensure_compatible_dir(out_dir: Path) -> Path:
    """Hardlink-only compatible subset under out_dir. Never emits absolute symlinks."""
    out_dir = out_dir.resolve()
    compat = out_dir / "compatible-17level-with-pres"
    compat.mkdir(parents=True, exist_ok=True)
    for name in COMPATIBLE_MEMBERS:
        src = out_dir / name
        dst = compat / name
        if not src.is_file():
            raise FileNotFoundError(f"missing compatible member {src}")
        if dst.exists() or dst.is_symlink():
            dst.unlink()
        try:
            os.link(src, dst)
        except OSError as error:
            raise RuntimeError(
                f"hardlink failed for {src.name} -> {dst} ({error}). "
                f"Refusing symlink fallback that can escape the data root. "
                f"Run on a filesystem that supports hardlinks within {out_dir}."
            ) from error
        # Path must remain under out_dir; size must match the source member.
        dst_resolved = dst.resolve()
        try:
            dst_resolved.relative_to(out_dir)
        except ValueError as error:
            dst.unlink(missing_ok=True)
            raise RuntimeError(
                f"compatible path escaped data root: {dst_resolved}"
            ) from error
        if dst.stat().st_size != src.stat().st_size:
            raise RuntimeError(f"compatible hardlink size mismatch for {name}")
    print(f"compatible dir ready (hardlinks only): {compat}", flush=True)
    return compat


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=Path("target/test-data/noaa-psl-ncep-reanalysis1-official"),
    )
    args = parser.parse_args()
    records = []
    for kind, name in DEFAULT_FILES:
        url = url_for(kind, name)
        dest = args.out_dir / name
        size, digest = download(url, dest, FROZEN_SHA256.get(name))
        records.append(
            {
                "name": name,
                "kind": kind,
                "url": url,
                "path": str(dest.as_posix()),
                "size": size,
                "sha256": digest,
            }
        )
    ensure_compatible_dir(args.out_dir)
    manifest = {
        "dataset": "noaa_psl_ncep_reanalysis1_official_multifile_netcdf",
        "source": "https://downloads.psl.noaa.gov/Datasets/ncep.reanalysis/",
        "license": "NOAA/NCEP reanalysis data; PSL redistribution",
        "attribution": (
            "NCEP/NCAR Reanalysis 1 data provided by the NOAA PSL, Boulder, Colorado, USA, "
            "from their website at https://psl.noaa.gov"
        ),
        "compatible_subdir": "compatible-17level-with-pres",
        "compatible_members": COMPATIBLE_MEMBERS,
        "files": records,
    }
    out = args.out_dir / "FETCH_MANIFEST.json"
    out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print("wrote", out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
