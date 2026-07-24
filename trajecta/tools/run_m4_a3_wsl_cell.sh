#!/usr/bin/env bash
set -euo pipefail

: "${TRAJECTA_M4_A3_PARTICLES:?required}"
: "${TRAJECTA_M4_A3_DURATION_SECONDS:?required}"
: "${TRAJECTA_M4_A3_FAMILY:?required}"
: "${TRAJECTA_M4_A3_DIRECTION:?required}"
: "${TRAJECTA_M4_A3_ARTIFACT_DIR:?required}"

export PATH="/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/trajecta-m4-a3-target}"
export LIBCLANG_PATH="${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}"
export TRAJECTA_REQUIRE_REAL_MET=1

cd /mnt/e/flexpart/trajecta
mkdir -p "$CARGO_TARGET_DIR" "$TRAJECTA_M4_A3_ARTIFACT_DIR"

uname -a
rustc -V
cargo -V
cargo test --offline --release -p trajecta-core --test m4_a3_real_data \
  real_stratospheric_ozone_three_families_forward_backward \
  -- --ignored --nocapture --exact
