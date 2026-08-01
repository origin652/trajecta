# Trajecta M5-A5.5 performance optimization plan

Date: 2026-08-01
Status: **executed / passed**
Version boundary: `0.0.0`

## 1. Objective and result

M5-A5.5 closes the M5-A5 equivalent-core performance failure while preserving
the product, lifecycle, mass, package, and evidence boundaries. The mandatory
Trajecta/FLEXPART median ratio is at most `1.5` at both 10k and 50k.

Final result:

```text
10k ratio: 0.34993611428571425  passed
50k ratio: 1.0965138835820896   passed
```

## 2. Contract amendment

The original 300-second contract failed at 50k. The user later authorized a
controlled numerical change and appropriate accuracy tradeoff. The final
comparison uses a 600-second transport step and 1200-second output interval in
both programs.

For FLEXPART with `CTL < 0`, `LSYNCTIME` is the particle transport step.
FLEXPART requires the output interval and averaging interval to be at least
twice that step. The harness therefore rejects 600/600 and accepts 600/1200.
This amendment supersedes the original fixed 300/600 comparison clause.

The output interval and transport step remain ordinary user configuration;
this work does not introduce a second numerical-model version or compatibility
mode.

## 3. Single implementation rule

Every optimized hot path replaces its predecessor in place. The implementation
does not retain a hidden high-accuracy fallback, `_v2` copy, runtime switch, or
two independently maintained algorithms.

Continuous boundary handling has two required branches within one algorithm:

1. a conservative certificate accepts paths proven safely inside all relevant
   envelopes; and
2. unresolved near-boundary paths receive exact continuous intersection work.

The exact branch is boundary semantics, not failure-driven fallback.

## 4. Implemented work

- removed repeated complete hybrid-column construction and sampled only the
  required vertical bracket/support levels;
- grouped common RK2 transport times and reused prepared interpolation support;
- added conservative same-cell/short-arc boundary certification and a dense
  cell-envelope cache;
- removed redundant release-population validation and completed-release scans;
- added direct equal-area sampling for non-dateline axis-aligned rectangles;
- stopped job-control SQLite polling between configured monitor intervals;
- batched fine-grained performance observations without changing exact counts;
- retained sorted stable output IDs and exact residual boundary handling.

## 5. Correctness and evidence gates

Every final run requires:

- `RunOutcome::Complete`, complete manifest, and zero abnormal termination;
- exact lifecycle ordering and finite active coordinates;
- frozen mass-ledger and quality gates;
- SQLite integrity and terminal WAL truncation;
- full provenance-bundle semantic verification;
- no execute-phase source I/O;
- stable canonical SQL across repeats;
- stable A5 path-normalized provenance and canonical-output digests;
- complete input, package, and host identity evidence.

Cross-model centroid, height, dispersion, and occupancy differences are
reported at every common time. Because FLEXPART and Trajecta use different
source-sampling RNGs and integrators, these ensemble differences are not
silently promoted to particle-paired hard tolerances.

## 6. Validation ladder

1. target module tests;
2. workspace tests, clippy, docs, formatting, validators, and diff check;
3. packaged 1k and focused 10k/50k checks during optimization;
4. three rotated formal repetitions at each particle count;
5. six scientific comparisons;
6. schema-valid aggregate, raw CSV data, and five deterministic charts;
7. A review and written acceptance report.

Failed attempts are preserved and never overwritten. The passing attempt is
`fair-dt600-formal-attempt-5`; earlier attempts remain diagnostic evidence.
