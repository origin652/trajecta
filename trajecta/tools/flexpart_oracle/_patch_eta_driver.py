#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""RETIRED — not a build tool and not part of any oracle entrypoint.

Historical one-shot maintenance helper. Drivers/build scripts are first-class
sources under tools/flexpart_oracle/. Do not invoke this file.
"""
from __future__ import annotations
import sys
if __name__ == "__main__":
    sys.stderr.write(f"{__file__} is retired; not part of the oracle build path.\n")
    raise SystemExit(2)
