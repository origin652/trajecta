#!/usr/bin/env python3
"""Build root robots.txt and sitemap.xml for a mike-hosted release snapshot."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


BASE = "https://origin652.github.io/trajecta/"
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")


def build(site: Path, output: Path, version: str) -> None:
    if VERSION.fullmatch(version) is None:
        raise ValueError(f"invalid documentation version: {version}")
    source = site / "sitemap.xml"
    sitemap = source.read_text(encoding="utf-8")
    if BASE not in sitemap:
        raise ValueError("release sitemap does not contain the public site URL")
    versioned = sitemap.replace(BASE, f"{BASE}{version}/")
    output.mkdir(parents=True, exist_ok=True)
    (output / "sitemap.xml").write_text(versioned, encoding="utf-8", newline="\n")
    (output / "robots.txt").write_text(
        "User-agent: *\n"
        "Allow: /\n"
        "Disallow: /trajecta/dev/\n"
        f"Sitemap: {BASE}sitemap.xml\n",
        encoding="utf-8",
        newline="\n",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--site-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", default="0.1.0-alpha.1")
    args = parser.parse_args()
    build(args.site_dir.resolve(), args.output.resolve(), args.version)
    print(f"M5.1 root SEO assets built for {args.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
