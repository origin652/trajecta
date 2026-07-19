#!/usr/bin/env python3
"""Thin wrapper: classic conversion is owned by prepare_era5_pressure_anchors.py."""
from __future__ import annotations

import runpy
import sys
from pathlib import Path


def main() -> int:
    prepare = Path(__file__).with_name("prepare_era5_pressure_anchors.py")
    sys.argv = [str(prepare)]
    runpy.run_path(str(prepare), run_name="__main__")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
