#!/usr/bin/env python3
"""Validate the bilingual M5.1 documentation source and optional built site."""

from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import sys
from pathlib import Path, PurePosixPath
from urllib.parse import unquote, urlsplit

import yaml


ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs" / "user"
MKDOCS = ROOT / "mkdocs.yml"
CONTRACT = ROOT / "testdata" / "M5_CLI_CONTRACT.v1.json"
LOCALES = ("en", "zh-CN")
FRONT_MATTER = re.compile(r"\A---\n(.*?)\n---\n", re.DOTALL)
MARKDOWN_LINK = re.compile(r"!?\[[^\]]*\]\(([^)]+)\)")
SNIPPET = re.compile(r'^\s*--8<--\s+["\']([^"\']+)["\']\s*$', re.MULTILINE)
HEADING = re.compile(r"^(#{1,6})\s+", re.MULTILINE)
ACRONYM = re.compile(r"\b[A-Z][A-Z0-9]+(?:-[A-Z0-9]+)*\b")
KNOWN_ACRONYMS = {
    "AI",
    "ABI",
    "API",
    "ARM64",
    "ASL",
    "CDS",
    "CF",
    "CFSR",
    "CI",
    "CLI",
    "CPU",
    "CSV",
    "CSS",
    "DLL",
    "ERA5",
    "FIFO",
    "FLEXPART",
    "GRIB",
    "GRIB2",
    "GRBLOW",
    "GNU",
    "HDF5",
    "HTML",
    "ID",
    "IPC",
    "JSON",
    "JSONL",
    "LTS",
    "M4",
    "M5",
    "M5-A3",
    "M5-A5",
    "MIT",
    "MSVC",
    "MSYS2",
    "NCEI",
    "NOAA",
    "NCEP",
    "OOM",
    "OS",
    "PID",
    "PNG",
    "PR",
    "PV60",
    "RSS",
    "RK2",
    "README",
    "SBOM",
    "SEO",
    "SHA-256",
    "SQL",
    "SHM",
    "SVG",
    "TLS",
    "TOML",
    "URL",
    "UTC",
    "UCRT64",
    "UUID",
    "WAL",
    "WSL2",
    "YAML",
    "ZIP",
}
SEO_TERMS = (
    "Lagrangian water-vapor tracking",
    "Lagrangian moisture tracking",
    "domain filling",
    "atmospheric trajectories",
    "forward and backward trajectories",
)


class ValidationError(RuntimeError):
    """One or more documentation contracts failed."""


def require(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


def relative_files(locale: str) -> set[PurePosixPath]:
    base = DOCS / locale
    return {
        PurePosixPath(path.relative_to(base).as_posix())
        for path in base.rglob("*")
        if path.is_file()
    }


def markdown_files(locale: str) -> list[Path]:
    return sorted((DOCS / locale).rglob("*.md"))


def metadata(text: str) -> dict[str, object] | None:
    match = FRONT_MATTER.match(text.replace("\r\n", "\n"))
    if not match:
        return None
    value = yaml.safe_load(match.group(1))
    return value if isinstance(value, dict) else None


def strip_nonprose(text: str) -> str:
    text = FRONT_MATTER.sub("", text.replace("\r\n", "\n"), count=1)
    text = re.sub(r"```.*?```", "", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", "", text)
    text = re.sub(r"https?://\S+", "", text)
    lines = [
        line
        for line in text.splitlines()
        if not line.lstrip().startswith(("|", "#", "--8<--"))
    ]
    return "\n".join(lines)


def flatten_nav(value: object) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [path for item in value for path in flatten_nav(item)]
    if isinstance(value, dict):
        return [path for item in value.values() for path in flatten_nav(item)]
    return []


def validate_structure(config: dict[str, object], failures: list[str]) -> None:
    baseline = relative_files("en")
    for locale in LOCALES[1:]:
        files = relative_files(locale)
        require(files == baseline, f"bilingual file parity failed for {locale}", failures)

    nav = flatten_nav(config.get("nav"))
    require(len(nav) == len(set(nav)), "MkDocs navigation contains duplicate paths", failures)
    nav_paths = {PurePosixPath(path) for path in nav}
    published = {path for path in baseline if path.suffix == ".md"}
    require(nav_paths == published, "MkDocs navigation and Markdown page set differ", failures)
    for path in nav_paths:
        for locale in LOCALES:
            require((DOCS / locale / Path(*path.parts)).is_file(), f"missing nav page {locale}/{path}", failures)

    for relative in sorted(published):
        heading_shapes = []
        for locale in LOCALES:
            text = (DOCS / locale / Path(*relative.parts)).read_text(encoding="utf-8")
            front = metadata(text)
            require(front is not None, f"missing front matter: {locale}/{relative}", failures)
            if front is not None:
                require(isinstance(front.get("title"), str) and bool(front["title"].strip()), f"missing title: {locale}/{relative}", failures)
                require(isinstance(front.get("description"), str) and bool(front["description"].strip()), f"missing description: {locale}/{relative}", failures)
            heading_shapes.append([len(match) for match in HEADING.findall(text)])
        require(heading_shapes[0] == heading_shapes[1], f"heading-level parity failed: {relative}", failures)


def local_link_target(source: Path, raw: str) -> Path | None:
    raw = raw.strip().strip("<>")
    if not raw or raw.startswith(("#", "mailto:", "http://", "https://")):
        return None
    path = unquote(urlsplit(raw).path)
    if not path:
        return None
    if path.startswith("/"):
        return DOCS / "en" / path.lstrip("/")
    return (source.parent / path).resolve(strict=False)


def validate_links_and_snippets(failures: list[str]) -> None:
    for locale in LOCALES:
        for page in markdown_files(locale):
            text = page.read_text(encoding="utf-8")
            for raw in MARKDOWN_LINK.findall(text):
                target = local_link_target(page, raw.split(maxsplit=1)[0])
                if target is None:
                    continue
                require(target.is_file(), f"broken local link in {page.relative_to(ROOT)}: {raw}", failures)
            for line in text.splitlines():
                match = SNIPPET.match(line)
                if not match:
                    continue
                snippet = ROOT / match.group(1)
                require(snippet.is_file(), f"missing snippet in {page.relative_to(ROOT)}: {match.group(1)}", failures)
                require(match.group(1).startswith("examples/"), f"user snippet must come from examples/: {match.group(1)}", failures)
    for tutorial in ("domain-fill.md", "release.md", "air-mass.md", "ozone.md"):
        for locale in LOCALES:
            text = (DOCS / locale / "tutorials" / tutorial).read_text(encoding="utf-8")
            require(bool(SNIPPET.search(text)), f"tutorial has no executable example snippet: {locale}/{tutorial}", failures)


def command_paths() -> set[str]:
    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    return {item["path"] for item in contract["commands"]} | {"run report"}


def documented_command(line: str) -> str | None:
    stripped = line.strip()
    if not stripped.startswith(("trajecta ", "trajecta.exe ")):
        return None
    try:
        tokens = shlex.split(stripped, posix=True)
    except ValueError:
        return "<unparseable>"
    if not tokens or Path(tokens[0]).name not in {"trajecta", "trajecta.exe"}:
        return None
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"--format", "--config", "--project"}:
            index += 2
            continue
        if token == "--json":
            index += 1
            continue
        if token in {"--help", "-h"}:
            return None
        break
    if index >= len(tokens):
        return None
    root = tokens[index]
    if root in {"doctor", "run"}:
        if root == "run" and index + 1 < len(tokens) and tokens[index + 1] == "report":
            return "run report"
        return root
    if index + 1 >= len(tokens):
        return root
    return f"{root} {tokens[index + 1]}"


def validate_commands_and_generated(binary: Path, failures: list[str]) -> None:
    allowed = command_paths()
    for locale in LOCALES:
        for page in markdown_files(locale):
            text = page.read_text(encoding="utf-8")
            if "Generated by tools/generate_m5_1_reference.py" in text:
                continue
            for line in text.splitlines():
                path = documented_command(line)
                if path is not None:
                    require(path in allowed, f"unknown documented CLI command in {page.relative_to(ROOT)}: {path}", failures)
            lowered = text.casefold()
            require("trajecta plugin" not in lowered and "trajecta export" not in lowered, f"future command presented in {page.relative_to(ROOT)}", failures)

    generator = subprocess.run(
        [sys.executable, str(ROOT / "tools" / "generate_m5_1_reference.py"), "--binary", str(binary), "--check"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )
    require(generator.returncode == 0, "generated reference drift: " + generator.stdout.strip(), failures)


def validate_style_and_scope(failures: list[str]) -> None:
    for locale in LOCALES:
        for page in markdown_files(locale):
            relative = page.relative_to(DOCS / locale)
            text = page.read_text(encoding="utf-8")
            if "flexpart" in text.casefold():
                require(relative.parts[0] == "validation", f"comparison term outside Validation: {locale}/{relative}", failures)
            prose = strip_nonprose(text)
            if locale == "zh-CN":
                require(not re.search(r"不是.{0,40}而是", prose, re.DOTALL), f"forbidden template contrast: {locale}/{relative}", failures)
                for sentence in re.split(r"[。！？]", prose):
                    require(sentence.count("、") < 5, f"dense Chinese enumeration: {locale}/{relative}", failures)
                    cjk = re.findall(r"[\u3400-\u9fff]", sentence)
                    require(len(cjk) <= 240, f"Chinese sentence exceeds 240 characters: {locale}/{relative}", failures)
            else:
                unknown = sorted(set(ACRONYM.findall(prose)) - KNOWN_ACRONYMS)
                require(not unknown, f"undefined uppercase abbreviations in {locale}/{relative}: {unknown}", failures)

    for locale in LOCALES:
        extensibility = (DOCS / locale / "concepts" / "extensibility.md").read_text(encoding="utf-8").casefold()
        marker = "not implemented" if locale == "en" else "尚未实现"
        require(marker in extensibility, f"extensibility page lacks unavailable marker: {locale}", failures)


def validate_source_seo(config: dict[str, object], failures: list[str]) -> None:
    require(config.get("site_url") == "https://origin652.github.io/trajecta/", "site_url drift", failures)
    require(config.get("site_description") == "Open-source Lagrangian water-vapor tracking and atmospheric trajectory framework.", "site description drift", failures)
    home = (DOCS / "en" / "index.md").read_text(encoding="utf-8").casefold()
    for term in SEO_TERMS:
        require(term.casefold() in home, f"English home is missing SEO term: {term}", failures)
    robots = (DOCS / "en" / "robots.txt").read_text(encoding="utf-8")
    require(
        "Allow: /" in robots
        and "Disallow: /trajecta/dev/" in robots
        and "Sitemap:" in robots,
        "robots.txt contract failed",
        failures,
    )
    dev = ROOT / "mkdocs.dev.yml"
    require(dev.is_file(), "mkdocs.dev.yml is missing", failures)
    if dev.is_file():
        dev_config = yaml.safe_load(dev.read_text(encoding="utf-8"))
        dev_extra = dev_config.get("extra", {}) if isinstance(dev_config, dict) else {}
        override = ROOT / "docs" / "site-overrides" / "main.html"
        override_text = override.read_text(encoding="utf-8").casefold() if override.is_file() else ""
        require(
            isinstance(dev_extra, dict)
            and dev_extra.get("noindex") is True
            and "noindex,nofollow" in override_text,
            "dev docs do not declare noindex,nofollow",
            failures,
        )


def validate_built_site(site: Path, expect_noindex: bool, failures: list[str]) -> None:
    require((site / "sitemap.xml").is_file(), "built site has no sitemap.xml", failures)
    require((site / "robots.txt").is_file(), "built site has no robots.txt", failures)
    html_pages = [path for path in site.rglob("*.html") if path.name != "404.html"]
    require(bool(html_pages), "built site contains no HTML pages", failures)
    for page in html_pages:
        text = page.read_text(encoding="utf-8")
        require('rel="canonical"' in text, f"built page has no canonical URL: {page.relative_to(site)}", failures)
        has_noindex = 'name="robots" content="noindex,nofollow"' in text.casefold()
        if expect_noindex:
            require(has_noindex, f"dev page has no noindex marker: {page.relative_to(site)}", failures)
        else:
            require(not has_noindex, f"release page unexpectedly has noindex: {page.relative_to(site)}", failures)
        require('hreflang="en"' in text and 'hreflang="zh-CN"' in text, f"built page lacks bilingual hreflang: {page.relative_to(site)}", failures)
    search = site / "search" / "search_index.json"
    if search.is_file():
        search_index = json.loads(search.read_text(encoding="utf-8"))
        locations = [item.get("location", "") for item in search_index.get("docs", [])]
        require(
            all("docs/engineering" not in location for location in locations),
            "engineering docs leaked into search",
            failures,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    suffix = ".exe" if sys.platform == "win32" else ""
    parser.add_argument("--binary", type=Path, default=ROOT / "target" / "debug" / f"trajecta-cli{suffix}")
    parser.add_argument("--site-dir", type=Path)
    parser.add_argument("--expect-noindex", action="store_true")
    args = parser.parse_args()
    config = yaml.safe_load(MKDOCS.read_text(encoding="utf-8"))
    if not isinstance(config, dict):
        raise SystemExit("mkdocs.yml must contain a mapping")
    failures: list[str] = []
    validate_structure(config, failures)
    validate_links_and_snippets(failures)
    validate_commands_and_generated(args.binary.resolve(), failures)
    validate_style_and_scope(failures)
    validate_source_seo(config, failures)
    if args.site_dir:
        validate_built_site(args.site_dir.resolve(), args.expect_noindex, failures)
    if failures:
        for failure in failures:
            print(f"M5.1 docs: {failure}", file=sys.stderr)
        return 1
    print(f"M5.1 docs passed: {len(markdown_files('en'))} bilingual pages")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
