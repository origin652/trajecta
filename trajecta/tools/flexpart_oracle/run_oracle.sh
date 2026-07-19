#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build/run FLEXPART GPL oracle harness against frozen commit.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FP_ROOT="${TRAJECTA_FLEXPART_ROOT:-$ROOT/../flexpart}"
COMMIT="dace3affa2ba71677f12f3858b04aaf59f8ee51e"
OUT="$ROOT/target/m3-oracle"
mkdir -p "$OUT/logs" "$OUT/artifacts"

echo "FLEXPART root: $FP_ROOT"
if [[ ! -d "$FP_ROOT/.git" ]]; then
  echo "status=external_blocked reason=flexpart_checkout_missing" | tee "$OUT/STATUS.txt"
  exit 2
fi

pushd "$FP_ROOT" >/dev/null
HAVE=$(git rev-parse HEAD)
echo "HEAD=$HAVE expected=$COMMIT" | tee "$OUT/logs/commit.txt"
if [[ "$HAVE" != "$COMMIT" ]]; then
  echo "Attempting checkout $COMMIT"
  git fetch --all --tags || true
  if ! git checkout "$COMMIT"; then
    echo "status=external_blocked reason=flexpart_commit_unavailable" | tee "$OUT/STATUS.txt"
    exit 2
  fi
fi
popd >/dev/null

# Queries: prefer terrain-selected formal path; skeleton only with flag.
set +e
python3 "$ROOT/tools/flexpart_oracle/generate_queries.py" 2>&1 | tee "$OUT/logs/generate_queries.log"
QRC=${PIPESTATUS[0]}
set -e
if [[ "$QRC" -ne 0 ]]; then
  echo "formal terrain queries unavailable; writing skeleton under queries-skeleton/" | tee -a "$OUT/logs/generate_queries.log"
  python3 "$ROOT/tools/flexpart_oracle/generate_queries.py" --skeleton-ok 2>&1 | tee -a "$OUT/logs/generate_queries.log"
fi

{
  echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "uname=$(uname -a || true)"
  echo "gfortran=$(command -v gfortran || true)"
  gfortran --version 2>/dev/null | head -3 || true
  echo "eccodes=$(command -v grib_ls || true)"
  echo "default_real_bits_required=32"
  echo "forbid=-fdefault-real-8"
} | tee "$OUT/logs/toolchain.txt"

# Environment probe: schema-valid failed oracle ONLY when real input identity exists.
# Otherwise write STATUS/logs only — never forge empty files[] oracle JSON.
if ! command -v gfortran >/dev/null 2>&1; then
  echo "status=external_blocked reason=gfortran_missing" | tee "$OUT/STATUS.txt"
  python3 "$ROOT/tools/flexpart_oracle/generate_oracle_stub.py" --reason gfortran_missing || true
  exit 2
fi

echo "status=incomplete reason=harness_fortran_driver_not_yet_linked_to_flexpart_routines" | tee "$OUT/STATUS.txt"
echo "Next: implement GPL driver under tools/flexpart_oracle calling interpol_wind/interpol_partoutput_val."
# Still emit schema-valid failed probes when inputs exist (non-zero).
python3 "$ROOT/tools/flexpart_oracle/generate_oracle_stub.py" --reason harness_incomplete || true
exit 3
