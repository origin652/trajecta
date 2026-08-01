#!/usr/bin/env python3
"""Validate the deployed bilingual release and noindex dev documentation."""

from __future__ import annotations

import argparse
import json
import re
import urllib.request
from urllib.parse import urljoin


BASE = "https://origin652.github.io/trajecta/"
VERSION = "0.1.0-alpha.1"
USER_AGENT = "Trajecta-M5.1-docs-check/1"


def fetch(url: str) -> tuple[str, str]:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status != 200:
            raise ValueError(f"unexpected HTTP {response.status}: {url}")
        return response.geturl(), response.read().decode("utf-8")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def validate(base: str, version: str) -> None:
    base = base.rstrip("/") + "/"
    release = urljoin(base, f"{version}/")
    final_url, home = fetch(base)
    require(final_url.rstrip("/") == release.rstrip("/"), "default URL does not resolve to the release")
    require("noindex" not in home.casefold(), "release home has noindex")
    require('hreflang="en"' in home and 'hreflang="zh-CN"' in home, "release home lacks hreflang")
    require('rel="canonical"' in home and release in home, "release canonical URL is not versioned")
    for term in (
        "Lagrangian water-vapor tracking",
        "Lagrangian moisture tracking",
        "domain filling",
        "atmospheric trajectories",
        "forward and backward trajectories",
    ):
        require(term.casefold() in home.casefold(), f"release home lacks SEO term: {term}")

    _zh_url, chinese = fetch(urljoin(release, "zh-CN/"))
    require('lang="zh"' in chinese or 'lang="zh-CN"' in chinese, "Chinese page language is missing")
    require('hreflang="en"' in chinese and 'hreflang="zh-CN"' in chinese, "Chinese page lacks hreflang")

    _dev_url, dev = fetch(urljoin(base, "dev/"))
    require('name="robots" content="noindex,nofollow"' in dev.casefold(), "dev home lacks noindex,nofollow")
    _robots_url, robots = fetch(urljoin(base, "robots.txt"))
    require("Disallow: /trajecta/dev/" in robots, "robots.txt does not exclude dev")
    require(f"Sitemap: {base}sitemap.xml" in robots, "robots.txt sitemap URL drifted")
    _sitemap_url, sitemap = fetch(urljoin(base, "sitemap.xml"))
    require(release in sitemap and f"{release}zh-CN/" in sitemap, "root sitemap lacks bilingual release URLs")

    _search_url, search_text = fetch(urljoin(release, "search/search_index.json"))
    search = json.loads(search_text)
    locations = [item.get("location", "") for item in search.get("docs", [])]
    require(not any("engineering" in location for location in locations), "engineering history leaked into search")
    require(any(location.startswith("zh-CN/") for location in locations), "Chinese pages are absent from search")
    require(re.search(r"<title>[^<]+</title>", home) is not None, "release home has no title")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=BASE)
    parser.add_argument("--version", default=VERSION)
    args = parser.parse_args()
    validate(args.base, args.version)
    print(f"M5.1 public site passed: {args.base}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
