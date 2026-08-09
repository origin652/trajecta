---
title: Build environment
description: Build the Trajecta workspace and full native CLI on Windows or Ubuntu, then run the local contributor gates.
---

# Build environment

Trajecta can be built in two useful forms. The default Cargo build keeps the
feedback loop short for Rust development. The full build enables the ecCodes and
netCDF-C readers used by the distributed product packages.

Commands on this page run from the `trajecta` directory inside the repository.

## Toolchain

| Component | Project baseline | Used for |
| --- | --- | --- |
| Rust | `1.85.0` or newer; edition 2024 | All five workspace crates and the CLI binary |
| Cargo | Supplied by the Rust toolchain | Locked dependency resolution, builds, tests, clippy, rustdoc |
| Python | 3.11 or newer; CI uses 3.12 | Validators, packaging, data helpers, documentation |
| Git | Current supported release | Source identity, package input inventory, release worktrees |
| C toolchain | Host ABI matching the Rust target | Native ecCodes, netCDF-C, and HDF5 linkage |
| MkDocs dependencies | Versions in `requirements-docs.txt` | Local site preview and strict documentation build |

The workspace declares `rust-version = "1.85"`. CI installs 1.85.0 explicitly,
so contributors should test language or standard-library changes against that
baseline even when their daily toolchain is newer.

```text
rustup toolchain install 1.85.0 --profile minimal
rustup override set 1.85.0
rustc --version
cargo --version
```

On Windows, enable Git long paths before working in deeply nested package or
runtime test directories:

```text
git config --global core.longpaths true
```

## Build the Rust CLI

Fetch dependencies once while network access is available, then build with the
lockfile:

```text
cargo fetch --locked
cargo build --locked --package trajecta-cli
```

The development executable is written to:

| Host | Binary |
| --- | --- |
| Windows | `target/debug/trajecta-cli.exe` |
| Linux | `target/debug/trajecta-cli` |

Confirm that the executable starts and that Cargo selected the expected source:

```text
target/debug/trajecta-cli --version
target/debug/trajecta-cli --help
```

PowerShell uses the Windows path form:

```text
.\target\debug\trajecta-cli.exe --version
.\target\debug\trajecta-cli.exe --help
```

For an optimized Rust-only binary:

```text
cargo build --offline --locked --release --package trajecta-cli
```

Cargo calls this executable `trajecta-cli`. The deterministic package builder
renames the product executable to `trajecta.exe` on Windows and `trajecta` on
Linux.

## Native reader build

The formal product feature set is:

```text
trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

Both features are explicit. When enabled, a missing native library or an ABI
mismatch stops the build; the selected reader does not switch silently to a
different implementation.

| Platform | Native development environment | Runtime requirement |
| --- | --- | --- |
| Ubuntu 24.04 x86_64 | `pkg-config`, ecCodes headers and library, netCDF-C headers and library, HDF5 headers and library; libclang when bindings are regenerated | Loader can find the linked shared libraries and ecCodes definitions |
| Windows x86_64 GNU | MSYS2 UCRT64 GCC, `pkgconf`, UCRT64 netCDF-C/HDF5, project ecCodes built for the same GNU ABI, libclang when bindings are regenerated | Required UCRT64 and ecCodes DLL directories remain on `PATH`; `ECCODES_DEFINITION_PATH` names the matching definitions |

On Ubuntu, install the distribution development packages before the full build:

```text
sudo apt-get update
sudo apt-get install pkg-config libeccodes-dev libnetcdf-dev libhdf5-dev libclang-dev
```

On the verified Windows GNU setup, MSYS2 UCRT64 supplies netCDF-C, HDF5, and
`pkgconf`:

```text
pacman -S --needed mingw-w64-ucrt-x86_64-netcdf mingw-w64-ucrt-x86_64-pkgconf
```

The Windows environment needs paths from one ABI family. A typical UCRT64 setup
has these values before Cargo starts:

```text
PATH=<ucrt64-bin>;<eccodes-bin>;%PATH%
NETCDF_DIR=<ucrt64-root>
PKG_CONFIG_PATH=<ucrt64-lib-pkgconfig>;<eccodes-lib-pkgconfig>
ECCODES_DEFINITION_PATH=<eccodes-share-definitions>
LIBCLANG_PATH=<directory-containing-libclang>
```

!!! warning "Use one Windows ABI"

    Pair `x86_64-pc-windows-gnu` Rust with UCRT64 native libraries. MSVC import
    libraries belong to a different toolchain.

Check the Rust host and the C library ABI together:

```text
rustc -vV
pkg-config --modversion netcdf
pkg-config --modversion eccodes
```

Build the same feature set as the product packages:

```text
cargo build --offline --locked --release --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

The command produces `target/release/trajecta-cli.exe` on Windows or
`target/release/trajecta-cli` on Ubuntu. Run `--help` in the same environment to
catch a missing runtime library before starting a data test.

## Choosing a build during development

| Work area | Fast loop | Required wider check |
| --- | --- | --- |
| Case, Profile, units, diagnostics | Default features and the affected crate tests | Workspace tests and schema/reference checks |
| Pure Rust query or numerical code | Default features | Relevant real-data replay and deterministic output tests |
| GRIB reader or ecCodes metadata | `native-eccodes` tests | Native differential tests and packaged smoke |
| NetCDF reader, hybrid levels, HDF5 access | `native-netcdf` tests | Native differential tests on the formal target |
| Native loading or packaging | Full feature build | Clean-extraction probe on Windows and Ubuntu |
| CLI rendering or job control | Default CLI build | Runtime contracts and machine-output tests |

Default-feature tests are useful while editing. They do not replace a native
reader check when the modified path changes file interpretation or package
loading.

## Workspace gates

After dependencies are cached, the main source gates run without registry
access:

```text
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
git diff --check
```

The workspace lint profile denies missing documentation for public Rust items,
unsafe code, ignored `must_use` results, `panic!`, `todo!`, `unimplemented!`,
`unwrap`, and `expect` in production targets. Test modules opt into narrowly
scoped allowances where fixture setup benefits from direct assertions.

Use package-specific commands during iteration:

```text
cargo test --offline --package trajecta-met query::
cargo test --offline --package trajecta-core integrator::
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli
```

Cargo accepts a test-name filter after the test target. List tests first when a
module path has changed:

```text
cargo test --offline --package trajecta-core -- --list
```

## Documentation environment

Create a dedicated Python environment so documentation packages do not alter a
provider or analysis environment:

```text
python -m venv .venv-docs
```

Activate it, then install the pinned site dependencies:

```text
python -m pip install --upgrade pip
python -m pip install -r requirements-docs.txt
```

Generate or check source-backed reference pages before building the site:

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
mkdocs build --strict
```

Use `target/debug/trajecta-cli.exe` for the two `--binary` arguments on Windows.
For a browser preview:

```text
mkdocs serve
```

The release and dev sites use separate MkDocs configurations. The dev build is
also checked for its search-engine `noindex` contract:

```text
mkdocs build --strict -f mkdocs.dev.yml -d site-dev
python tools/validate_m5_1_docs.py --site-dir site-dev --expect-noindex
```

## Cleaning and separate target directories

Native rebuilds and package matrices benefit from an isolated Cargo target:

```text
CARGO_TARGET_DIR=target-native cargo build --offline --locked --release \
  --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

In PowerShell, set `$env:CARGO_TARGET_DIR = "target-native"` for the current
session. Separate target directories keep default and native artifacts distinct
and make it clear which binary a test executed. A full `cargo clean` is rarely
needed; removing an isolated target directory is enough when its native build
metadata is stale.
