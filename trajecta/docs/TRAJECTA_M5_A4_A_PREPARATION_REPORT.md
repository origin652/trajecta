# Trajecta M5-A4 A Preparation Report

**Status: superseded by the final A-owned execution recorded in the legacy-named
`TRAJECTA_M5_A4_B_EXECUTION_REPORT.md`. Both formal packages, both clean smokes,
all 60 cells, and the 30-pair cross-platform audit passed. M5-A4 is complete;
M5 overall is not complete.**

## 1. Source and boundary

- Base commit: `fc4d7373670660817d594c21045432d05712ffe8` on `main`.
- Initial A-side preflight package source identity:
  `00386802b784a20adf976b527fb191ae9f77ffe3c93283f8c1f9886886f27117`.
- The package identity intentionally records the exact working-tree bytes at build
  time. The early attempt-10 identity is historical. The final Windows and Ubuntu
  24.04 formal packages share source identity
  `e4640fb83772237fb609ec307a3780ad6a63910ad89b6722489e4f08d3bbf75f`.
- The user reassigned the remaining M5-A4 package, smoke, matrix, and
  cross-platform work to A. The B prompt is retained only as historical procedure
  and was not used for delegation.
- All crate versions remain `0.0.0`. No tag, release, commit, or push was made.
- No numerical core, interpolation, integration, boundary, population, tolerance,
  abnormal classification, M4 evidence, or deletion semantic was changed.
- The outer `../origo-validation-v1.json` was not read, modified, packaged, or
  submitted.
- No subagent was used.

## 2. A-owned implementation

### Product packaging and supply chain

- Added one deterministic host-native package builder/verifier:
  `tools/m5_a4_package.py`.
- Added safe ZIP/tar inspection, single-root extraction, normalized members,
  payload size/SHA verification, deterministic archive ordering, adjacent archive
  checksum, CycloneDX 1.5 SBOM, third-party license inventory, native runtime
  closure, examples, license, and build manifest.
- The package build and archive verifier now call public native library APIs and
  hard-fail if declared versions differ from actual runtime bytes:
  - `codes_get_api_version`;
  - `nc_inq_libvers`;
  - `H5get_libversion`.
- This probe caught and rejected a real false-positive build in which build-script
  discovery reported netCDF-C 4.9.3 / HDF5 1.14.6 but linker ordering selected
  netCDF-C 4.10.1 / HDF5 2.1.0 DLLs.

### Product matrix harness

- Added `tools/run_m5_a4_product_matrix.py` with a frozen 30-cell order per
  platform and 60 cells across Windows/Linux:
  - Rust reader 1k: 18;
  - Rust reader 10k: 6;
  - native reader 1k: 6.
- Every cell uses an isolated config, catalog, project, lock, run root, and
  `attempt-1`; existing roots are never overwritten.
- The runner stops on the first failed cell, does not retry or modify parameters,
  and preserves all command stdout/stderr and product artifacts.
- Per-cell gates cover Complete/manifest identity, abnormal=0, lifecycle, mass,
  quality, population model, exact reader backend, SQLite integrity/counts/WAL,
  provenance/digests, full verification, trajectory, deterministic report,
  catalog identity, daemon idle/restart, and completed-job non-rerun.
- Windows extended-length paths are passed to SQLite as native filesystem paths
  with `PRAGMA query_only=ON`; they are not misparsed as file-URI authorities.
- CFSR manifest identity matches the exact DatasetLock selected by each cell. A
  smoke lock may contain three frames and a formal lock may contain four. Runtime
  provenance remains direction-aware: forward uses 00/06/12 and backward uses
  06/12/18 for the formal CFSR window.

### CLI and contracts

- Human/JSON `--help` exits 0; an empty command remains usage exit 2.
- Added strict build-manifest/product-cell schemas and examples.
- `tools/validate_m5_a0_contracts.py` now freezes 30 cells per platform, 60 total,
  exact phase counts, Rust/native coverage, four-frame fixture shape, and distinct
  forward/backward starts.

## 3. Frozen M5-A4 four-frame fixtures

These are A4-only local fixtures under `target/m5-a4/fixtures`; existing M3/M4
fixtures were not modified.

### ERA5 pressure, ready/

```text
era5_pressure_20181201.nc
  9d1b2a64aa01acd950b091ac83cc69bbdcc43b5e6c0eba478787703828db167e
era5_surface_20181201.nc
  0f438e28a084b313939fdde0ee5e91ebf218d7fbf0a9d12c0552135eec872969
times: 00 / 06 / 12 / 18 UTC
forward start: 06 UTC
backward start: 12 UTC
```

### ERA5 hybrid, ready/

```text
era5_hybrid137_prepared_20181201.nc
  eb8f8554c2ed87e0a49d8a7e5ddf52b7337c96461db7c650a8d890e3f400e0e6
era5_surface_20181201.nc
  3c21ad12b54e559331710ada547d4fed6cdf8ece1450d7c92ec364691b51a730
times: 00 / 03 / 06 / 09 UTC
forward start: 03 UTC
backward start: 06 UTC
```

### CFSR pressure

```text
pgbl00.gdas.2009010100.grb2
  fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c
pgbl00.gdas.2009010106.grb2
  00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b
pgbl00.gdas.2009010112.grb2
  f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5
pgbl00.gdas.2009010118.grb2
  a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade
forward start: 06 UTC
backward start: 12 UTC
```

## 4. Final Windows A-side package evidence

Artifact root:

```text
target/m5-a4/a-preflight/windows-package-attempt-10/
```

```text
archive SHA-256:
  21b177d495cb92e3fef9d53d5b48b63e119a021e63104582216d6a3efdba38cf
binary SHA-256:
  a7bdda98edb1464f83a1d059bff6917839d6bd6b125485c1cba4a39aa672f4d7
BUILD-MANIFEST SHA-256:
  c55da0e51f3f529868490503b8333ac73ad37189d523193d5b6f3442123ee90d
source-tree SHA-256:
  00386802b784a20adf976b527fb191ae9f77ffe3c93283f8c1f9886886f27117
native API probe:
  ecCodes 2.47.0 / netCDF-C 4.9.3 / HDF5 1.14.6
```

Build-result SHA-256:
`379038804cb1b1c6053cf3a2f1b27fe860a0190bb726b45d114aa6f07fd2cc42`.

Verify-result SHA-256:
`89f825c2256e96ad64514e3dba15011ddeb66b62102e1d77165ad6a56240c3b0`.

The clean extraction is outside the source tree at:

```text
E:/flexpart/m5-a4-extract-a-preflight-10/trajecta-0.0.0-windows-x86_64/
```

## 5. Product execution evidence

Final clean-package smoke:

```text
target/m5-a4/a-final-clean-smoke-2/M5_A4_PRODUCT_MATRIX_SUMMARY.json
SHA-256 05997ea66a7e8a754dd7643d080d65037db11d492e73883121ee97110e70924b
result: passed, 2/2
Rust forward CFSR 1k:   461 ms
native backward CFSR 1k: 426 ms
```

Both cells passed every hard gate, including foreground default, detached wait,
daemon idle/restart, completed-job non-rerun, native backend selection, full verify,
SQLite/WAL, provenance, trajectory, report, catalog, and abnormal=0.

Final formal-duration preflight:

```text
target/m5-a4/a-final-formal-preflight-2/M5_A4_PRODUCT_MATRIX_SUMMARY.json
SHA-256 268502ca9afba6c0b2665d67a013859151f8bcdae4b371b5b3502fb82261cf3e

cell: Windows / ERA5 pressure / release / forward / Rust / 1k / 600 s
cell result SHA-256:
  c15e1de9ad1bad045bcb78f3002b95b9c3017353fbd84c36965b8b45ae1f7329
result: passed
runner: 1,111 ms
abnormal: 0
```

## 6. Final local gates

```text
cargo test --offline --workspace
  passed; 541 passed / 11 ignored (552 listed)

cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/test_m5_a4_package.py
  6 passed
python tools/test_m5_a4_product_matrix.py
  6 passed
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
python -m py_compile <A4 and ERA5 fixture tools>
git diff --check
  passed
```

## 7. Preserved failed attempts

- attempt-5: MinGW runtime path ordering caused the C compiler probe to fail.
- attempt-6: direct use of the current Conda HDF5 2.1.0 prefix was rejected by
  `hdf5-metno-sys`.
- attempt-7: build completed, but independent API probes proved the manifest's
  netCDF/HDF5 versions did not match the packaged DLLs.
- attempt-8: corrected link ordering and runtime bytes; used to validate the probe.
- attempt-9: intermediate probe-enabled package used before the final source-tree
  identity correction.
- attempt-10: final A-side package after fixing NUL-delimited dirty-path identity;
  manifest paths are source-root relative and untracked directories are expanded.

No failed artifact was overwritten or relabeled as passed.

## 8. Historical exit conditions — closed

The earlier open conditions were a fresh Windows package/smoke/30-cell matrix, a
fresh Ubuntu 24.04 package/smoke/30-cell matrix, and explicit A adjudication of
all cross-platform digest differences. They are now closed by:

```text
Windows package: target/m5-a4/a-review/windows-package-attempt-17/
Windows smoke:   target/m5-a4/a-review/windows-clean-smoke-attempt-17/
Windows matrix:  target/m5-a4/a-review/windows-matrix-attempt-17/
Linux package:   target/m5-a4/a-review/linux-package-attempt-4/
Linux smoke:     target/m5-a4/a-review/linux-clean-smoke-attempt-5/
Linux matrix:    target/m5-a4/a-review/linux-matrix-attempt-5/
Cross-platform:  target/m5-a4/a-review/cross-platform-attempt-1/
```

M5-A4 is complete. M5 overall remains incomplete because later M5 stages are not
part of this report.

## 9. B first-failure A review closeout

B's first Windows formal matrix stopped at the third cell:

```text
windows-x86_64__era5-pressure__air-mass__forward__rust__p1000__w1
```

The packaged run completed with two birth-time `invalid_meteorology`
terminations. Both points had valid thermodynamic columns. At the exact 06 UTC
frame they occupied the bottom pressure interval, whose complete geometric-W
support was unavailable. Endpoint-time sampling already used the established
surface-layer bridge there, but the exact-frame transport path did not. A stale
surface helper precheck also compared against the structural thermodynamic
lowest level instead of the lowest complete transport anchor.

The A-owned correction is deliberately narrow:

- exact/between-frame upper transport falls back to the existing surface-layer
  route only for incomplete support in the structural bottom bracket;
- incomplete support above the bottom bracket remains a hard failure;
- the redundant structural-lowest precheck was removed;
- explain output records `surface_layer` for this route;
- no tolerance, source data, anomaly classification, schema, cap, version, or
  population rule changed.

Regressions:

```text
query::engine::tests::exact_bottom_interval_uses_surface_when_transport_anchor_is_incomplete
real_era5_pressure_exact_bottom_transport_replay
```

The corrected product run exposed a separate harness-contract error. Normal
`population_outflow` terminal states retain known provenance quality while the
five meteorology values are absent. The original A4 runner incorrectly required
every aggregate validity bucket to be `ok`. The quality gate now requires exact
wind/pressure/temperature coverage of every state, accepts only coherent `ok`
or normal-terminal `missing` buckets, and accepts only `source | derived` quality.
`estimated`, unknown validity/quality, duplicate buckets, inconsistent missing
counts, and uncovered states remain failures. A Python regression freezes the
observed mixed source/derived surface-layer case.

## 10. Fresh A review package and failed-cell replay

Fresh Windows package:

```text
target/m5-a4/a-review/windows-package-attempt-12/

archive SHA-256:
  1c2afb2a7856f5e1401a66e8f3562b5ef4a474fdb0e915580f91d076a84f314d
binary SHA-256:
  3608abdc94c44c455178e1e2f637fdc2156ff1ac69261dd64acb610f31f18c86
BUILD-MANIFEST SHA-256:
  7469aec60c94cb4b0bd98312e6a3dc76e3af17db98a72190f73bc2351cac53ca
source-tree SHA-256:
  db5b512f8dd879796b3a593ccf51b64c07107019c9c53b9639f8a2e93d7399fa
```

Build and verify probes both reported exactly:

```text
ecCodes  2.47.0
netCDF-C 4.9.3
HDF5     1.14.6
```

The original failed cell was replayed once from the clean extraction under:

```text
target/m5-a4/a-review/failed-cell-attempt-2/
```

Result:

```text
matrix summary SHA-256:
  f2ac0a8ff3f68e1cf01751327800462aa9f015413f149e0c115030597840fd96
cell summary SHA-256:
  775259800a1999a888bca584eff10e5879f6b84a92a5ebb1d2ab12f4547c2d36

result: passed
manifest/job: complete
abnormal_count: 0
normal population_outflow: 19
particle/state rows: 1,000 / 2,000
runner: 1,371 ms
full verification / SQLite / terminal WAL / provenance / report / quality: passed
```

All final local gates passed, including workspace tests, workspace clippy with
`-D warnings`, doc, fmt, both A4 Python suites (6 package and 7 matrix tests),
M4/M5 validators, Python compilation, and `git diff --check`.

The final workspace gate also exposed and closed an independent A3 report
finalization race. A force-cancel request can outlive a short daemon idle window;
after returning the legal interrupted snapshot, the daemon may service an already
queued shutdown before the CLI asks for attempt-history metadata. Previously this
left a valid interrupted manifest without the automatic `run-report.md`. The CLI
now falls back to the terminal snapshot and manifest when that catalog lookup is
unavailable, writes the deterministic report, and retains `report.refresh_failed`
as a structured warning about the omitted catalog enrichment. It does not restart
the job or alter lifecycle state. Both the deterministic fallback regression and
the real CFSR daemon/worker/force-cancel contract pass.

At that historical stop, the package proved the executable correction and the
original-cell closeout but was not reusable as final evidence: a subsequent
test-only clippy cleanup, the terminal-report race fix, and report/prompt updates
changed source-tree identity. The then-required next step was a fresh package and
a full Windows restart before Linux. Section 13 records the later completion.

## 11. B ozone preflight failure and A closeout

B's post-review Windows matrix passed the first four cells and stopped at:

```text
windows-x86_64__era5-pressure__ozone__forward__rust__p1000__w1
```

The failure occurred in packaged `project finalize`, before numerical execution.
The profile's `case_path` correctly selected `cases/cell.json`; the mismatch
diagnostic was secondary. The generated ozone Case selected:

```text
ozone_substance: ozone
```

but declared an empty `substances` array. Case cross-component validation
therefore emitted `case.population.ozone_substance_unknown`; the invalid Case was
not inserted into `resolved_cases`, which then produced
`project.profile_case_mismatch` for the otherwise-correct path.

The A-owned correction changes only the A4 matrix fixture generator:

```json
{"id":"ozone","display_name":"Ozone"}
```

is now declared for ozone cells. Release and air-mass Cases are unchanged. The
Python regression freezes both the exact declaration and the equality between
`particle_population.ozone_substance` and the declared substance ID. No Case
schema, numerical core, population algorithm, data, tolerance, cap, version, or
runtime error classification changed.

Fresh Windows review package:

```text
target/m5-a4/a-review/windows-package-attempt-13/

archive SHA-256:
  0f84df068e16e0ad0c6c6d4543d6f1cac229d59292ee1806d2c436bb3eef88a6
binary SHA-256:
  0a836b48e72938733e5bdcb01d25a6968f7d81638276be0fff8d27eb35857148
BUILD-MANIFEST SHA-256:
  9752a4214e296c57950b6d526f2839ef553e0e198c354f41dfdae9fd86df62ce
source-tree SHA-256:
  3e06846fd7119b2b5446dc1c7d75ccf1ec8ab118623181898a68c0ca4b3f3b42
```

Build and verify independently reported:

```text
ecCodes  2.47.0
netCDF-C 4.9.3
HDF5     1.14.6
```

The failed ozone cell was replayed once from the clean extraction under:

```text
target/m5-a4/a-review/ozone-case-closeout-attempt-1/
```

Result:

```text
matrix summary SHA-256:
  d504dd136b4c279e35e3c49dd40dce0c686dddb586ed9776bccd0f7943a7e2b3
cell summary SHA-256:
  064aedc6112a2cae178a03877e033fea9cbc1625633d02ed0769f606e7c4e9e2

result: passed
manifest/job: complete
abnormal_count: 0
normal population_outflow: 32
particle/state rows: 1,000 / 2,000
runner: 1,468 ms
project finalize / lifecycle / mass / quality / full verification: passed
SQLite integrity/counts / terminal WAL / provenance / trajectory/report: passed
```

All source gates passed after the correction: workspace tests, workspace clippy
with `-D warnings`, doc, fmt, the 6 package tests and 7 matrix tests, Python
compilation, M4/M5 validators, and `git diff --check`.

Attempt-13 proved the corrected ozone Case and executable product path only. At
that historical stop, report/prompt changes required a fresh package under the
`post-ozone-closeout` roots and a Windows restart from cell 1 before Linux.
Section 13 records the later completion.

## 12. B CFSR input-identity failure and A closeout

B's post-ozone Windows matrix passed its first twelve cells and stopped at:

```text
windows-x86_64__cfsr-pressure__release__forward__rust__p1000__w1
```

The product run itself completed with all numerical, lifecycle, quality, SQLite,
provenance, trajectory, and report gates passing. Only the A4 harness
`input_identity` check failed. It expected the forward directional frame subset
00/06/12 in `manifest.inputs.dataset_content_sha256`, while that formal cell's
DatasetLock correctly froze four files, including 18 UTC.

A adjudicated the two identities separately:

- `dataset_content_sha256` is the immutable content identity of every logical
  file frozen by the selected lock. It is not globally fixed at four frames: the
  smoke lock may contain three, while the formal CFSR lock in this closeout
  contained four.
- provenance record sources identify the frames that actually entered the
  numerical path. Forward must use exactly 00/06/12; backward must use exactly
  06/12/18.

The production manifest and loader were therefore not changed. The A4 harness now:

- compares manifest content identity against the exact selected lock inventory;
- adds the required boolean `provenance_input_usage` check;
- scans the provenance bundle in bounded chunks for known fixture basenames,
  preserving token matches across chunk boundaries without materializing the
  complete JSON document;
- rejects missing or extra directional runtime source files.

The existing failed forward artifact independently contained 00/06/12 provenance
sources and no 18 UTC source. Existing clean-smoke evidence also confirmed native
backward provenance uses 06/12/18 and no 00 UTC source. The regression freezes
both manifest and provenance semantics. No schema version, production numerical
code, reader, DatasetLock, manifest structure, fixture, tolerance, cap, or error
classification changed.

Fresh Windows review package:

```text
target/m5-a4/a-review/windows-package-attempt-14/

archive SHA-256:
  814d5e226c55e8af56b9928e5bb528a82c30b7b695979c6bafc4f998b4a2c8f3
binary SHA-256:
  e286bbfc965d99abaf2a0b4c5927a1a2cef30edb8194238223bad337f16586fb
BUILD-MANIFEST SHA-256:
  03c3622d9ac4b4541c2d466c30beaa323ad747b91f5c981af66a0d9d66306bcb
source-tree SHA-256:
  a7399ad5eb1adb15d69d74732edfd07c2bf4bb8328fc70c14a155d511c7226be
```

Build and verify independently reported:

```text
ecCodes  2.47.0
netCDF-C 4.9.3
HDF5     1.14.6
```

The failed CFSR cell was replayed once from the clean extraction under:

```text
target/m5-a4/a-review/cfsr-input-identity-closeout-attempt-1/
```

Result:

```text
matrix summary SHA-256:
  6e090e425dedf7de21430d177cd84bffa362740a9eded2b3ae6d0b3cb5496284
cell summary SHA-256:
  a36cf3a11558e2c2a7057321702c6702b8c38e621db880ec9c51d02ef73f3cb9

result: passed
manifest/job: complete
abnormal_count: 0
runner: 1,076 ms
input_identity: passed (00/06/12/18 frozen hashes)
provenance_input_usage: passed (00/06/12 used; 18 absent)
all remaining product checks: passed
```

All source gates passed after the correction: workspace tests, workspace clippy
with `-D warnings`, doc, fmt, the 6 package tests and 7 matrix tests, Python
compilation, M4/M5 validators, and `git diff --check`.

Attempt-14 proved the corrected identity audit and failed-cell product path only.
At that historical stop, report/prompt changes required a fresh build under
`post-input-identity-closeout`, followed by a Windows restart from cell 1 before
Linux. Section 13 records the later completion.

## 13. Final A-owned Ubuntu 24.04 and cross-platform closeout

The paragraph immediately above is retained as historical chronology. The user
subsequently assigned all remaining M5-A4 execution to A. A built fresh Windows
and Ubuntu 24.04 packages from one source identity, ran both clean-package smokes,
ran all 60 formal cells without retry, and performed the required 30-pair review.

After those runs, only the existing A4 evidence/procedure Markdown files and the
ignored audit artifact under `target/` were updated. No product source, package
input, schema, fixture, or harness changed. The frozen package identity below is
therefore retained even though finalizing its own report changes a whole-tree
documentation hash.

Shared source identity:

```text
e4640fb83772237fb609ec307a3780ad6a63910ad89b6722489e4f08d3bbf75f
```

Formal results:

| Platform | Package verify | Clean smoke | Formal matrix |
|---|---:|---:|---:|
| Windows x86_64 | passed | 2/2 passed | 30/30 passed |
| Ubuntu 24.04 Linux x86_64 | passed | 2/2 passed | 30/30 passed |

Final evidence hashes:

```text
Windows package verify:
  6686750e02aab140be4b6923f0fc3bd920253074a1db5b33952ee8e7e0c42963
Linux package verify:
  95a0a0c733a960a039308a8472c811712bef5ee39f73cab7b327dcae7d0fbd7e
Windows smoke summary:
  f4eddeb41b4f9077a8b77bee445afe805aed3b0565c4f9cdb7aba7c2f77dcd87
Linux smoke summary:
  ff0ecf9f7e5f3179159659a3bf486c92763a153af41bca249edc6a4c0b61f559
Windows matrix summary:
  77d1149b91a8b8c95b88e87c31babed521f4aabc60d1e2202fb0afb24411b4fc
Linux matrix summary:
  329f284ba990bcbe90c83220690b7e965b4c288071860461c0aab37f9c15eede
Cross-platform audit:
  5f8cac1cdedf364676e83a91c9b3d0fadc6ca2ceb7e50194a0def5ac3e020c2f
```

The cross-platform artifact contains exactly 60 unique platform cell IDs and 30
pairing keys. Raw digest triplets are not bitwise equal: provenance v1 retains
the exact platform-local forensic source path. SQLite scientific content is
exact for 7/30 pairs. Across all 30 pairs, particle IDs and logical row sets are
identical, with no missing rows, non-finite values, value-presence mismatches, or
termination reason/classification changes.

The remaining measured differences are bounded floating-point deltas. Three 10k
domain-fill pairs each contain two terminal events whose integer time differs by
exactly one nanosecond; event, state, and termination records remain internally
consistent. Path-normalized provenance semantics match 27/30 pairs, and all
30/30 match after additionally normalizing only the corresponding interpolation
weight bits. The maximum ozone particle-mass relative delta is
`6.322889757768409e-14`, below the existing M4 domain-fill step tolerance
`1e-12`. No new tolerance was introduced.

A reviewed and accepted the full per-table/per-field evidence in:

```text
target/m5-a4/a-review/cross-platform-attempt-1/
  M5_A4_CROSS_PLATFORM_AUDIT.json
```

The legacy-named `TRAJECTA_M5_A4_B_EXECUTION_REPORT.md` contains the full final
package, smoke, matrix, and adjudication tables. M5-A4 is complete; M5 overall is
not complete. Versions remain `0.0.0`, and no commit or push was made as part of
this closeout.

Final post-report gates passed: workspace `543 passed / 12 ignored`, workspace
clippy with `-D warnings`, native-netCDF feature clippy with the formal Windows
native prefix, the real ERA5 native full-chain test, doc, fmt, 7 package tests, 8
matrix tests, Python compilation, M4/M5 validators, and `git diff --check`.
Windows and Ubuntu 24.04 process inspection found no remaining Trajecta process.

One additional attempt to execute the complete 15-test historical
`real_netcdf_pipeline` suite remained CPU-active at the 1,800-second outer limit
and was terminated by that limit. It is not counted as a pass or as a frozen A4
failure; the directly relevant real native full-chain gate completed and passed.
