---
title: Build environment
description: Reproducible Rust, Python, native-reader, documentation, and local validation setup for Trajecta contributors.
---

# Build environment

## Required tools

- Rust `1.85` or newer with Cargo; the workspace uses edition 2024.
- Python `3.11` or newer for validators, packaging, evidence tools, and docs.
- Git with long-path support on Windows.
- MkDocs dependencies pinned in `requirements-docs.txt` for documentation work.

The repository supports offline Cargo gates after dependencies and native
components have been prepared:

```text
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
git diff --check
```

## Native readers

Product builds enable `trajecta-met/native-eccodes` and
`trajecta-met/native-netcdf`. The formal packages contain ecCodes, netCDF-C,
HDF5, and the required ecCodes definitions. A feature build must fail when its
native runtime is absent; it cannot silently switch readers.

Run pure-Rust tests without native features for fast development. Run both
reader paths before changing file interpretation, metadata, native loading, or
packaging.

## Documentation environment

Create a dedicated Python environment, install the pinned requirements, and
build from the `trajecta` directory:

```text
python -m pip install -r requirements-docs.txt
mkdocs build --strict
mkdocs serve
```

The published site uses the same commands in CI.
