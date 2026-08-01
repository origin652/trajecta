# Trajecta M5-A5 A execution report

Date: 2026-08-01
Technical comparison status: **complete / passed**
Release status: **complete / passed**
Prerelease version: `0.1.0-alpha.1`

## 1. Outcome

M5-A5 is complete. The FLEXPART comparison was technically accepted after the
M5-A5.5 performance closeout, and the user-approved prerelease was published
and independently revalidated from downloaded GitHub assets. The final
symmetric 600-second transport-step matrix completed 24/24 runs and 6/6
scientific comparisons.

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
docs/engineering/TRAJECTA_M5_A5_5_A_EXECUTION_REPORT.md
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

## 4. Release closeout

The user approved the informed external publication boundary on 2026-08-01.
The workspace version changed once from `0.0.0` to `0.1.0-alpha.1`; no
intermediate version, compatibility implementation, second tag, or extra
prerelease was created.

```text
release commit: 0e1406f4f2d2001fbfbf8fa30510b6f815a217bf
annotated tag:  v0.1.0-alpha.1
tag target:     0e1406f4f2d2001fbfbf8fa30510b6f815a217bf
source tree:    d27f47d7eab6de648976155fd106b6f1e2220d2e28cc6ea165739e779a226855
GitHub release: https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1
release flags:  prerelease=true, draft=false
```

Published assets:

| Platform | Archive SHA-256 | Binary SHA-256 | Build-manifest SHA-256 |
|---|---|---|---|
| Windows x86_64 | `af411fdeb866691e05c077900a1b91fe0561d7c10073f5fcf18e1bd1bb590c42` | `949d35608dd62f9f6b89b06fde73f768b843046fc7586df1e82a1c2a8f9c7369` | `ad909df37d0809751b804a7dff470905f0c0cc007ec1ecc2c58de0c8ef4a19c9` |
| Ubuntu 24.04 x86_64 | `a9a643f1a8d1ea04372a5f72d50490a007058dae98bae9445d72ce12c12945cb` | `38e8a4bbbc2b468f410d78f4a4369912f09b093f9954cf5f73348edd9b2bb58b` | `770f47afe80acba9242da88196e4e664b1a56e709a04a89680f0f3cb999dbb23` |

GitHub's asset digests match these archive identities. Both archives and both
adjacent checksum files were downloaded into a fresh directory after
publication. Verification used the downloaded bytes, not the pre-upload
paths.

Published-download verification evidence:

```text
target/m5-a5/release/published-download-attempt-1/
  M5_A5_PUBLISHED_WINDOWS_VERIFY_RESULT.json
  SHA-256 f5488ff95b3a0cc34aa6c59f1422b0e0a58de8b5ce087135b03416827029faa0

  M5_A5_PUBLISHED_LINUX_VERIFY_RESULT.json
  SHA-256 29fa1ee70344720b8c61c819e1758b6eb938448baca3b9553766e6d61f383dd6
```

Both verify results are `passed` and independently reproduce the expected
archive, binary, manifest, source-tree, payload, and native-library identities.
Windows probed ecCodes 2.47.0 / netCDF-C 4.9.3 / HDF5 1.14.6; Ubuntu probed
ecCodes 2.34.1 / netCDF-C 4.9.2 / HDF5 1.10.10.

Published-download clean-extraction smoke:

```text
Windows: target/m5-a5/release/published-windows-clean-smoke-attempt-1/
  result passed; 2/2 cells; 0 failed
  summary SHA-256 1b82c3b3418b14c19debc754d2f79abea933e0ac02ab7342a16861c1e1bc8e47

Ubuntu: target/m5-a5/release/published-linux-clean-smoke-attempt-1/
  result passed; 2/2 cells; 0 failed
  summary SHA-256 0faef6d3f0dbd7bd9d773023215ceee1204ace722cba62aa00c5d6fa70765865
```

Each platform ran the packaged Rust forward cell and native backward cell with
real CFSR data, including clean preflight, daemon/worker lifecycle, result
inspection, full verification, trajectory reading, report generation, SQLite,
WAL, provenance, and package identity gates. No Windows or Ubuntu Trajecta
process remained afterward.

Failed prepublication build attempts were retained honestly. Windows attempts
1--4 exposed sandboxed C compilation, missing native build variables, missing
libclang, and mixed DLL precedence before the frozen A4 environment produced
attempt 5. Linux attempt 1 found the absent `python` alias; attempt 2 exposed a
cross-shell empty-path expansion and its three untracked generated paths were
verified, removed, and followed by a clean-tree audit; literal-path attempt 3
passed. None of these failed attempts was published or used by smoke.

A5 is complete. M5 documentation and final user guidance remain a separate
discussion as previously agreed.

## 5. Primary evidence

```text
/root/.cache/trajecta/m5-a5.5/fair-dt600-formal-attempt-5/
  M5_A5_FLEXPART_COMPARISON.json
  SHA-256 b8d0f5befcf3bcf0c9cbf9ac70faa7ff708b676d906f6c27593d7202ab2a3b39
```

Earlier failed and diagnostic attempts remain immutable and are not release
artifacts.
