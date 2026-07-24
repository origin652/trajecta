#!/usr/bin/env bash
set -euo pipefail

: "${TRAJECTA_M4_A3_ARTIFACT_ROOT:?required}"

particles="${TRAJECTA_M4_A3_PARTICLES:-1000}"
duration="${TRAJECTA_M4_A3_DURATION_SECONDS:-600}"
families="${TRAJECTA_M4_A3_FAMILIES:-era5-pressure era5-hybrid cfsr-pressure}"
directions="${TRAJECTA_M4_A3_DIRECTIONS:-forward backward}"

for family in $families; do
    for direction in $directions; do
        export TRAJECTA_M4_A3_PARTICLES="$particles"
        export TRAJECTA_M4_A3_DURATION_SECONDS="$duration"
        export TRAJECTA_M4_A3_FAMILY="$family"
        export TRAJECTA_M4_A3_DIRECTION="$direction"
        export TRAJECTA_M4_A3_ARTIFACT_DIR="${TRAJECTA_M4_A3_ARTIFACT_ROOT}/${family}/${direction}"
        /bin/bash --noprofile --norc /mnt/e/flexpart/trajecta/tools/run_m4_a3_wsl_cell.sh
    done
done
