# Trajecta M5-A5.5 A execution report

Date: 2026-08-01
Execution status: **complete**
Acceptance status: **passed**
Release status: development version remains `0.0.0`

## 1. Executive result

M5-A5.5 is accepted. The final 10,000/50,000-particle comparison completed all
24 execution records, all six scientific comparisons, and every lifecycle,
quality, mass, SQLite, provenance, determinism, and performance gate.

| Particles | FLEXPART core median | Trajecta equivalent-core median | Ratio | `<= 1.5` |
|---:|---:|---:|---:|---|
| 10,000 | `1.330 s` | `0.465415032 s` | `0.34993611428571425` | passed |
| 50,000 | `1.340 s` | `1.469328604 s` | `1.0965138835820896` | passed |

The original 10k Trajecta result was `32.279993226 s`; the final result is
`69.36x` faster. The earlier optimized 50k formal median was `6.109832836 s`;
the final 50k median is another `4.16x` faster. The nonlinear 50k gap that
reopened A5.5 is closed.

## 2. Final comparison contract

The user explicitly authorized a controlled numerical-contract change after
the frozen 300-second comparison failed to scale. The final symmetric contract
is:

```text
simulation:       3600 s
transport step:    600 s in both programs
output interval:  1200 s in both programs
particles:        10k and 50k
workers/threads:  4 on CPU set 0-3
formal repeats:   3, with rotated execution order
```

This is not based on treating an unrelated FLEXPART field as a time step.
FLEXPART 11.1 uses `LSYNCTIME` for particle transport when `CTL < 0`, as in
this advection-only case. FLEXPART also requires `LOUTSTEP` and `LOUTAVER` to
be at least twice `LSYNCTIME`, so a 600-second transport step requires the
1200-second common output interval. The harness validates this relationship
and rejects an invalid 600/600 configuration.

There is one production numerical path. The former hot implementations were
replaced in place; no `_v2`, compatibility switch, hidden accuracy fallback,
or old slow path remains.

## 3. Performance result

| Measurement | 10k | 50k |
|---|---:|---:|
| FLEXPART core median | `1.330 s` | `1.340 s` |
| Trajecta equivalent-core median | `0.465415032 s` | `1.469328604 s` |
| FLEXPART particle-NetCDF product median | `1.310 s` | `1.370 s` |
| Trajecta SQLite/provenance product median | `2.194965529 s` | `8.680348905 s` |
| FLEXPART core throughput | `7,519 particles/s` | `37,313 particles/s` |
| Trajecta core throughput | `21,486 particles/s` | `34,029 particles/s` |
| FLEXPART core median RSS | `371,175,424 B` | `379,322,368 B` |
| Trajecta product median RSS | `88,064,000 B` | `171,012,096 B` |

The 50k Trajecta product still writes and independently verifies a SQLite
scientific database and a content-addressed provenance bundle. FLEXPART's
comparison product is a particle NetCDF without an equivalent audit product,
so complete-product wall time remains report-only rather than replacing the
equivalent-core gate. The 50k Trajecta product completes a one-hour simulation
in about 8.68 seconds, roughly 415 times faster than real time.

The final 50k median stage attribution is:

```text
runner total:                         8.680 s
runner output:                        7.180 s
  output finish:                      5.110 s
    SQLite terminal audit:            2.060 s
    provenance bundle finalization:   1.760 s
    canonical SQL digest:             1.220 s
  output sink writes:                 1.630 s
runner integrator:                    0.810 s
runner boundary:                      0.220 s
runner population:                    0.100 s
```

These inclusive stages are not additive across nesting. They show that the
remaining product difference is audit/output work, not an unresolved transport
core bottleneck.

## 4. Scientific comparison

All six reports passed with complete finite ensembles and aligned common output
times. Across the three common times, median cross-model ranges are:

| Particles | Horizontal centroid separation | Vertical centroid difference | Dispersion ratio | Occupancy ratio |
|---:|---:|---:|---:|---:|
| 10k | `214.89–276.55 m` | `-49.79–-22.46 m` | `1.00116–1.00373` | `0.98148–1.01802` |
| 50k | `60.66–271.88 m` | `-46.43–-19.29 m` | `1.00288–1.00514` | `0.97521–0.99130` |

These are aggregate comparisons between different source-sampling RNGs and
integrators, not a particle-ID crosswalk. Lifecycle, mass, finite values,
input identity, coverage, and internal quality are hard gates; cross-model
ensemble differences remain report-only as specified by A5.

## 5. Determinism and provenance

The production `trajecta.provenance-content/v1` digest deliberately retains
forensic source paths. Each formal repetition lives under a different project
root, so its raw content and derived canonical digest correctly differ even
though the source bytes are identical. Canonical SQL digests are identical in
all three repetitions at each particle count.

The A5 harness now computes a separate, explicitly named test digest:

```text
trajecta.m5-a5-path-normalized-provenance/v1
```

It replaces only the prepared data-root prefix with `dataset://` and then
rehashes every provenance record, field set, and sample assignment. Absolute
sources outside the prepared data root are rejected. It does not modify the
production bundle or production v1 digest. For both 10k and 50k, the path-
normalized provenance and canonical-output digests match across all three
formal repetitions.

## 6. Final evidence

Passing aggregate:

```text
/root/.cache/trajecta/m5-a5.5/fair-dt600-formal-attempt-5/
  M5_A5_FLEXPART_COMPARISON.json
  SHA-256 b8d0f5befcf3bcf0c9cbf9ac70faa7ff708b676d906f6c27593d7202ab2a3b39
```

Executed Trajecta package:

```text
/root/.cache/trajecta/m5-a5.5/batched-boundary-metrics-package-attempt-1/
source-tree SHA-256 5fe6b857c1c59072b1eae3603fe6a51b77f01171481cdbe1e8313cba74e225c1
binary SHA-256      266a3c31dfb98e5206ee3e1c1d885d207b667a0ecb9f6e6caee438435a81b24f
archive SHA-256     c13db358baca1a6c03ad6d0a15fcc22fa81cb5f9ea9e2fe9968e5b6dd90bf21e
manifest SHA-256    7a69d06935ab03219bfc6b3939c5af0cd07ecec22efabda3fa0b9fedc1814bd8
```

FLEXPART identity:

```text
commit:        dace3affa2ba71677f12f3858b04aaf59f8ee51e
binary SHA-256 5b6aacf1ac653c26dca2fd14e498d5e705bfdf7a38896a789869c3c11d24c629
```

The aggregate contains deterministic timing/science CSV files and five SVG
charts. Important SHA-256 values are:

```text
timings CSV:      f666ddf1d8f1c23e270e6b384f8c00254d2134b00e0a47437a86ac2d13ca8965
science CSV:      5ac0bb8d1797cbcde54ca33ed16ab788b51130c8b95537e384ab60de6b15c3f0
core chart:       c55e708a721dfd26cd1ec6fb2f732a9d60a33a21a83bc8c4558511240787b18b
product chart:    a8e44765a71badd2e71ae60514e4655ffa0523a17d8213dab85f456fdcffb954
science chart:    aef032e936fff5a83a8a767b89ceee050fa0e26fd630897def8f207767774ab9
```

Attempts 1–4 remain preserved. They record, respectively, earlier performance
failure, schema closeout failure, raw-path digest failure, and the rejected
shared-absolute-path experiment. No failed attempt was overwritten or relabeled
as passed.

## 7. Source gates

The final source passed:

```text
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
python -m py_compile <A5 tools>
python tools/test_m5_a5_flexpart_comparison.py   (13 passed)
git diff --check
```

The version remains `0.0.0`. No tag, GitHub Prerelease, or release-version
change is part of M5-A5.5 acceptance.
