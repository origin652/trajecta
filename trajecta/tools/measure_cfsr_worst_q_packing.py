#!/usr/bin/env python3
"""Reproducible CFSR worst-q packing extraction (ecCodes 2.47.x).

Reads frozen pgbl files + cfsr_ulp_raw_calibration.json worst points, locates the
matching GRIB message via grib_ls, and writes structured packing fields:

  offset, dataRepresentationTemplateNumber, bitsPerValue, referenceValue,
  binaryScaleFactor, decimalScaleFactor, packingType, paramId, shortName,
  typeOfLevel, level

Does not modify M3_TOLERANCES registry.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CFSR_DIR = ROOT / "target/test-data/cfsr-ncei-pgbl-official"
CALIB = ROOT / "target/m3-comparison/cfsr_ulp_raw_calibration.json"
OUT = ROOT / "target/m3-comparison/cfsr_worst_q_packing.json"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def require_grib_ls() -> str:
    # Prefer repo-local native tree then PATH
    candidates = [
        ROOT / ".native/eccodes/Library/bin/grib_ls",
        ROOT / ".native/eccodes/Library/bin/grib_ls.exe",
    ]
    for c in candidates:
        if c.is_file():
            return str(c)
    import shutil

    p = shutil.which("grib_ls")
    if not p:
        print("INCOMPLETE: grib_ls not found", file=sys.stderr)
        raise SystemExit(2)
    return p


def grib_ls_table(grib_ls: str, path: Path) -> list[dict]:
    keys = [
        "count",
        "offset",
        "dataRepresentationTemplateNumber",
        "bitsPerValue",
        "referenceValue",
        "binaryScaleFactor",
        "decimalScaleFactor",
        "paramId",
        "shortName",
        "typeOfLevel",
        "level",
        "packingType",
        "discipline",
        "parameterCategory",
        "parameterNumber",
    ]
    env = os.environ.copy()
    defs = ROOT / ".native/eccodes/Library/share/eccodes/definitions"
    if defs.is_dir():
        env["ECCODES_DEFINITION_PATH"] = str(defs)
    proc = subprocess.run(
        [grib_ls, "-p", ",".join(keys), str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(f"grib_ls failed on {path}: {proc.stderr[-500:]}")
    lines = [ln for ln in (proc.stdout or "").splitlines() if ln.strip()]
    if len(lines) < 2:
        return []
    # grib_ls prints path line then header then rows — find header with 'offset'
    header_idx = None
    for i, ln in enumerate(lines):
        if "offset" in ln and "shortName" in ln.replace(" ", ""):
            header_idx = i
            break
        if "offset" in ln and i + 1 < len(lines):
            # sometimes header is space-separated key names
            header_idx = i
            break
    if header_idx is None:
        # treat first non-path line as header
        header_idx = 1 if lines[0].endswith(path.name) or path.name in lines[0] else 0
    header = lines[header_idx].split()
    rows = []
    for ln in lines[header_idx + 1 :]:
        if not ln.strip() or ln.startswith(str(path)) or path.name in ln and len(ln.split()) < 5:
            continue
        parts = ln.split()
        if len(parts) < len(header):
            continue
        # right-align: last len(header) tokens if extra path noise
        if len(parts) > len(header):
            parts = parts[-len(header) :]
        row = dict(zip(header, parts))
        rows.append(row)
    return rows


def coerce_num(v: str):
    try:
        if any(c in v for c in (".", "e", "E")):
            return float(v)
        return int(v)
    except Exception:
        return v


def pick_q_message(rows: list[dict], level_hpa: float | None) -> dict | None:
    qrows = []
    for r in rows:
        sn = str(r.get("shortName", r.get("shortname", "")))
        pid = str(r.get("paramId", r.get("paramid", "")))
        if sn == "q" or pid == "133":
            qrows.append(r)
    if not qrows:
        return None
    if level_hpa is None:
        return qrows[0]
    # closest level
    best = None
    best_d = 1e300
    for r in qrows:
        try:
            lev = float(r.get("level", "nan"))
        except Exception:
            continue
        d = abs(lev - level_hpa)
        if d < best_d:
            best_d = d
            best = r
    return best


def level_from_worst(worst: dict, field_report: dict) -> float | None:
    # Infer from linear index + layout if present; else use A note ~5 hPa
    # Prefer explicit level if present
    if "level_hpa" in worst:
        return float(worst["level_hpa"])
    # From samples if level_index known - CFSR isobaric levels standard set
    # A review: 5 hPa near top
    return 5.0


def main() -> int:
    grib_ls = require_grib_ls()
    ver = subprocess.run(
        [grib_ls, "-V"], capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    eccodes_version = (ver.stdout or ver.stderr or "").strip()

    files = sorted(CFSR_DIR.glob("pgbl00.gdas.20090101*.grb2"))
    if len(files) < 3:
        print("INCOMPLETE: CFSR pgbl files missing", file=sys.stderr)
        return 2

    calib = None
    if CALIB.is_file():
        calib = json.loads(CALIB.read_text(encoding="utf-8"))

    worst_entries = []
    if calib:
        for fr in calib.get("fields", []):
            if "0.1.0" not in str(fr.get("field", "")) and fr.get("field") != "grib:0.1.0:isobaric":
                if "q" not in str(fr.get("field", "")).lower():
                    # still accept if path says q via worst
                    pass
            w = fr.get("worst") or {}
            if not w:
                continue
            if "0.1.0" not in str(fr.get("field", "")) and str(w.get("field", "")).find("0.1.0") < 0:
                if fr.get("field") and "0.1.0" not in fr["field"]:
                    continue
            worst_entries.append({"field_report": fr, "worst": w})

    out_files = []
    for path in files:
        rows = grib_ls_table(grib_ls, path)
        # Attach all q rows structured
        q_msgs = []
        for r in rows:
            sn = str(r.get("shortName", ""))
            pid = str(r.get("paramId", ""))
            if sn != "q" and pid != "133":
                continue
            q_msgs.append(
                {
                    "offset": coerce_num(r.get("offset", "")),
                    "dataRepresentationTemplateNumber": coerce_num(
                        r.get("dataRepresentationTemplateNumber", "")
                    ),
                    "bitsPerValue": coerce_num(r.get("bitsPerValue", "")),
                    "referenceValue": coerce_num(r.get("referenceValue", "")),
                    "binaryScaleFactor": coerce_num(r.get("binaryScaleFactor", "")),
                    "decimalScaleFactor": coerce_num(r.get("decimalScaleFactor", "")),
                    "packingType": r.get("packingType"),
                    "paramId": coerce_num(r.get("paramId", "")),
                    "shortName": r.get("shortName"),
                    "typeOfLevel": r.get("typeOfLevel"),
                    "level": coerce_num(r.get("level", "")),
                    "count": coerce_num(r.get("count", "")),
                    "discipline": coerce_num(r.get("discipline", "")),
                    "parameterCategory": coerce_num(r.get("parameterCategory", "")),
                    "parameterNumber": coerce_num(r.get("parameterNumber", "")),
                }
            )

        # Match worst from calib for this file
        worst_pack = None
        matched_worst = None
        for ent in worst_entries:
            w = ent["worst"]
            wfile = str(w.get("file", ""))
            if wfile and wfile not in path.name and path.name not in wfile:
                # also match via field_report.file
                fr_file = str(ent["field_report"].get("file", ""))
                if path.name not in fr_file and fr_file not in path.name:
                    continue
            lev = level_from_worst(w, ent["field_report"])
            row = pick_q_message(rows, lev)
            if row:
                worst_pack = {
                    "offset": coerce_num(row.get("offset", "")),
                    "dataRepresentationTemplateNumber": coerce_num(
                        row.get("dataRepresentationTemplateNumber", "")
                    ),
                    "bitsPerValue": coerce_num(row.get("bitsPerValue", "")),
                    "referenceValue": coerce_num(row.get("referenceValue", "")),
                    "binaryScaleFactor": coerce_num(row.get("binaryScaleFactor", "")),
                    "decimalScaleFactor": coerce_num(row.get("decimalScaleFactor", "")),
                    "packingType": row.get("packingType"),
                    "paramId": coerce_num(row.get("paramId", "")),
                    "shortName": row.get("shortName"),
                    "typeOfLevel": row.get("typeOfLevel"),
                    "level": coerce_num(row.get("level", "")),
                    "matched_level_hpa_target": lev,
                    "worst_point": {
                        "linear_index": w.get("linear_index"),
                        "level_index": w.get("level_index"),
                        "y": w.get("y"),
                        "x": w.get("x"),
                        "maximum_ulps": w.get("maximum_ulps"),
                        "maximum_absolute_difference": w.get("maximum_absolute_difference"),
                        "rust_value": w.get("rust_value"),
                        "native_value": w.get("native_value"),
                        "rust_bits_hex": w.get("rust_bits_hex"),
                        "native_bits_hex": w.get("native_bits_hex"),
                        "valid_time_unix": w.get("valid_time_unix"),
                    },
                }
                matched_worst = w
                break

        out_files.append(
            {
                "file": path.name,
                "relative_path": path.relative_to(ROOT).as_posix()
                if path.is_relative_to(ROOT)
                else path.as_posix(),
                "size": path.stat().st_size,
                "sha256": sha256_file(path),
                "q_message_count": len(q_msgs),
                "q_messages": q_msgs,
                "worst_q_message_packing": worst_pack,
                "matched_worst_present": matched_worst is not None,
            }
        )

    doc = {
        "schema_hint": "trajecta.m3.cfsr_worst_q_packing/v1",
        "status": "raw_measurement_not_registry",
        "eccodes_version": eccodes_version,
        "grib_ls_path": grib_ls,
        "source_calibration": {
            "path": CALIB.relative_to(ROOT).as_posix() if CALIB.is_file() else None,
            "sha256": sha256_file(CALIB) if CALIB.is_file() else None,
        },
        "files": out_files,
        "note": "Structured packing for A v1.0.1 evidence; B does not edit registry.",
    }
    # One-way evidence chain: packing may *read* the raw ULP calibration and pin its
    # SHA, but must never rewrite that source (would invalidate the pinned digest).
    raw = (json.dumps(doc, indent=2, sort_keys=True) + "\n").encode("utf-8")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(raw)
    digest = hashlib.sha256(raw).hexdigest()
    OUT.with_suffix(".json.sha256").write_text(digest + "\n", encoding="utf-8")
    # Optional sidecar pointer only — does not mutate CALIB.
    ptr = OUT.with_name("cfsr_worst_q_packing.SOURCE_CALIBRATION.json")
    ptr.write_text(
        json.dumps(doc.get("source_calibration") or {}, indent=2) + "\n",
        encoding="utf-8",
    )
    print("wrote", OUT.as_posix(), "sha", digest[:16])
    if CALIB.is_file():
        print(
            "source_calibration_sha_pinned",
            (doc.get("source_calibration") or {}).get("sha256"),
        )
    for f in out_files:
        wp = f.get("worst_q_message_packing")
        print(
            f["file"],
            "q_msgs",
            f["q_message_count"],
            "worst_level",
            None if not wp else wp.get("level"),
            "template",
            None if not wp else wp.get("dataRepresentationTemplateNumber"),
            "decScale",
            None if not wp else wp.get("decimalScaleFactor"),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
