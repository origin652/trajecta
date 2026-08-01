#!/usr/bin/env python3
"""Fetch one CFSR pgbl GRIB2 hour from NCEI with safe TLS, Range resume, and atomic finish.

Integrity contract:
- Never disables certificate verification.
- Uses `.part` + atomic rename.
- Validates HTTP 206 Content-Range start against local offset.
- Validates Content-Length / total size when present.
- On size/SHA mismatch or HTTP 416: delete bad `.part` (and bad dest) and full re-download.
- Optional frozen size/sha256 must match before rename.
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import os
import re
import ssl
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

URL_CANDIDATES = [
    "https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/{yyyy}/{yyyymm}/{yyyymmdd}/pgbl00.gdas.{stamp}.grb2",
    "https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-by-pressure-level/{yyyy}/{yyyymm}/{yyyymmdd}/pgbl00.gdas.{stamp}.grb2",
    "https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-pressure/{yyyy}/{yyyymm}/{yyyymmdd}/pgbl00.gdas.{stamp}.grb2",
]

CONTENT_RANGE_RE = re.compile(
    r"bytes\s+(?P<start>\d+)-(?P<end>\d+)/(?P<total>\d+|\*)", re.IGNORECASE
)


class IntegrityError(RuntimeError):
    """Size/hash/range integrity failure that requires discarding partial state."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _opener() -> urllib.request.OpenerDirector:
    proxy = os.environ.get("HTTPS_PROXY") or os.environ.get("https_proxy")
    proxy = proxy or os.environ.get("HTTP_PROXY") or os.environ.get("http_proxy")
    proxy = proxy or os.environ.get("ALL_PROXY") or os.environ.get("all_proxy")
    if proxy:
        return urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": proxy, "https": proxy})
        )
    return urllib.request.build_opener()


def _parse_content_range(header: str | None) -> tuple[int, int, int | None] | None:
    if not header:
        return None
    match = CONTENT_RANGE_RE.search(header.strip())
    if not match:
        return None
    start = int(match.group("start"))
    end = int(match.group("end"))
    total_raw = match.group("total")
    total = None if total_raw == "*" else int(total_raw)
    return start, end, total


def _discard_partial(part: Path, dest: Path | None = None) -> None:
    part.unlink(missing_ok=True)
    if dest is not None and dest.is_file():
        # Only remove dest when caller is recovering from integrity failure of an
        # existing completed object that failed frozen checks.
        pass


def download(
    url: str,
    dest: Path,
    *,
    expected_size: int | None,
    expected_sha256: str | None,
    retries: int = 8,
) -> tuple[int, str]:
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_suffix(dest.suffix + ".part")
    opener = _opener()
    existing = part.stat().st_size if part.is_file() else 0
    attempt = 0
    declared_total: int | None = expected_size
    while attempt < retries:
        attempt += 1
        headers = {"User-Agent": "trajecta-fetch-cfsr/0.1"}
        if existing > 0:
            headers["Range"] = f"bytes={existing}-"
        request = urllib.request.Request(url, headers=headers)
        try:
            with opener.open(request, timeout=300) as response:
                status = getattr(response, "status", 200)
                content_length = response.headers.get("Content-Length")
                content_range = response.headers.get("Content-Range")

                if existing > 0:
                    if status == 200:
                        part.unlink(missing_ok=True)
                        existing = 0
                        continue
                    if status == 416:
                        # Stale complete/corrupt partial; full restart.
                        part.unlink(missing_ok=True)
                        existing = 0
                        continue
                    if status != 206:
                        part.unlink(missing_ok=True)
                        existing = 0
                        raise IntegrityError(f"expected HTTP 206 for resume, got {status}")
                    parsed = _parse_content_range(content_range)
                    if parsed is None:
                        part.unlink(missing_ok=True)
                        existing = 0
                        raise IntegrityError(
                            f"HTTP 206 missing/invalid Content-Range: {content_range!r}"
                        )
                    start, end, total = parsed
                    if start != existing:
                        part.unlink(missing_ok=True)
                        existing = 0
                        raise IntegrityError(
                            f"Content-Range start {start} != local offset {existing}"
                        )
                    if total is not None:
                        declared_total = total
                    if content_length is not None:
                        expected_chunk = int(content_length)
                        if end + 1 - start != expected_chunk:
                            part.unlink(missing_ok=True)
                            existing = 0
                            raise IntegrityError(
                                "Content-Range length disagrees with Content-Length"
                            )
                else:
                    if status != 200:
                        raise RuntimeError(f"expected HTTP 200, got {status}")
                    if content_length is not None:
                        declared_total = int(content_length)

                mode = "ab" if existing > 0 and status == 206 else "wb"
                if mode == "wb":
                    existing = 0
                with part.open(mode) as out:
                    while True:
                        chunk = response.read(1024 * 1024)
                        if not chunk:
                            break
                        out.write(chunk)
                        existing += len(chunk)

            size = part.stat().st_size
            if declared_total is not None and size != declared_total:
                part.unlink(missing_ok=True)
                existing = 0
                raise IntegrityError(
                    f"downloaded size {size} != declared total {declared_total}"
                )
            if expected_size is not None and size != expected_size:
                part.unlink(missing_ok=True)
                existing = 0
                raise IntegrityError(
                    f"downloaded size {size} != expected frozen size {expected_size}"
                )
            digest = sha256_file(part)
            if expected_sha256 is not None and digest.lower() != expected_sha256.lower():
                part.unlink(missing_ok=True)
                existing = 0
                raise IntegrityError(
                    f"sha256 {digest} != expected frozen sha256 {expected_sha256}"
                )
            os.replace(part, dest)
            return size, digest
        except urllib.error.HTTPError as error:
            if error.code == 404:
                raise
            if error.code == 416:
                part.unlink(missing_ok=True)
                existing = 0
                if attempt >= retries:
                    raise
                time.sleep(min(2**attempt, 30))
                continue
            if attempt >= retries:
                raise
            time.sleep(min(2**attempt, 30))
        except IntegrityError as error:
            # Always discard partials after integrity failure so the next attempt
            # is a full re-download, never a Range resume from a poisoned .part.
            part.unlink(missing_ok=True)
            existing = 0
            declared_total = expected_size
            print(f"integrity failure, full re-download: {error}", file=sys.stderr)
            if attempt >= retries:
                raise
            time.sleep(min(2**attempt, 30))
        except (urllib.error.URLError, TimeoutError, ssl.SSLError, OSError, RuntimeError) as error:
            if attempt >= retries:
                raise
            print(f"retry after error: {error}", file=sys.stderr)
            time.sleep(min(2**attempt, 30))
    raise RuntimeError(f"failed to download {url}")


def ensure_valid_existing(
    dest: Path, expected_size: int | None, expected_sha256: str | None
) -> tuple[int, str] | None:
    if not dest.is_file():
        return None
    size = dest.stat().st_size
    digest = sha256_file(dest)
    if expected_size is not None and size != expected_size:
        print(
            f"existing dest size mismatch {size} != {expected_size}; deleting for re-download",
            file=sys.stderr,
        )
        dest.unlink(missing_ok=True)
        return None
    if expected_sha256 is not None and digest.lower() != expected_sha256.lower():
        print(
            f"existing dest sha mismatch {digest}; deleting for re-download",
            file=sys.stderr,
        )
        dest.unlink(missing_ok=True)
        return None
    return size, digest


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stamp", default=None, help="YYYYMMDDHH e.g. 2009010112")
    parser.add_argument("--out-dir", default=None)
    parser.add_argument("--expected-size", type=int, default=None)
    parser.add_argument("--expected-sha256", default=None)
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run local HTTP integrity tests and exit",
    )
    args = parser.parse_args()
    if args.self_test:
        return 0 if run_self_tests() else 1
    if not args.stamp or not args.out_dir:
        print("--stamp and --out-dir are required (unless --self-test)", file=sys.stderr)
        return 2
    stamp = args.stamp
    if len(stamp) != 10 or not stamp.isdigit():
        print("stamp must be YYYYMMDDHH", file=sys.stderr)
        return 2
    yyyy = stamp[0:4]
    yyyymm = stamp[0:6]
    yyyymmdd = stamp[0:8]
    out_dir = Path(args.out_dir)
    dest = out_dir / f"pgbl00.gdas.{stamp}.grb2"
    existing = ensure_valid_existing(dest, args.expected_size, args.expected_sha256)
    if existing is not None:
        size, digest = existing
        print(f"exists size={size} sha256={digest} path={dest}")
        return 0
    last_error = None
    for template in URL_CANDIDATES:
        url = template.format(yyyy=yyyy, yyyymm=yyyymm, yyyymmdd=yyyymmdd, stamp=stamp)
        print(f"try {url}", file=sys.stderr)
        try:
            size, digest = download(
                url,
                dest,
                expected_size=args.expected_size,
                expected_sha256=args.expected_sha256,
            )
            print(f"ok size={size} sha256={digest} path={dest}")
            return 0
        except Exception as error:  # noqa: BLE001
            last_error = error
            print(f"fail {error}", file=sys.stderr)
            part = dest.with_suffix(dest.suffix + ".part")
            # Integrity path already unlinks; network errors may keep partial for resume.
            if isinstance(error, IntegrityError) and part.is_file():
                part.unlink(missing_ok=True)
    print(f"all candidates failed: {last_error}", file=sys.stderr)
    return 1


def run_self_tests() -> bool:
    """Local HTTP server tests: 200/206/416 and hash recovery without network."""
    payload = b"CFSR-FIXTURE-BYTES-" + os.urandom(64)
    digest = hashlib.sha256(payload).hexdigest()
    size = len(payload)

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, fmt: str, *args: object) -> None:  # noqa: A003
            return

        def do_GET(self) -> None:  # noqa: N802
            range_header = self.headers.get("Range")
            if range_header:
                # bytes=N-
                try:
                    start = int(range_header.split("=", 1)[1].split("-", 1)[0])
                except Exception:
                    self.send_error(400)
                    return
                if start >= size:
                    self.send_error(416)
                    return
                body = payload[start:]
                end = size - 1
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            self.send_response(200)
            self.send_header("Content-Length", str(size))
            self.end_headers()
            self.wfile.write(payload)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{port}/pgbl00.gdas.2009010112.grb2"
    try:
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "pgbl00.gdas.2009010112.grb2"
            # Full download with frozen hash.
            got_size, got_digest = download(
                url, dest, expected_size=size, expected_sha256=digest, retries=3
            )
            assert got_size == size and got_digest == digest
            assert dest.is_file()

            # Poison .part then force integrity failure recovery.
            part = dest.with_suffix(dest.suffix + ".part")
            dest.unlink()
            part.write_bytes(b"CORRUPT-PARTIAL-XXXX")
            got_size, got_digest = download(
                url, dest, expected_size=size, expected_sha256=digest, retries=4
            )
            assert got_size == size and got_digest == digest
            assert not part.exists()

            # Wrong frozen hash must delete .part and fail (then succeed after correct hash).
            dest.unlink()
            try:
                download(
                    url,
                    dest,
                    expected_size=size,
                    expected_sha256="0" * 64,
                    retries=2,
                )
                raise AssertionError("expected integrity failure")
            except IntegrityError:
                part = dest.with_suffix(dest.suffix + ".part")
                assert not part.exists(), "bad hash must not leave poisoned .part"
                assert not dest.exists()

            # Existing corrupt dest is deleted by ensure_valid_existing.
            dest.write_bytes(b"bad-final")
            assert ensure_valid_existing(dest, size, digest) is None
            assert not dest.exists()
        print("self-test ok")
        return True
    except Exception as error:  # noqa: BLE001
        print(f"self-test failed: {error}", file=sys.stderr)
        return False
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    raise SystemExit(main())
