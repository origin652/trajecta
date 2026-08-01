# Trajecta package quick start

This archive contains the `0.1.0-alpha.1` prerelease. It is intended for
evaluation and clean-package validation before a stable release.

Run `trajecta --help` (or `trajecta.exe --help` on Windows), then create a
machine-local configuration:

```text
trajecta --config config.toml config init
trajecta --config config.toml doctor --deep
```

`examples/minimal` is intentionally configured before meteorology is present.
Copy it to a writable directory, place the required CFSR files under `data/`,
then run `project validate`, `project data-plan`, and `project finalize`.
Trajecta never downloads meteorology while starting a run.

The Rust reader is the default. The optional native reader is compiled into the
same executable and uses the native runtime files shipped with this archive.
Scientific input data is not redistributed in the package.

`BUILD-MANIFEST.json`, `SBOM.cdx.json`, and
`THIRD-PARTY-LICENSES.json` describe the exact package payload. Verify the
archive against the adjacent `.sha256` file before extraction.
