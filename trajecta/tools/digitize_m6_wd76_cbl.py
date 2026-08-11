#!/usr/bin/env python3
"""Digitize the WD76 concentration profiles reproduced by Weil et al. (2012)."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

import pdfplumber


SOURCE_SHA256 = "38d084754c806ec6772aeca44d901115a1faa72ae0214371981d47b57e9bb61f"
SOURCE_SIZE_BYTES = 1_549_241
PAGE_INDEX = 11
HEIGHT_AXIS_MAX = 1.25
PANEL_TIMES = [0.12, 0.25, 0.38, 0.50, 1.55, 2.95]
CONCENTRATION_AXIS_MAX = [12.5, 10.0, 7.0, 5.0, 2.5, 2.0]
EXPECTED_POINT_COUNTS = [5, 8, 11, 13, 17, 17]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def rounded(value: float) -> float:
    return round(value, 8)


def center(item: dict[str, Any]) -> tuple[float, float]:
    return (0.5 * (item["x0"] + item["x1"]), 0.5 * (item["top"] + item["bottom"]))


def digitize(source: Path) -> dict[str, Any]:
    if source.stat().st_size != SOURCE_SIZE_BYTES or sha256(source) != SOURCE_SHA256:
        raise ValueError("input PDF does not match the frozen NCAR manuscript identity")

    with pdfplumber.open(source) as document:
        if len(document.pages) != 29:
            raise ValueError("unexpected source page count")
        page = document.pages[PAGE_INDEX]
        panels = sorted(page.rects, key=lambda rect: (rect["top"], rect["x0"]))
        if len(panels) != 6:
            raise ValueError("Figure 4 panel frames could not be identified")
        markers = [char for char in page.chars if char["text"] == "ⓕ"]

    profiles = []
    removed_legend_markers = []
    for panel_index, (panel, time, concentration_max) in enumerate(
        zip(panels, PANEL_TIMES, CONCENTRATION_AXIS_MAX, strict=True)
    ):
        selected = []
        for marker in markers:
            x, y = center(marker)
            if not (
                panel["x0"] <= x <= panel["x1"]
                and panel["top"] <= y <= panel["bottom"]
            ):
                continue
            if panel_index == 0 and x < 160.0 and y < 170.0:
                removed_legend_markers.append(marker)
                continue
            selected.append(marker)
        selected.sort(key=lambda marker: center(marker)[1], reverse=True)
        if len(selected) != EXPECTED_POINT_COUNTS[panel_index]:
            raise ValueError(
                f"panel {time:.2f} contains {len(selected)} points; "
                f"expected {EXPECTED_POINT_COUNTS[panel_index]}"
            )

        points = []
        for marker in selected:
            x, y = center(marker)
            points.append(
                {
                    "pdf_bbox_points": [
                        rounded(marker["x0"]),
                        rounded(marker["top"]),
                        rounded(marker["x1"]),
                        rounded(marker["bottom"]),
                    ],
                    "pdf_center_points": [rounded(x), rounded(y)],
                    "height_fraction": rounded(
                        (panel["bottom"] - y) / panel["height"] * HEIGHT_AXIS_MAX
                    ),
                    "dimensionless_concentration": rounded(
                        (x - panel["x0"]) / panel["width"] * concentration_max
                    ),
                }
            )

        profiles.append(
            {
                "dimensionless_time": time,
                "source_height_fraction": 0.07,
                "panel_pdf_bbox_points": [
                    rounded(panel["x0"]),
                    rounded(panel["top"]),
                    rounded(panel["x1"]),
                    rounded(panel["bottom"]),
                ],
                "axis": {
                    "height_fraction": [0.0, HEIGHT_AXIS_MAX],
                    "dimensionless_concentration": [0.0, concentration_max],
                },
                "normalization_scale": rounded(
                    max(point["dimensionless_concentration"] for point in points)
                ),
                "points": points,
            }
        )

    if len(removed_legend_markers) != 1 or sum(map(len, (p["points"] for p in profiles))) != 71:
        raise ValueError("Figure 4 marker accounting is incomplete")

    legend_x, legend_y = center(removed_legend_markers[0])
    return {
        "schema_version": "trajecta.m6.wd76-cbl/v1",
        "asset_id": "willis-deardorff-cbl-1976/v1",
        "primary_source": {
            "title": "A laboratory model of diffusion into the convective planetary boundary layer",
            "authors": ["G. E. Willis", "J. W. Deardorff"],
            "doi": "10.1002/qj.49710243212",
        },
        "digitized_reproduction": {
            "title": "Statistical Variability of Dispersion in the Convective Boundary Layer: Ensembles of Simulations and Observations",
            "authors": [
                "Jeffrey C. Weil",
                "Peter P. Sullivan",
                "Edward G. Patton",
                "Chin-Hoh Moeng",
            ],
            "doi": "10.1007/s10546-012-9704-y",
            "repository_pdf_url": "https://www2.mmm.ucar.edu/people/sullivan/talks/papers/weil_blm.pdf",
            "pdf_size_bytes": SOURCE_SIZE_BYTES,
            "pdf_sha256": SOURCE_SHA256,
            "pdf_page": PAGE_INDEX + 1,
            "figure": "4",
        },
        "digitization": {
            "method": "extract filled laboratory-marker glyph bounding boxes from the PDF content stream and apply the published panel-axis transforms",
            "pdf_coordinate_system": "points from page top-left",
            "height_axis_max": HEIGHT_AXIS_MAX,
            "point_count": 71,
            "excluded_legend_marker_pdf_center_points": [
                rounded(legend_x),
                rounded(legend_y),
            ],
            "coordinate_uncertainty": "raw glyph bounding boxes are retained; half the plotted marker width and height bound the reading precision",
            "normalization": "each profile uses its maximum observed dimensionless concentration because the publication reports no pointwise uncertainty",
        },
        "profiles": profiles,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_pdf", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    encoded = json.dumps(digitize(args.source_pdf), indent=2, ensure_ascii=False) + "\n"
    if args.output is None:
        print(encoded, end="")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
