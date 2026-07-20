#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FP_ROOT="${TRAJECTA_FLEXPART_ROOT:-$ROOT/../flexpart}"
EXPECTED_COMMIT="dace3affa2ba71677f12f3858b04aaf59f8ee51e"
BUILD_DIR="${TRAJECTA_ORACLE_BUILD_DIR:-$ROOT/target/m3-oracle/build/eta-hybrid}"
DRIVER="$ROOT/tools/flexpart_oracle/src/eta_hybrid_oracle_driver.f90"
FC="${FC:-gfortran}"
if [[ -x /usr/bin/nf-config ]]; then
  NF_CONFIG="${NF_CONFIG:-/usr/bin/nf-config}"
else
  NF_CONFIG="${NF_CONFIG:-nf-config}"
fi

if [[ ! -d "$FP_ROOT/.git" ]]; then
  echo "FLEXPART checkout missing: $FP_ROOT" >&2
  exit 2
fi
actual_commit="$(git -C "$FP_ROOT" rev-parse HEAD)"
if [[ "$actual_commit" != "$EXPECTED_COMMIT" ]]; then
  echo "FLEXPART commit $actual_commit != $EXPECTED_COMMIT" >&2
  exit 2
fi

for command in "$FC" "$NF_CONFIG"; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command missing: $command" >&2
    exit 2
  fi
done

eccodes_mod="$(find /usr/lib /usr/local/lib -path '*/fortran/*/eccodes.mod' -print -quit 2>/dev/null || true)"
if [[ -z "$eccodes_mod" ]]; then
  echo "eccodes.mod not found; install libeccodes-dev" >&2
  exit 2
fi
eccodes_include="$(dirname "$eccodes_mod")"
netcdf_include="$($NF_CONFIG --includedir)"
netcdf_flibs="$($NF_CONFIG --flibs)"

mkdir -p "$BUILD_DIR/mod" "$BUILD_DIR/obj" "$BUILD_DIR/bin"

common_flags=(
  -O2 -g -m64 -cpp -mcmodel=large
  -UUSE_NCF -DETA -Duseomp -fopenmp
  -ffunction-sections -fdata-sections
  -J"$BUILD_DIR/mod" -I"$BUILD_DIR/mod"
  -I"$eccodes_include" -I"$netcdf_include"
)

# Persist actual compile identity for oracle JSON (full flags, including -g/-U*/-D*).
printf '%s\n' "${common_flags[@]}" >"$BUILD_DIR/compile_flags.txt"
{
  echo "FC=$FC"
  echo "flexpart_commit=$actual_commit"
  echo "USE_NCF=undef"
  echo "ETA=define"
  echo "useomp=define"
  echo "flags=${common_flags[*]}"
} >"$BUILD_DIR/compile_identity.txt"

sources=(
  par_mod.f90
  com_mod.f90
  qvsat_mod.f90
  random_mod.f90
  cbl_mod.f90
  pbl_profile_mod.f90
  point_mod.f90
  class_gribfile_mod.f90
  cmapf_mod.f90
  particle_mod.f90
  turbulence_mod.f90
  windfields_mod.f90
  interpol_mod.f90
  coord_ecmwf_mod.f90
  verttransform_mod.f90
)

objects=()
for source in "${sources[@]}"; do
  object="$BUILD_DIR/obj/${source%.f90}.o"
  "$FC" "${common_flags[@]}" -c "$FP_ROOT/src/$source" -o "$object"
  objects+=("$object")
done

oracle_calcpar_src="$ROOT/tools/flexpart_oracle/src/oracle_calcpar_mod.f90"
oracle_calcpar_obj="$BUILD_DIR/obj/oracle_calcpar_mod.o"
"$FC" "${common_flags[@]}" -c "$oracle_calcpar_src" -o "$oracle_calcpar_obj"
objects+=("$oracle_calcpar_obj")

driver_object="$BUILD_DIR/obj/eta_hybrid_oracle_driver.o"
"$FC" "${common_flags[@]}" -c "$DRIVER" -o "$driver_object"

binary="$BUILD_DIR/bin/eta_hybrid_oracle_driver"
# shellcheck disable=SC2086
"$FC" -o "$binary" "$driver_object" "${objects[@]}" \
  -fopenmp -Wl,--gc-sections -leccodes_f90 -leccodes $netcdf_flibs

# Symbol evidence for oracle provenance / runner gates.
if ! command -v nm >/dev/null 2>&1; then
  echo "nm not found" >&2
  exit 2
fi
nm -A "$binary" >"$BUILD_DIR/nm_symbols.txt" || nm "$binary" >"$BUILD_DIR/nm_symbols.txt"
{
  echo "required_check_begin"
  for sym in verttransform_ecmwf interpol_wind interpol_partoutput_val interpol_pbl oracle_calcpar; do
    if grep -q "$sym" "$BUILD_DIR/nm_symbols.txt"; then
      echo "OK $sym"
    else
      echo "MISSING $sym" >&2
      exit 2
    fi
  done
  echo "required_check_end"
} | tee "$BUILD_DIR/nm_required_check.txt"

printf '%s\n' "$binary"
