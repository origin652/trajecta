# Trajecta M5-A5 A execution report

Date: 2026-08-01
Technical comparison status: **complete / passed**
Release status: **user-approved; version transition complete, publication pending**
Prerelease version: `0.1.0-alpha.1`

## 1. Outcome

The M5-A5 FLEXPART comparison is technically accepted after the M5-A5.5
performance closeout. The final symmetric 600-second transport-step matrix
completed 24/24 runs and 6/6 scientific comparisons.

```text
10k: 0.465415032 / 1.330 = 0.34993611428571425  passed
50k: 1.469328604 / 1.340 = 1.0965138835820896   passed
limit: <= 1.5
```

All lifecycle, mass, finite-value, SQLite, WAL, bundle, package, daemon-idle,
input-identity, scientific-coverage, and determinism gates passed. The passing
aggregate and five publication charts are complete.

Detailed implementation and evidence are in:

```text
docs/TRAJECTA_M5_A5_5_A_EXECUTION_REPORT.md
```

## 2. Completed A5 work

- fixed FLEXPART 11.1 commit, executable, patch, native, and species identities;
- exact/aligned official ERA5 hybrid-137 meteorology equivalence audit;
- one clean Ubuntu 24.04 package and four-CPU execution contract;
- 10k and 50k warm-ups plus three rotated formal repetitions;
- equivalent-core and complete-product timings, RSS, throughput, and scaling;
- six aggregate scientific comparisons at common physical output times;
- canonical SQL and explicit path-normalized provenance determinism;
- deterministic timing/science CSV data and five SVG charts;
- full Rust workspace, clippy, doc, formatting, M4/M5, and A5 harness gates.

## 3. Performance interpretation

At 50k, Trajecta's transport core is only about `9.65%` slower than FLEXPART
on the frozen host and comfortably passes the `1.5x` gate. At 10k, Trajecta's
core is faster.

The full 50k Trajecta product takes `8.680 s` versus `1.370 s` for FLEXPART's
particle NetCDF. This is not an equal product comparison: Trajecta also creates
and independently verifies SQLite lifecycle/science state and a content-
addressed provenance bundle. It still simulates one hour in about 8.68 seconds.
The difference is published rather than hidden, but it is not substituted for
the equivalent-core acceptance gate.

## 4. What remains in A5

The user approved the release boundary on 2026-08-01. The workspace version
transition to `0.1.0-alpha.1` and its source gates are complete. The remaining
execution steps are:

1. create one tag and one GitHub Prerelease;
2. download the published artifact and run clean-extraction release smoke.

M5 documentation/final user guidance remains a separate final discussion as
previously agreed.

## 5. Primary evidence

```text
/root/.cache/trajecta/m5-a5.5/fair-dt600-formal-attempt-5/
  M5_A5_FLEXPART_COMPARISON.json
  SHA-256 b8d0f5befcf3bcf0c9cbf9ac70faa7ff708b676d906f6c27593d7202ab2a3b39
```

Earlier failed and diagnostic attempts remain immutable and are not release
artifacts.
