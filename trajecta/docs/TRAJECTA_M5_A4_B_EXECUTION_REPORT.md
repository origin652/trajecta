# Trajecta M5-A4 Final Product-Matrix Execution Report

> The filename is retained for continuity with the earlier B execution handoff.
> The user reassigned all remaining M5-A4 execution and adjudication to A; this
> final report is A-owned and must not be treated as a B delivery.

## Status

**Passed. M5-A4 exit conditions are satisfied on Windows x86_64 and Ubuntu
24.04 Linux x86_64.** M5 as a whole remains incomplete because later M5 stages
are outside this report.

All 60 formal product cells ran from the same source identity and passed their
individual package, lifecycle, numerical-quality, SQLite, provenance, result,
and catalog gates. A then completed the required 30-pair cross-platform review
and accepted the measured differences without changing any tolerance, schema,
fixture, version, or runtime parameter.

No commit or push was made. All crate versions remain `0.0.0`. No subagent was
used. The outer `../origo-validation-v1.json` was not read or modified.

## Frozen source identity

```text
e4640fb83772237fb609ec307a3780ad6a63910ad89b6722489e4f08d3bbf75f
```

Both formal package manifests and all 60 cell summaries record this identity.
After the formal runs, A changed only the existing A4 evidence/procedure Markdown
files and generated the ignored cross-platform audit under `target/`; no product
source, package input, schema, fixture, or test harness changed. Those necessary
post-run report edits naturally change a whole-working-tree hash, so the package
is identified by the frozen hash above rather than a hash recomputed after its
own report was finalized.

## Formal packages

| Evidence | Windows x86_64 | Ubuntu 24.04 Linux x86_64 |
|---|---|---|
| Package root | `target/m5-a4/a-review/windows-package-attempt-17/` | `target/m5-a4/a-review/linux-package-attempt-4/` |
| Archive SHA-256 | `4773c4168f42188d15d91fdf3ab2489db87d986ee09853434f0a88d30a4322b0` | `897a36299153c845b0ef5d36a40221d059e92bd37dff424aa15ebf7990e1b701` |
| Binary SHA-256 | `a5cb33ac88668f0493a736d943140ee778b2361f54d5dfedc423a7e9f5528d3e` | `d23ad091cb02b5e6ba1e88b3345081665ec2ad86ec2617d0e497c98830fd3f78` |
| BUILD-MANIFEST SHA-256 | `4bb6bec2836b8ad7faf12a84634285eb972fa5a20c547f68637eee40de82eb78` | `b18a1dc700815af21f19f4097fc09c8b7191488643c832f3079a0a0104a134d9` |
| Verify-result SHA-256 | `6686750e02aab140be4b6923f0fc3bd920253074a1db5b33952ee8e7e0c42963` | `95a0a0c733a960a039308a8472c811712bef5ee39f73cab7b327dcae7d0fbd7e` |
| Payload files | 45 | 24,455 |
| ecCodes | 2.47.0 | 2.34.1 |
| netCDF-C | 4.9.3 | 4.9.2 |
| HDF5 | 1.14.6 | 1.10.10 |

Build-time declarations and package-extraction API probes agreed on both
platforms. The Linux package was built and verified in the installed
`Ubuntu-24.04` WSL distribution, not Ubuntu 22.04.

The large Linux payload is the closed native runtime, including the packaged
ecCodes definitions tree. Safe internal file symlinks were dereferenced into
ordinary archive files; broken links, directory links, and links escaping the
definitions root remained hard failures.

## Clean-package smoke

| Platform | Result | Summary | SHA-256 |
|---|---:|---|---|
| Windows x86_64 | 2/2 passed | `target/m5-a4/a-review/windows-clean-smoke-attempt-17/M5_A4_PRODUCT_MATRIX_SUMMARY.json` | `f4eddeb41b4f9077a8b77bee445afe805aed3b0565c4f9cdb7aba7c2f77dcd87` |
| Ubuntu 24.04 Linux x86_64 | 2/2 passed | `target/m5-a4/a-review/linux-clean-smoke-attempt-5/M5_A4_PRODUCT_MATRIX_SUMMARY.json` | `ff0ecf9f7e5f3179159659a3bf486c92763a153af41bca249edc6a4c0b61f559` |

Both clean extractions passed package verification, human/JSON help, config,
deep doctor, configured/pending example states, foreground default execution,
detached execution, cross-process job observation, idle daemon restart, full
result verification, trajectory streaming, automatic report generation, and
completed-job non-rerun behavior.

Manifest input identity is matched against the exact DatasetLock selected by
each cell. A smoke lock may contain three CFSR frames, while a formal lock may
contain four. Provenance independently proves the frames actually used by the
directional runtime path.

## Formal 60-cell matrix

| Platform | Result | Summary | SHA-256 |
|---|---:|---|---|
| Windows x86_64 | 30/30 passed | `target/m5-a4/a-review/windows-matrix-attempt-17/M5_A4_PRODUCT_MATRIX_SUMMARY.json` | `77d1149b91a8b8c95b88e87c31babed521f4aabc60d1e2202fb0afb24411b4fc` |
| Ubuntu 24.04 Linux x86_64 | 30/30 passed | `target/m5-a4/a-review/linux-matrix-attempt-5/M5_A4_PRODUCT_MATRIX_SUMMARY.json` | `329f284ba990bcbe90c83220690b7e965b4c288071860461c0aab37f9c15eede` |

Coverage is exactly 60 unique platform cell IDs and 30 unique pairing keys over
`family × population × direction × backend × particles × worker_threads`.
Every cell passed:

- packaged executable identity and native-backend selection;
- project finalize, foreground/detached lifecycle, daemon idle/restart, job and
  attempt identity, and completed-job non-rerun;
- `Complete`/manifest `complete`, abnormal count zero, finite/quality/population
  audits, mass ledger, and terminal-event consistency;
- SQLite integrity/counts, terminal WAL, trajectory output, full verification,
  deterministic report regeneration, and report digest exclusion;
- manifest DatasetLock identity and direction-specific provenance source usage.

No cell was retried in either final matrix. Older failed attempts remain intact
under their original attempt roots and were not relabeled or overwritten.

## Cross-platform digest and numerical audit

Machine evidence:

```text
target/m5-a4/a-review/cross-platform-attempt-1/
  M5_A4_CROSS_PLATFORM_AUDIT.json
SHA-256:
  5f8cac1cdedf364676e83a91c9b3d0fadc6ca2ceb7e50194a0def5ac3e020c2f
```

The artifact records both platforms' three digests and both full run directories
for every one of the 30 pairs, plus table-by-table logical-key, categorical, and
numeric comparisons.

### Raw digest counts

| Digest | Exact pairs |
|---|---:|
| `content_sha256` | 0/30 |
| `sqlite_sql_sha256` | 7/30 |
| `canonical_output_sha256` | 0/30 |
| Complete triplet | 0/30 |

This is not hidden or relabeled as exact determinism. Provenance v1 deliberately
retains the exact forensic source path, so Windows and Linux package-local paths
produce different raw provenance-content digests. Since canonical output combines
SQL and provenance content, it also differs whenever the content digest differs.

After normalizing only platform-local source paths, provenance record/field-set/
sample semantics match exactly for 27/30 pairs. The remaining three pairs match
after additionally normalizing only the `before_weight_bits` and
`after_weight_bits` attached to six terminal samples whose event time differs by
one integer nanosecond:

```text
era5-hybrid | air-mass | forward  | rust | 10000 | 4
era5-hybrid | ozone    | backward | rust | 10000 | 4
era5-pressure | air-mass | backward | rust | 10000 | 4
```

All 30 pairs have identical particle IDs and logical row-key sets. There are no
missing rows, non-finite comparison values, value-presence mismatches, or changes
to termination reason/classification. The six event timestamps are internally
consistent across `output_event`, `particle_state`, and `termination`; each delta
is exactly `1 ns`.

### Global measured numeric maxima

| Field | Maximum absolute delta | Maximum relative delta |
|---|---:|---:|
| particle ozone mass, kg | `6.103515625e-05` | `6.322889757768409e-14` |
| longitude, degrees | `2.278177646530821e-13` | `6.224898350119458e-14` |
| latitude, degrees | `1.2789769243681803e-13` | `1.5212917040707617e-15` |
| height ASL, m | `2.701625589907053e-10` | `2.112984430175438e-12` |
| eastward wind, m/s | `9.254819133275305e-13` | `7.595334928886927e-12` |
| northward wind, m/s | `1.2314593789142236e-12` | `1.9698744538836918e-11` |
| geometric vertical velocity, m/s | `1.686480816109892e-13` | `7.521846339323976e-10` |
| air pressure, Pa | `1.4115357771515846e-09` | `1.4169305443463353e-14` |
| air temperature, K | `1.0800249583553523e-12` | `3.814301331510445e-15` |
| boundary intersection fraction | `2.609690241683893e-12` | `4.1309760314995543e-10` |

The comparatively larger relative values are attached to quantities near zero;
their absolute deltas remain shown above. The maximum particle-mass relative
delta is below the existing M4 domain-fill step tolerance `1e-12` and final
tolerance `1e-11`.

No new trajectory tolerance was invented. Fields without an M4 cross-platform
hard tolerance remain report-only evidence, as required by the A4 plan, and were
explicitly adjudicated by A from the full logical-row comparison.

## A adjudication

A accepts the cross-platform result because:

1. all 60 individual product hard gates passed from one source identity;
2. all 30 pairs preserve input identity, numerical contract, particle identity,
   lifecycle and termination semantics, logical row sets, and finite values;
3. all non-exact numeric differences are explicitly bounded in the machine
   artifact, with existing M4 mass tolerances satisfied;
4. the only integer differences are six internally consistent `±1 ns` terminal
   time quantizations;
5. provenance differences are completely explained by exact forensic paths and
   those same terminal interpolation weights;
6. no tolerance, schema, version, fixture, reader selection, cap, or runtime
   parameter was changed to make the matrix pass.

Therefore the cross-platform audit result is `passed`, not `external_blocked` and
not an unresolved numerical mismatch.

## Final gates and cleanup

```text
cargo test --offline --workspace
  passed; 543 passed / 12 ignored (555 listed)

cargo clippy --offline --workspace --all-targets -- -D warnings
  passed

cargo clippy --offline -p trajecta-met --all-targets \
  --features native-netcdf -- -D warnings
  passed with the formal Windows native prefix:
    PATH begins D:\msys64\ucrt64\bin
    NETCDF_DIR=D:\msys64\ucrt64
    HDF5_DIR=D:\msys64\ucrt64

cargo test --offline -p trajecta-met --features native-netcdf \
  --test real_netcdf_pipeline \
  real_era5_cds_pressure_official_native_full_chain -- --exact
  1 passed / 14 filtered out

cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/test_m5_a4_package.py
  7 passed
python tools/test_m5_a4_product_matrix.py
  8 passed
python -m py_compile tools/m5_a4_package.py \
  tools/run_m5_a4_product_matrix.py
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
  all passed
```

The native prefix is the same MSYS2 UCRT64 environment independently probed by
the formal Windows package as netCDF-C 4.9.3 / HDF5 1.14.6. Without placing that
prefix first, the host's earlier Miniforge DLL directories cause GCC `cc1` to
load conflicting runtime bytes; this was diagnosed as an external shell
environment issue and not hidden by a source change.

An additional non-frozen attempt to run all 15 tests in
`real_netcdf_pipeline` remained CPU-active at the outer 1,800-second limit and
was terminated by that limit. It produced no failed assertion and is not counted
as passed. The A4-required real native full-chain test above completed and passed.

Final process inspection found no Trajecta process on Windows. Ubuntu 24.04's
process table likewise contained no Trajecta daemon, worker, or product runner.

## Final classification

- M5-A4: **complete**.
- M5 overall: **not complete**; later M5 stages remain.
- Commit/push: **not performed**.
- Product version: remains **`0.0.0`**.
