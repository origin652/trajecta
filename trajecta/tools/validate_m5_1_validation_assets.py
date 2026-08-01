#!/usr/bin/env python3
"""Rebuild M5.1 publication charts and verify both language asset trees."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import tempfile

import run_m5_a5_formal as formal


ROOT = Path(__file__).resolve().parents[1]
ENGLISH = ROOT / "docs" / "user" / "en" / "assets" / "validation"
CHINESE = ROOT / "docs" / "user" / "zh-CN" / "assets" / "validation"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def files(root: Path) -> dict[str, Path]:
    return {
        path.relative_to(root).as_posix(): path
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def validate() -> None:
    aggregate_path = ENGLISH / "raw" / "M5_A5_FLEXPART_COMPARISON.json"
    aggregate = json.loads(aggregate_path.read_text(encoding="utf-8"))
    if aggregate.get("status") != "passed":
        raise ValueError("publication aggregate is not passed")

    frozen_charts = aggregate["artifacts"]["charts"]
    for name, expected in frozen_charts.items():
        path = ENGLISH / "raw" / "original-charts" / name
        if sha256(path) != expected:
            raise ValueError(f"original frozen chart SHA-256 mismatch: {name}")
    publication = json.loads(
        (ENGLISH / "PUBLICATION_MANIFEST.json").read_text(encoding="utf-8")
    )
    expected_charts = publication["charts"]
    for name, expected in expected_charts.items():
        if sha256(ENGLISH / "charts" / name) != expected:
            raise ValueError(f"publication chart SHA-256 mismatch: {name}")
    for key, name in (
        ("timing_csv", "M5_A5_TIMINGS.csv"),
        ("science_csv", "M5_A5_SCIENCE.csv"),
    ):
        expected = aggregate["artifacts"][key]["sha256"]
        if sha256(ENGLISH / "raw" / name) != expected:
            raise ValueError(f"frozen CSV SHA-256 mismatch: {name}")

    with tempfile.TemporaryDirectory() as temporary:
        rebuilt = Path(temporary)
        generated = formal.create_charts(
            rebuilt, aggregate["performance"], aggregate["scientific"]
        )
        if generated != expected_charts:
            raise ValueError("regenerated chart SHA-256 values drifted")

    english = files(ENGLISH)
    chinese = files(CHINESE)
    if english.keys() != chinese.keys():
        raise ValueError("validation asset paths differ between languages")
    for relative in english:
        if sha256(english[relative]) != sha256(chinese[relative]):
            raise ValueError(f"bilingual validation asset drift: {relative}")


def main() -> int:
    argparse.ArgumentParser(description=__doc__).parse_args()
    validate()
    print("M5.1 validation assets: passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
