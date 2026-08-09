---
title: Tests and fixtures
description: Unit, contract, real-data, runtime, product-package, performance, and documentation test layers in Trajecta.
---

# Tests and fixtures

Trajecta tests are organized by the boundary they cross. A parser invariant can
usually be checked inside one crate; a claim about a distributed binary requires
a clean extracted package, its native libraries, real meteorology, and the
production daemon-worker path.

The workspace test suite is the common baseline. Specialized matrices add the
data and environment needed for broader claims.

## Test layers

| Layer | Main location | Question answered |
| --- | --- | --- |
| Unit tests | Module `#[cfg(test)]` blocks | Does one type or algorithm preserve its local invariants? |
| Crate integration tests | `crates/*/tests` | Do public modules compose without private test access? |
| Schema and contract tests | Crate tests plus `testdata` validators | Do public files and machine messages retain their declared shape? |
| Real-data reader tests | `trajecta-met/tests/real_*` | Does a production reader interpret an identified meteorological source correctly? |
| Numerical replay | `trajecta-core/tests/m4_*` | Do integration, boundaries, populations, mass, and outputs agree on real data? |
| Runtime contracts | `trajecta-cli/tests/m5_a2_runtime_contracts.rs` | Can the production daemon and worker complete, cancel, and retain an attempt? |
| Product package | `tools/m5_a4_package.py` and `tools/run_m5_a4_product_matrix.py` | Does a clean extracted archive run its bundled binary and native libraries? |
| Validation and performance | Frozen M4/M5 runners and publication assets | Do scientific metrics, identities, resource gates, and timing rules still pass? |
| Documentation | M5.1 generators, validators, MkDocs, quickstarts | Do examples, bilingual pages, references, links, and published metadata agree with the product? |

Passing a wider layer normally includes its narrower prerequisites, but it does
not make every narrower test redundant. Unit failures are much faster to locate
than a failed package cell.

## Workspace tests

Run the complete default-feature suite from the `trajecta` directory:

```text
cargo test --offline --workspace
```

During development, select the affected crate or integration target:

```text
cargo test --offline --package trajecta-case
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
cargo test --offline --package trajecta-core --test m4_a2_domain_fill
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli --test m5_a1_cli_contracts
```

Use `-- --list` to inspect the compiled test names and a trailing filter to run a
single case:

```text
cargo test --offline --package trajecta-core --test m4_a2_domain_fill -- --list
cargo test --offline --package trajecta-core --test m4_a2_domain_fill boundary_mass
```

The workspace lint and documentation gates accompany the tests:

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
git diff --check
```

## Contract fixtures

`testdata` contains formats that are shared outside a single module. Important
groups include:

| Prefix | Contents |
| --- | --- |
| `M3_*` | Reader comparison, tolerance, and scientific query contracts |
| `M4_*` | Numerical contract, run manifest, provenance bundle, SQLite schema, performance attribution |
| `M5_CLI_*` | Command tree, machine envelope, and stream-item formats |
| `M5_CONFIG`, `M5_PROJECT`, `M5_DATA_PLAN` | Local configuration and project control-plane formats |
| `M5_JOB_*`, `M5_PRUNE_*` | Job record, event, scheduler-facing contract, and dry-run cleanup plan |
| `M5_RESULT_*`, `M5_TRAJECTORY_*` | Result inspection and trajectory stream formats |
| `M5_BUILD_*`, `M5_PRODUCT_*` | Product archive and package-matrix formats |
| M5 comparison contracts | Comparison formats used only by the Validation section |
| `REAL_MET_MANIFEST.json` | Expected origin, path, size, and hash of external meteorological fixtures |

Schema files and examples are tested together. A schema change should include a
valid example, explicit invalid cases, and a decision about schema identity. A
field addition to an existing alpha document still needs all producers and
consumers updated in one change.

The top-level validators check relationships that JSON Schema alone cannot
express:

```text
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
```

These programs verify file identity, command coverage, stable constants, and
cross-document references. They are source checks and should finish without a
provider credential.

## Real meteorological fixtures

Large or provider-controlled data remains outside Git. The manifest records its
identity, while environment variables point tests to local copies.

| Variable | Dataset family |
| --- | --- |
| `TRAJECTA_REAL_CFSR_DIR` | Official CFSR pressure-level GRIB files |
| `TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR` | Prepared official ERA5 pressure-level files |
| `TRAJECTA_REAL_ERA5_HYBRID137_DIR` | Prepared official ERA5 hybrid 137-level files |
| `TRAJECTA_REAL_CFSR_PRESSURE_NETCDF_DIR` | CFSR pressure-level NetCDF fixture |
| `TRAJECTA_REAL_ERA5_PRESSURE_NETCDF_DIR` | ERA5 pressure-level NetCDF fixture |
| `TRAJECTA_REAL_ERA5_HYBRID_NETCDF4_DIR` | ERA5 hybrid NetCDF4 fixture |
| `TRAJECTA_REAL_CFSR_DERIVED_MULTIFILE_DIR` | Multi-file CFSR-derived NetCDF fixture |
| `TRAJECTA_REAL_NOAA_PSL_NCEP_R1_DIR` | NOAA PSL NCEP/NCAR Reanalysis 1 fixture |

Many ordinary real-data tests return early when their asset is absent. Set
`TRAJECTA_REQUIRE_REAL_MET=1` in formal runs so a missing required file becomes a
test failure:

```text
TRAJECTA_REQUIRE_REAL_MET=1 \
TRAJECTA_REAL_CFSR_DIR=/data/cfsr \
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
```

In PowerShell:

```text
$env:TRAJECTA_REQUIRE_REAL_MET = "1"
$env:TRAJECTA_REAL_CFSR_DIR = "D:\met\cfsr"
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
```

!!! tip "Require the fixture in formal runs"

    Set `TRAJECTA_REQUIRE_REAL_MET=1` so a missing local dataset fails the test
    immediately.

Keep the assertion tied to the source hash and Profile. A file with a familiar
name but different bytes is a different fixture.

## Pure and native reader tests

Reader work has three useful comparisons:

1. Metadata and inventory selection from the same source.
2. Field arrays, masks, grid, vertical coordinates, and valid times across the
   pure and native backends.
3. End-to-end query output after derivation and interpolation.

Build native tests with the feature being exercised:

```text
cargo test --offline --package trajecta-met \
  --features native-eccodes --test real_cfsr_pipeline

cargo test --offline --package trajecta-met \
  --features native-netcdf --test real_netcdf_pipeline

cargo test --offline --package trajecta-met \
  --features native-eccodes,native-netcdf --test real_netcdf_pipeline
```

Differential tolerances come from versioned tolerance contracts. Tests compare
all selected valid times and levels when the contract concerns full-field
interpretation. Sampling a convenient point is suitable for a smoke test, not
for reader equivalence.

## Numerical and lifecycle replay

`trajecta-core/tests` combines synthetic engineering cases with real-data
replays. Coverage includes:

- signed forward and backward clocks;
- spherical RK2 convergence and batch behavior;
- surface, top, horizontal-domain, and periodic boundaries;
- release, air-mass domain fill, and ozone domain fill;
- dynamic boundary births and stable particle IDs;
- mass-ledger closure and normal/abnormal termination classification;
- output-event coverage, SQLite row identity, provenance, and full verification;
- deterministic normalized output across worker counts.

Long real-data and performance cells are marked `#[ignore]` so the ordinary
workspace suite stays bounded. Run an ignored target by name with its frozen
fixture environment:

```text
cargo test --offline --release --package trajecta-core \
  --test m4_a4_real_perf -- --ignored --nocapture
```

Formal orchestration scripts add exact cell selection, timeout, resource
measurement, attempt preservation, and summary validation. Prefer the
orchestrator for an acceptance matrix; a direct ignored test is most useful
while investigating one cell.

## Runtime contracts

The M5 runtime tests launch the compiled CLI as a daemon and as a separate
worker. They use an isolated config, catalog, IPC endpoint, project, and result
root. The contract covers a complete run and cancellation/force-cancellation
paths with retained attempts.

```text
cargo test --offline --package trajecta-cli --test m5_a2_runtime_contracts
```

Run these tests serially only when diagnosing a host-specific process race; the
test itself owns isolated paths. After a failure, inspect the retained runtime
root before terminating unrelated processes. A stale worker should be matched by
both executable identity and run ID.

## Product packages

Package tests begin from an archive built by `tools/m5_a4_package.py`. Verification
checks the adjacent checksum, safe extraction paths, build manifest, payload
digests, SBOM, third-party license inventory, executable identity, and native
component inventory.

The clean-package smoke contains two CFSR cells:

- pure Rust reader, release population, forward, 1,000 particles, one worker;
- native reader, release population, backward, 1,000 particles, one worker.

The formal matrix has 30 cells on each platform:

| Group | Cells | Coverage |
| --- | ---: | --- |
| Rust 1k | 18 | Three dataset families × three populations × two directions |
| Rust 10k | 6 | Selected cross-family population and direction combinations, four workers |
| Native 1k | 6 | Three dataset families × two directions for release |

The runner creates a new attempt directory for every cell and stops at the first
failure. It verifies project finalization, daemon lifecycle, manifest status,
abnormal terminations, mass and quality checks, input identity, SQLite, WAL,
trajectory access, provenance, report idempotence, and catalog state.

Run package verification and a matrix only in a prepared formal environment:

```text
python tools/m5_a4_package.py verify \
  --archive <archive> \
  --extract-root <empty-directory> \
  --result <verification.json>

python tools/run_m5_a4_product_matrix.py smoke \
  --package-root <verified-root> \
  --artifact-root <new-artifact-root> \
  --cfsr-dir <cfsr-directory>
```

The formal product matrix adds both ERA5 roots and uses `matrix` in place of
`smoke`.

## Quickstarts and tutorials

`tools/run_m5_1_quickstart.py` executes the documented commands against either a
source-tree binary or an extracted product package. It creates a fresh project,
configures resource pools, builds a data plan, finalizes, runs deep doctor,
executes in the foreground, performs full verification, inspects the result,
reads one trajectory, and generates the run report.

The four example projects are:

| Example | Data and population |
| --- | --- |
| `domain-fill-cfsr` | CFSR domain-fill moisture workflow |
| `release-cfsr` | CFSR release workflow |
| `air-mass-era5-pressure` | ERA5 pressure-level air-mass workflow |
| `ozone-era5-hybrid` | ERA5 hybrid-level ozone workflow |

Weekly and release workflows run the two CFSR tutorials on Windows and Ubuntu
packages. The two ERA5 tutorials run on Ubuntu with provider credentials supplied
through CI secrets.

## Documentation tests

The documentation gate combines generated content, hand-written content, built
HTML, and one executable quickstart:

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_validation_assets.py
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
mkdocs build --strict
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli --site-dir site
```

The validator checks paired language paths, navigation, front matter, generated
references, documented commands, diagnostic codes, snippets, SEO metadata, and
style constraints. A separate dev-site build checks `noindex`. External links
run in a retrying CI job so network availability does not make the local Markdown
build non-deterministic.

Validation charts have an additional reproducibility gate:

```text
python tools/validate_m5_1_validation_assets.py
```

It rebuilds the five publication charts from frozen JSON/CSV data, compares
their SHA-256 values, and checks that both language asset trees contain identical
bytes.

## Failure artifacts

Formal runners use create-new attempt directories and do not overwrite a failed
cell. A useful failure handoff contains:

- exact source or package identity;
- command transcript and return code;
- first failing check;
- manifest, SQLite/WAL state, logs, and forensic files that exist;
- matrix summary showing cells passed, failed, and not run;
- confirmation that no later cell was started after a stop-on-failure gate.

An interrupted outer observation window and a failed product cell are different
states. Preserve the partial attempt, then start a new numbered attempt only
when the matrix contract permits a retry.

## Adding a test

Place a new test at the narrowest layer that observes the rule. Give it one
failure reason and use public APIs when the claim concerns a crate boundary.
When adding a fixture:

1. record its origin and license or data-use terms;
2. store size and SHA-256 in the relevant manifest;
3. keep credentials and provider configuration outside the fixture;
4. make missing-data behavior explicit for local and formal runs;
5. retain the smallest spatial and temporal subset that exercises the path;
6. add cleanup only for temporary files, never for a failed formal attempt.

Finish by running the focused test, its owning crate, workspace gates, and the
specialized matrix named in the change-routing table of
[Developer architecture](index.md).
