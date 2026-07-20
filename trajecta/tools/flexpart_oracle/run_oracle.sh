#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Three-family FLEXPART GPL oracle entrypoint (ERA5 pressure, CFSR, ERA5 hybrid).
# Does not change queries, registry, Trajecta algorithms, or FLEXPART commit.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FP_ROOT="${TRAJECTA_FLEXPART_ROOT:-$ROOT/../flexpart}"
COMMIT="dace3affa2ba71677f12f3858b04aaf59f8ee51e"
OUT="$ROOT/target/m3-oracle"
# Independent build dirs for this pressure-coordinate rework cycle.
PRESSURE_BUILD="${TRAJECTA_ORACLE_PRESSURE_BUILD_DIR:-$OUT/build/pressure-meter-pcrework}"
ETA_BUILD="${TRAJECTA_ORACLE_ETA_BUILD_DIR:-$OUT/build/eta-hybrid-pcrework}"

mkdir -p "$OUT/logs" "$OUT/artifacts" "$OUT/reports" "$OUT/work"
cd "$ROOT"

echo "FLEXPART root: $FP_ROOT"
if [[ ! -d "$FP_ROOT/.git" ]]; then
  echo "status=external_blocked reason=flexpart_checkout_missing" | tee "$OUT/STATUS.txt"
  exit 2
fi

pushd "$FP_ROOT" >/dev/null
HAVE=$(git rev-parse HEAD)
echo "HEAD=$HAVE expected=$COMMIT" | tee "$OUT/logs/commit.txt"
if [[ "$HAVE" != "$COMMIT" ]]; then
  echo "status=external_blocked reason=flexpart_commit_mismatch have=$HAVE expected=$COMMIT" | tee "$OUT/STATUS.txt"
  exit 2
fi
popd >/dev/null

if [[ -x /usr/bin/python3 ]]; then
  ORACLE_PYTHON=/usr/bin/python3
else
  ORACLE_PYTHON="${TRAJECTA_ORACLE_PYTHON:-python3}"
fi

{
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "uname=$(uname -a || true)"
  echo "python=$ORACLE_PYTHON"
  echo "gfortran=$(command -v gfortran || true)"
  gfortran --version 2>/dev/null | head -3 || true
  echo "eccodes=$(command -v grib_ls || true)"
  echo "default_real_bits_required=32"
  echo "forbid=-fdefault-real-8"
  echo "pressure_build=$PRESSURE_BUILD"
  echo "eta_build=$ETA_BUILD"
} | tee "$OUT/logs/toolchain.txt"

if ! command -v gfortran >/dev/null 2>&1; then
  echo "status=external_blocked reason=gfortran_missing" | tee "$OUT/STATUS.txt"
  exit 2
fi

echo "Cleaning canonical oracle/MIT outputs from prior cycles (queries preserved)"
rm -f \
  "$OUT/artifacts/era5_pressure_oracle.json" \
  "$OUT/artifacts/cfsr_pressure_oracle.json" \
  "$OUT/artifacts/era5_hybrid_oracle.json" \
  "$OUT/artifacts/era5_pressure_trajecta_subject.json" \
  "$OUT/artifacts/cfsr_pressure_trajecta_subject.json" \
  "$OUT/artifacts/era5_hybrid_trajecta_subject.json" \
  "$OUT/reports/era5_pressure_common_semantics.json" \
  "$OUT/reports/cfsr_pressure_common_semantics.json" \
  "$OUT/reports/era5_hybrid_common_semantics.json" \
  "$OUT/reports/era5_pressure_difference_report.json" \
  "$OUT/reports/cfsr_pressure_difference_report.json" \
  "$OUT/reports/era5_hybrid_difference_report.json" \
  "$OUT/FAMILY_ROLLUP.json" \
  "$OUT/ORACLE_RUN_SUMMARY.json" \
  "$OUT/STATUS.txt"
rm -rf \
  "$OUT/work/era5-pressure" \
  "$OUT/work/cfsr-pressure" \
  "$OUT/work/era5-hybrid" \
  "$PRESSURE_BUILD" \
  "$ETA_BUILD"
mkdir -p "$OUT/work/era5-pressure" "$OUT/work/cfsr-pressure" "$OUT/work/era5-hybrid"

echo "Building pressure-meter adapter driver (verttransform_gfs)"
env -u LD_LIBRARY_PATH TRAJECTA_ORACLE_BUILD_DIR="$PRESSURE_BUILD" \
  bash "$ROOT/tools/flexpart_oracle/build_pressure_meter_driver.sh" \
  2>&1 | tee "$OUT/logs/build_pressure_meter.log"
PM_BIN="$(tail -n 1 "$OUT/logs/build_pressure_meter.log")"

echo "Building ETA hybrid driver (verttransform_ecmwf)"
env -u LD_LIBRARY_PATH TRAJECTA_ORACLE_BUILD_DIR="$ETA_BUILD" \
  bash "$ROOT/tools/flexpart_oracle/build_eta_hybrid_driver.sh" \
  2>&1 | tee "$OUT/logs/build_eta_hybrid.log"
ETA_BIN="$(tail -n 1 "$OUT/logs/build_eta_hybrid.log")"

echo "ERA5 pressure oracle (source=era5 vertical=pressure pbl=official_prescribed BLH transform=gfs)"
env -u LD_LIBRARY_PATH "$ORACLE_PYTHON" \
  "$ROOT/tools/flexpart_oracle/run_pressure_meter_oracle.py" \
  --binary "$PM_BIN" \
  2>&1 | tee "$OUT/logs/run_era5_pressure.log"

echo "CFSR pressure oracle (source=cfsr vertical=pressure pbl=official_prescribed HPBL transform=gfs)"
env -u LD_LIBRARY_PATH "$ORACLE_PYTHON" \
  "$ROOT/tools/flexpart_oracle/run_cfsr_pressure_oracle.py" \
  --binary "$PM_BIN" \
  2>&1 | tee "$OUT/logs/run_cfsr_pressure.log"

echo "ERA5 hybrid ETA oracle (source=era5 vertical=hybrid_eta pbl=richardson_diagnosed transform=ecmwf)"
env -u LD_LIBRARY_PATH "$ORACLE_PYTHON" \
  "$ROOT/tools/flexpart_oracle/run_era5_hybrid_oracle.py" \
  --binary "$ETA_BIN" \
  2>&1 | tee "$OUT/logs/run_era5_hybrid.log"

echo "MIT three-family comparison (A registry; pressure adapter registered; hybrid exceptions explicit)"
env -u LD_LIBRARY_PATH "$ORACLE_PYTHON" \
  "$ROOT/tools/run_m3_oracle_comparison.py" \
  2>&1 | tee "$OUT/logs/run_m3_oracle_comparison.log" || true

env -u LD_LIBRARY_PATH "$ORACLE_PYTHON" - <<'PY'
import json
from pathlib import Path

root = Path("target/m3-oracle")
summary = {"families": {}, "richardson_fail_counts": {}, "identities": {}}
all_complete = True
family_logs = {
    "era5_pressure": root / "work/era5-pressure/driver.richardson_fail_count.txt",
    "cfsr_pressure": root / "work/cfsr-pressure/driver.richardson_fail_count.txt",
    "era5_hybrid": root / "work/era5-hybrid/driver.richardson_fail_count.txt",
}
expected_identity = {
    "era5_pressure": {
        "source_family": "era5",
        "vertical_coordinate": "pressure",
        "pbl_height_mode": "official_prescribed",
        "vertical_transform": "verttransform_gfs",
        "build_mode": "pressure_meter_adapter",
    },
    "cfsr_pressure": {
        "source_family": "cfsr",
        "vertical_coordinate": "pressure",
        "pbl_height_mode": "official_prescribed",
        "vertical_transform": "verttransform_gfs",
        "build_mode": "pressure_meter_adapter",
    },
    "era5_hybrid": {
        "source_family": "era5",
        "vertical_coordinate": "hybrid_eta",
        "pbl_height_mode": "richardson_diagnosed",
        "vertical_transform": "verttransform_ecmwf",
        "build_mode": "eta_hybrid",
    },
}
for fam in ["era5_pressure", "cfsr_pressure", "era5_hybrid"]:
    path = root / "artifacts" / f"{fam}_oracle.json"
    doc = json.loads(path.read_text(encoding="utf-8"))
    ok = sum(r["status"] == "ok" for r in doc["records"])
    fail_path = family_logs[fam]
    token = fail_path.read_text(encoding="utf-8").strip() if fail_path.is_file() else "missing"
    side = root / "artifacts" / f"{fam}_oracle_identity.json"
    if not side.is_file():
        raise SystemExit(f"missing identity sidecar: {side}")
    side_doc = json.loads(side.read_text(encoding="utf-8"))
    identity = {
        "source_family": side_doc.get("source_family"),
        "vertical_coordinate": side_doc.get("vertical_coordinate"),
        "pbl_height_mode": side_doc.get("pbl_height_mode"),
        "vertical_transform": side_doc.get("vertical_transform"),
        "build_mode": side_doc.get("adapter") or side_doc.get("build_mode_schema"),
        "build_mode_schema": side_doc.get("build_mode_schema"),
    }
    exp = expected_identity[fam]
    for k in ("source_family", "vertical_coordinate", "pbl_height_mode", "vertical_transform"):
        if identity.get(k) != exp.get(k):
            raise SystemExit(f"identity mismatch for {fam}.{k}: {identity.get(k)} != {exp.get(k)}")
    if identity.get("build_mode") != exp.get("build_mode") and identity.get("build_mode_schema") != (
        "eta" if fam == "era5_hybrid" else "pressure_meter"
    ):
        # adapter name in sidecar.build_mode preferred
        if identity.get("build_mode") != exp.get("build_mode"):
            raise SystemExit(f"identity mismatch for {fam}.build_mode: {identity} != {exp}")
    summary["identities"][fam] = identity
    summary["families"][fam] = {
        "status": doc["status"],
        "ok": ok,
        "records": len(doc["records"]),
        "compile_flags_head": doc["oracle"]["compile_flags"][:8],
        "preprocessor_definitions": doc["oracle"]["preprocessor_definitions"],
        "sha256_prefix": __import__("hashlib").sha256(path.read_bytes()).hexdigest()[:16],
        "richardson_fail_count": token,
        "identity": identity,
        "query_sha256": doc["input"]["query_sha256"],
    }
    summary["richardson_fail_counts"][fam] = token
    if doc["status"] != "complete" or ok != 75:
        all_complete = False

rollup = root / "FAMILY_ROLLUP.json"
if rollup.is_file():
    summary["rollup"] = json.loads(rollup.read_text(encoding="utf-8"))

out = root / "ORACLE_RUN_SUMMARY.json"
out.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(json.dumps(summary, indent=2, sort_keys=True))

bad = []
for fam, token in summary["richardson_fail_counts"].items():
    if token not in {"0", "hard_fail"}:
        bad.append((fam, token))
if bad:
    raise SystemExit(f"illegal richardson tokens (silent fallback?): {bad}")
summary["silent_richardson_fallback_used"] = False
if not all_complete:
    Path("target/m3-oracle/STATUS.txt").write_text(
        "status=partial_or_failed reason=one_or_more_families_not_complete_75 "
        "silent_richardson_fallback=0\n",
        encoding="utf-8",
    )
    out.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    raise SystemExit(3)
Path("target/m3-oracle/STATUS.txt").write_text(
    "status=oracles_complete_mit_may_fail richardson_fail_count=0 silent_fallback=0 "
    "pressure_adapter_registered=1\n",
    encoding="utf-8",
)
PY
