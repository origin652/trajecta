# Historical B Prompt — M5-A4 formal packages and 60-cell product matrix

> Superseded: the user assigned all remaining M5-A4 package, smoke, matrix, and
> cross-platform execution to A. This file is retained only as the frozen
> mechanical procedure; it must not be handed to B or used to delegate later
> stages. The formal Linux baseline is now Ubuntu 24.04 by user decision.

You are B. Execute the A-frozen M5-A4 package and product matrix mechanically.
Do not modify implementation to make a failed cell green. Stop at the first real
product failure and return the preserved evidence to A.

This is the post-input-identity-closeout continuation. The three old B Windows
matrices (`2 passed / 1 failed / 27 not run`, `4 passed / 1 failed / 25 not run`,
and `12 passed / 1 failed / 17 not run`) and A's review packages attempt-12,
attempt-13, and attempt-14 are evidence only. Do not resume or reuse any of them.
Build from the current tree, record one new source identity, and restart the
Windows 30-cell matrix from cell 1 without a `--cell` filter. Linux may start only
after Windows passes all 30 cells. A closed the exact-frame bottom transport,
quality, force-cancel report, ozone Case, and CFSR input-identity harness issues.
The manifest now remains gated against all content frozen by its lock, while a
separate provenance gate verifies direction-specific runtime source usage. Both
the corrected ozone cell and CFSR forward cell passed clean-package A replays. B
must not edit these implementations or the harness.

## 0. Absolute rules

- Do not use subagents.
- Do not commit, push, tag, publish, or change any crate version. Every crate remains
  `0.0.0`.
- Do not read or touch `../origo-validation-v1.json`.
- Do not modify numerical core, meteorology, Case semantics, integration,
  interpolation, boundary handling, population logic, tolerances, abnormal
  classification, manifest/provenance/SQLite schemas, caps, or deletion behavior.
- `job rerun`, `job forget`, and `job prune` remain non-deleting/dry-run. Do not add
  `--apply` or any real artifact deletion.
- Do not create `v2`, `fix2`, legacy/new, copied runner, copied package script, or a
  second formal path.
- Do not install packages, download data, change system limits, retry a failed cell,
  reduce particles, change duration, change workers, or overwrite an attempt.
- The only repository file B may add or edit is:

```text
docs/TRAJECTA_M5_A4_B_EXECUTION_REPORT.md
```

- All other output belongs under `target/m5-a4/b-formal/post-input-identity-closeout/` or the explicitly named
  clean-extraction directories outside the source tree.

## 1. Record the starting identity

From `E:\flexpart\trajecta` record, without changing anything:

```text
git rev-parse HEAD
git status --short
git diff --stat
cargo metadata --offline --locked --no-deps --format-version 1
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
```

The worktree intentionally contains uncommitted A3/A4 work. Preserve every existing
change. Confirm all workspace package versions are `0.0.0`.

Run the focused A4 gates before packaging:

```text
cargo fmt --all -- --check
python tools/test_m5_a4_package.py
python tools/test_m5_a4_product_matrix.py
python -m py_compile tools/m5_a4_package.py tools/run_m5_a4_product_matrix.py
git diff --check
```

If any source gate fails, stop and report `failed`; do not edit implementation.

## 2. Frozen fixture roots and identities

Use only these existing A4 fixture roots:

```text
Windows:
  target/m5-a4/fixtures/era5-pressure-4frame/ready
  target/m5-a4/fixtures/era5-hybrid-4frame/ready
  target/m5-a4/fixtures/cfsr-pressure-4frame

WSL:
  /mnt/e/flexpart/trajecta/target/m5-a4/fixtures/era5-pressure-4frame/ready
  /mnt/e/flexpart/trajecta/target/m5-a4/fixtures/era5-hybrid-4frame/ready
  /mnt/e/flexpart/trajecta/target/m5-a4/fixtures/cfsr-pressure-4frame
```

The runner performs exact SHA validation. Expected identities are frozen in
`tools/run_m5_a4_product_matrix.py`. Missing files are `external_blocked`; a present
file with a different SHA is `failed`. Do not fetch, regenerate, copy from a
different fixture, or change the expected hashes.

For CFSR, `input_identity` requires the exact files frozen by the DatasetLock
selected for that cell. A smoke lock may contain three frames; the formal 600 s
lock used by this procedure may contain four. The separate
`provenance_input_usage` gate requires exactly 00/06/12 for forward formal cells
and 06/12/18 for backward formal cells. Either a missing or extra provenance
source is a real failure; do not weaken either gate.

## 3. Windows formal package

Run native/release commands outside the sandbox. Use a fresh target and fresh output
root; neither may already exist.

PowerShell environment and command:

```powershell
Set-Location E:\flexpart\trajecta
$native=(Resolve-Path '.native/eccodes/Library').Path
$ucrt='D:\msys64\ucrt64'
$env:PATH="$native\bin;$ucrt\bin;$env:PATH"
$env:PKG_CONFIG_PATH="$ucrt\lib\pkgconfig;$native\lib\pkgconfig"
$env:NETCDF_DIR=$ucrt
$env:LIBRARY_PATH="$ucrt\lib"
$env:RUSTFLAGS='-L native=D:/msys64/ucrt64/lib'
Remove-Item Env:CPATH -ErrorAction SilentlyContinue
$env:LIBCLANG_PATH='C:\Users\dell\miniforge3\pkgs\libclang-22.1.8-default_h570ddc7_3\Library\bin'

python tools/m5_a4_package.py build `
  --output-root target/m5-a4/b-formal/post-input-identity-closeout/windows-package-attempt-1 `
  --target-dir target/m5-a4/b-formal/post-input-identity-closeout/windows-target-attempt-1 `
  --native-library-dir "$native\bin" `
  --native-library-dir "$ucrt\bin" `
  --eccodes-definitions embedded `
  --native-component "ecCodes|2.47.0|Apache-2.0|https://github.com/ecmwf/eccodes" `
  --native-component "netCDF-C|4.9.3|BSD-3-Clause|https://github.com/Unidata/netcdf-c" `
  --native-component "HDF5|1.14.6|LicenseRef-HDF5|https://github.com/HDFGroup/hdf5"
```

The build must independently report exactly:

```text
native_version_probe:
  ecCodes  = 2.47.0
  netCDF-C = 4.9.3
  HDF5     = 1.14.6
```

Any mismatch is a package failure. Do not change the declared version to match an
accidentally selected DLL; fix nothing and return the artifact to A.

Verify and clean-extract outside the source tree:

```powershell
python tools/m5_a4_package.py verify `
  --archive target/m5-a4/b-formal/post-input-identity-closeout/windows-package-attempt-1/trajecta-0.0.0-windows-x86_64.zip `
  --extract-root E:\flexpart\m5-a4-b-post-input-identity-closeout-windows-extract-attempt-1 `
  --result target/m5-a4/b-formal/post-input-identity-closeout/windows-package-attempt-1/M5_A4_PACKAGE_VERIFY_RESULT.json
```

Build and verify native probes, archive SHA, binary SHA, manifest SHA, source SHA,
payload count, and runtime-file list must all be retained in the report.

## 4. Windows clean-package smoke

Run only the extracted product binary through the A runner:

```powershell
python tools/run_m5_a4_product_matrix.py smoke `
  --package-root E:\flexpart\m5-a4-b-post-input-identity-closeout-windows-extract-attempt-1\trajecta-0.0.0-windows-x86_64 `
  --artifact-root target/m5-a4/b-formal/post-input-identity-closeout/windows-clean-smoke-attempt-1 `
  --cfsr-dir target/m5-a4/fixtures/cfsr-pressure-4frame `
  --timeout-seconds 1200
```

Required result: `passed`, 2/2 cells, no not-run cell. This covers default
foreground Rust, detached native, daemon idle/restart, completed-job non-rerun,
full verification, SQLite/WAL, provenance, trajectory, report, catalog, and
abnormal=0.

If smoke fails, stop all formal matrix work and report the first failed attempt.

## 5. Windows 30-cell formal matrix

Only after the Windows smoke passes:

```powershell
python tools/run_m5_a4_product_matrix.py matrix `
  --package-root E:\flexpart\m5-a4-b-post-input-identity-closeout-windows-extract-attempt-1\trajecta-0.0.0-windows-x86_64 `
  --artifact-root target/m5-a4/b-formal/post-input-identity-closeout/windows-matrix-attempt-1 `
  --era5-pressure-dir target/m5-a4/fixtures/era5-pressure-4frame/ready `
  --era5-hybrid-dir target/m5-a4/fixtures/era5-hybrid-4frame/ready `
  --cfsr-dir target/m5-a4/fixtures/cfsr-pressure-4frame `
  --timeout-seconds 14400
```

The runner owns order, cell definitions, duration, particles, workers, and
stop-on-fail. Do not pass `--cell`; this must be the full frozen 30-cell matrix.

Required aggregate:

```text
result=passed
passed_cell_count=30
failed_cell_count=0
not_run_cell_ids=[]
```

At the first failed cell, the runner stops automatically. Do not retry and do not
start Linux. Return that attempt to A.

## 6. Ubuntu 24.04 formal package

Run in the installed `Ubuntu-24.04` WSL distribution, outside the sandbox. A
different distribution is not formal evidence.

First record:

```bash
cat /etc/os-release
rustc -vV
cargo -V
pkg-config --modversion eccodes
pkg-config --modversion netcdf
pkg-config --modversion hdf5
codes_info -d
```

If Ubuntu 24.04 or any required native development/runtime component is absent,
record `external_blocked` and stop. Do not install anything.

Use the versions actually printed by `pkg-config` as the three
`--native-component` values. Do not guess. Use the actual directory printed by
`codes_info -d` for `--eccodes-definitions`.

From `/mnt/e/flexpart/trajecta`, with a fresh `/tmp` target:

```bash
python tools/m5_a4_package.py build \
  --output-root target/m5-a4/b-formal/post-input-identity-closeout/linux-package-attempt-1 \
  --target-dir /tmp/trajecta-m5-a4-post-input-identity-closeout-linux-target-attempt-1 \
  --eccodes-definitions "$(codes_info -d)" \
  --native-component "ecCodes|<actual>|Apache-2.0|https://github.com/ecmwf/eccodes" \
  --native-component "netCDF-C|<actual>|BSD-3-Clause|https://github.com/Unidata/netcdf-c" \
  --native-component "HDF5|<actual>|LicenseRef-HDF5|https://github.com/HDFGroup/hdf5"
```

The package builder enforces Ubuntu 24.04 and probes the packaged libraries. A
declared/probed mismatch is `failed`, not `external_blocked`.

Verify and extract:

```bash
python tools/m5_a4_package.py verify \
  --archive target/m5-a4/b-formal/post-input-identity-closeout/linux-package-attempt-1/trajecta-0.0.0-linux-x86_64.tar.gz \
  --extract-root /tmp/trajecta-m5-a4-post-input-identity-closeout-linux-extract-attempt-1 \
  --result target/m5-a4/b-formal/post-input-identity-closeout/linux-package-attempt-1/M5_A4_PACKAGE_VERIFY_RESULT.json
```

## 7. Linux clean smoke and 30-cell matrix

Smoke:

```bash
python tools/run_m5_a4_product_matrix.py smoke \
  --package-root /tmp/trajecta-m5-a4-post-input-identity-closeout-linux-extract-attempt-1/trajecta-0.0.0-linux-x86_64 \
  --artifact-root target/m5-a4/b-formal/post-input-identity-closeout/linux-clean-smoke-attempt-1 \
  --cfsr-dir target/m5-a4/fixtures/cfsr-pressure-4frame \
  --timeout-seconds 1200
```

Only after smoke passes, formal matrix:

```bash
python tools/run_m5_a4_product_matrix.py matrix \
  --package-root /tmp/trajecta-m5-a4-post-input-identity-closeout-linux-extract-attempt-1/trajecta-0.0.0-linux-x86_64 \
  --artifact-root target/m5-a4/b-formal/post-input-identity-closeout/linux-matrix-attempt-1 \
  --era5-pressure-dir target/m5-a4/fixtures/era5-pressure-4frame/ready \
  --era5-hybrid-dir target/m5-a4/fixtures/era5-hybrid-4frame/ready \
  --cfsr-dir target/m5-a4/fixtures/cfsr-pressure-4frame \
  --timeout-seconds 14400
```

Required aggregate is the same 30/30 pass shape as Windows. Stop at the first
failure; do not retry.

## 8. Cross-platform evidence

Only if both 30-cell matrices pass:

- verify there are exactly 60 unique cell IDs and one binary/source/build-manifest
  identity per platform;
- pair cells by `(family, population, direction, backend, particles,
  worker_threads)`;
- record both platforms' `content_sha256`, `sqlite_sql_sha256`, and
  `canonical_output_sha256` for all 30 pairs;
- count exact triplet matches and list every mismatch with both run directories.

Do not declare a numerical mismatch acceptable. If any normalized triplet differs,
mark the cross-platform review `needs_A_adjudication`, stop, and hand the two full
run directories to A. Do not change tolerance or rerun.

## 9. Final gates and process cleanup

After execution, without modifying implementation:

```text
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
python tools/test_m5_a4_package.py
python tools/test_m5_a4_product_matrix.py
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

Confirm no packaged Trajecta daemon/worker remains on Windows or WSL. Do not kill
unrelated processes. Do not delete retained failure attempts or completed results.

## 10. Required B report

Write only:

```text
docs/TRAJECTA_M5_A4_B_EXECUTION_REPORT.md
```

Include:

- starting/ending HEAD, status, and diff stat;
- exact Windows and Linux OS/toolchain/native component identities;
- package archive/binary/build-manifest/source/SBOM/license hashes;
- build-time and verify-time native version probes;
- clean-smoke summaries and hashes;
- per-platform matrix summary paths/hashes and 30 cell result table;
- first failure or external blocker, with exact retained attempt;
- cross-platform digest-pair table/counts if all 60 cells passed;
- final gates and test counts;
- confirmation that no implementation, version, schema, numerical contract,
  tolerance, deletion behavior, or outer file was changed;
- explicit statement: no commit/push and no claim that M5-A4 or M5 is complete.

Possible final classifications:

- `passed_for_A_review`: both packages, both smokes, all 60 cells, all gates, and no
  unresolved cross-platform digest mismatch;
- `failed`: a source/package/product/gate failure;
- `external_blocked`: only a genuinely unavailable fixture, Ubuntu 24.04 runtime,
  or required native dependency, with no installation attempt.

B never gives the final M5-A4 decision. A reviews the actual artifacts and report.
