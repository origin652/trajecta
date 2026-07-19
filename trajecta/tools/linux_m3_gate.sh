#!/usr/bin/env bash
# Linux x86_64 M3 gates: core smoke vs A2 full.
# Core may pass independently. A2 full never reports passed when required
# pieces were skipped or blocked.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/target/m3-linux-gate"
mkdir -p "$OUT"
LOG="$OUT/gate.log"
exec > >(tee -a "$LOG") 2>&1

echo "=== Trajecta M3 Linux gate ==="
date -u +"utc=%Y-%m-%dT%H:%M:%SZ"
uname -a || true
echo "pwd=$ROOT"

# Always returns 0 so set -e does not abort the gate on expected non-zero steps.
# Exit code is recorded to exit_codes.txt.
run() {
  local name="$1"; shift
  echo "--- RUN $name: $*"
  set +e
  "$@"
  local rc=$?
  set -e
  echo "--- EXIT $name rc=$rc"
  echo "$name=$rc" >> "$OUT/exit_codes.txt"
  return 0
}

: > "$OUT/exit_codes.txt"

{
  echo "rustc=$(rustc --version 2>/dev/null || true)"
  echo "cargo=$(cargo --version 2>/dev/null || true)"
  echo "python=$(python3 --version 2>/dev/null || true)"
  echo "pkg-config=$(command -v pkg-config || true)"
  if command -v pkg-config >/dev/null 2>&1; then
    echo "netcdf_pc=$(pkg-config --modversion netcdf 2>/dev/null || echo missing)"
    echo "hdf5_pc=$(pkg-config --modversion hdf5 2>/dev/null || echo missing)"
    echo "eccodes_pc=$(pkg-config --modversion eccodes 2>/dev/null || echo missing)"
  fi
  echo "libclang=$(ldconfig -p 2>/dev/null | grep -i libclang | head -3 || true)"
  ls /usr/lib*/libclang.so* 2>/dev/null | head -5 || true
  echo "gfortran=$(command -v gfortran || true)"
  gfortran --version 2>/dev/null | head -2 || true
} | tee "$OUT/toolchain.txt"

setup_native_env() {
  if [[ -n "${NETCDF_DIR:-}" ]]; then
    export PKG_CONFIG_PATH="${NETCDF_DIR}/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export LIBRARY_PATH="${NETCDF_DIR}/lib:${LIBRARY_PATH:-}"
    export CPATH="${NETCDF_DIR}/include:${CPATH:-}"
  fi
  if [[ -n "${ECCODES_DIR:-}" ]]; then
    export PKG_CONFIG_PATH="${ECCODES_DIR}/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
    export LIBRARY_PATH="${ECCODES_DIR}/lib:${LIBRARY_PATH:-}"
    export CPATH="${ECCODES_DIR}/include:${CPATH:-}"
    export ECCODES_DEFINITION_PATH="${ECCODES_DEFINITION_PATH:-${ECCODES_DIR}/share/eccodes/definitions}"
  fi
  if [[ -z "${LIBCLANG_PATH:-}" ]]; then
    for c in /usr/lib/llvm-*/lib /usr/lib64 /usr/lib; do
      if compgen -G "$c/libclang.so*" > /dev/null; then
        export LIBCLANG_PATH="$c"
        break
      fi
    done
  fi
}
setup_native_env

HAS_NETCDF=0
HAS_ECCODES=0
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists netcdf; then HAS_NETCDF=1; fi
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists eccodes; then HAS_ECCODES=1; fi
echo "HAS_NETCDF=$HAS_NETCDF HAS_ECCODES=$HAS_ECCODES"

CORE_STATUS=0
A2_REASONS=()

run fmt cargo fmt --all -- --check
grep -q '^fmt=0$' "$OUT/exit_codes.txt" || CORE_STATUS=1
run clippy cargo clippy --offline --workspace --all-targets -- -D warnings
grep -q '^clippy=0$' "$OUT/exit_codes.txt" || CORE_STATUS=1
run tests cargo test --offline --workspace -- \
  --skip cfsr_pgbl_million_point_budget_and_threading \
  --skip cfsr_pgbl_million_point_performance_matrix
grep -q '^tests=0$' "$OUT/exit_codes.txt" || CORE_STATUS=1
run doc cargo doc --offline --workspace --no-deps
grep -q '^doc=0$' "$OUT/exit_codes.txt" || CORE_STATUS=1

export TRAJECTA_REQUIRE_REAL_MET=1

if [[ -d target/test-data/era5-cds-pressure-official/ready ]]; then
  run era5_chain cargo test --offline -p trajecta-met --test real_era5_query_chain -- --nocapture
  grep -q '^era5_chain=0$' "$OUT/exit_codes.txt" || CORE_STATUS=1
else
  echo "era5_chain=skipped_fixture_missing" >> "$OUT/exit_codes.txt"
  A2_REASONS+=("era5_fixtures_missing")
fi

BACKEND_RAN=0
if [[ "$HAS_NETCDF" -eq 1 && "$HAS_ECCODES" -eq 1 ]]; then
  run backend_cmp python3 tools/run_m3_backend_comparison.py
  BACKEND_RAN=1
  BRC=$(grep '^backend_cmp=' "$OUT/exit_codes.txt" | tail -1 | cut -d= -f2)
  echo "backend_cmp_rc=$BRC"
  # 0=all passed, 1=hard fail (e.g. CFSR 2ULP registry), 2=incomplete
  if [[ "$BRC" -eq 2 ]]; then
    A2_REASONS+=("backend_incomplete")
  fi
else
  echo "backend_cmp=external_blocked_native_libs netcdf=$HAS_NETCDF eccodes=$HAS_ECCODES" >> "$OUT/exit_codes.txt"
  A2_REASONS+=("native_libs_missing")
fi

ORACLE_RAN=0
run oracle bash tools/flexpart_oracle/run_oracle.sh
ORACLE_RAN=1
ORC=$(grep '^oracle=' "$OUT/exit_codes.txt" | tail -1 | cut -d= -f2)
echo "oracle_rc=$ORC"
if [[ "$ORC" -eq 0 ]]; then
  A2_REASONS+=("oracle_unexpected_success_without_complete_harness")
fi

MILLION_RAN=0
if [[ "${TRAJECTA_RUN_MILLION:-0}" == "1" ]]; then
  run million cargo test --offline -p trajecta-met --test real_million_point_perf -- \
    cfsr_pgbl_million_point_performance_matrix --nocapture
  MILLION_RAN=1
  MRC=$(grep '^million=' "$OUT/exit_codes.txt" | tail -1 | cut -d= -f2)
  if [[ "$MRC" -ne 0 ]]; then
    A2_REASONS+=("million_failed")
    CORE_STATUS=1
  fi
else
  echo "million=not_run_set_TRAJECTA_RUN_MILLION=1" >> "$OUT/exit_codes.txt"
  A2_REASONS+=("million_not_run")
fi

set +e
python3 - <<'PY'
import hashlib, json, os, sys
from pathlib import Path
root = Path('.')
out = Path('target/m3-linux-gate')
arts = []
for pattern in [
    'target/m3-comparison/**/*.json',
    'target/m3-oracle/**/*.json',
    'target/m3-million-point-perf.json',
    'target/m3-linux-gate/exit_codes.txt',
    'testdata/M3_*.json',
]:
    for p in root.glob(pattern):
        if p.is_file():
            h = hashlib.sha256(p.read_bytes()).hexdigest()
            arts.append({"path": p.as_posix(), "size": p.stat().st_size, "sha256": h})

schema_errs = []
try:
    from jsonschema import Draft202012Validator
except ImportError:
    schema_errs.append("jsonschema missing — no weak fallback")
    Draft202012Validator = None

if Draft202012Validator is not None:
    try:
        sch = json.loads(Path('testdata/M3_TOLERANCES.schema.json').read_text())
        inst = json.loads(Path('testdata/M3_TOLERANCES.v1.json').read_text())
        Draft202012Validator(sch).validate(inst)
        rep_schema = json.loads(Path('testdata/M3_COMPARISON_REPORT.schema.json').read_text())
        for p in Path('target/m3-comparison/reports').glob('*_backend_report.json'):
            Draft202012Validator(rep_schema).validate(json.loads(p.read_text()))
        ora_schema = json.loads(Path('testdata/M3_FLEXPART_ORACLE.schema.json').read_text())
        for p in Path('target/m3-oracle/artifacts').glob('*_oracle.json'):
            Draft202012Validator(ora_schema).validate(json.loads(p.read_text()))
    except Exception as e:
        schema_errs.append(str(e))

doc = {
    "runner": {
        "os": os.uname().sysname if hasattr(os, 'uname') else 'unknown',
        "release": os.uname().release if hasattr(os, 'uname') else '',
        "machine": os.uname().machine if hasattr(os, 'uname') else '',
    },
    "artifacts": sorted(arts, key=lambda a: a['path']),
    "schema_errors": schema_errs,
}
(out / 'ARTIFACTS.json').write_text(json.dumps(doc, indent=2)+'\n', encoding='utf-8')
print('wrote ARTIFACTS.json n=', len(arts), 'schema_errors', schema_errs)
sys.exit(1 if schema_errs else 0)
PY
SCHEMA_RC=$?
set -e
echo "schema_validate=$SCHEMA_RC" >> "$OUT/exit_codes.txt"
if [[ "$SCHEMA_RC" -ne 0 ]]; then
  CORE_STATUS=1
  A2_REASONS+=("schema_validate_failed")
fi

if [[ ${#A2_REASONS[@]} -eq 0 && "$BACKEND_RAN" -eq 1 && "$ORACLE_RAN" -eq 1 && "$MILLION_RAN" -eq 1 && "$CORE_STATUS" -eq 0 ]]; then
  A2_STATUS="incomplete"
  A2_REASONS+=("flexpart_oracle_harness_incomplete")
else
  if [[ "$HAS_NETCDF" -eq 0 || "$HAS_ECCODES" -eq 0 ]]; then
    A2_STATUS="external_blocked"
  else
    A2_STATUS="incomplete"
  fi
fi

{
  echo "core_status=$CORE_STATUS"
  echo "a2_full_status=$A2_STATUS"
  echo "a2_reasons=${A2_REASONS[*]-}"
  echo "backend_ran=$BACKEND_RAN oracle_ran=$ORACLE_RAN million_ran=$MILLION_RAN"
  echo "NOTE=core_smoke_is_not_A2_certification"
} | tee "$OUT/STATUS.txt"

echo "=== gate finished core=$CORE_STATUS a2=$A2_STATUS ==="
if [[ "$CORE_STATUS" -ne 0 ]]; then
  exit 1
fi
exit 0
