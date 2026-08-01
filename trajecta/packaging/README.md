# Trajecta package quick start

This archive contains the `0.1.0-alpha.1` prerelease. It is intended for
evaluation and clean-package validation before a stable release.

Run `trajecta --help` (or `trajecta.exe --help` on Windows), then create a
machine-local configuration:

```text
trajecta --config config.toml config init
trajecta --config config.toml doctor --deep
```

`examples/domain-fill-cfsr` is intentionally configured before meteorology is
present. The package also carries the release, air-mass, and ozone tutorial
projects. Their files are the configuration source used by the online manual.

`docs/quickstart` contains the bilingual quickstart, support matrix, and
emergency recovery sheet for offline use.
Copy it to a writable directory, place the required CFSR files under `data/`,
then run `project validate`, `project data-plan`, and `project finalize`.
Trajecta never downloads meteorology while starting a run.

`tools/fetch_trajecta_data.py` previews every provider request by default and
downloads only with `--execute`. CFSR downloads use the Python standard
library. ERA5 pressure and hybrid preparation require the pinned packages in
`requirements-data.txt`:

```text
python -m pip install -r requirements-data.txt
```

The ERA5 hybrid coefficient extractor is included in the helper and does not
require Cargo or a source checkout. CDS credentials remain in the provider's
official configuration or environment and are not copied into the project.

The Rust reader is the default. The optional native reader is compiled into the
same executable and uses the native runtime files shipped with this archive.
Scientific input data is not redistributed in the package.

`BUILD-MANIFEST.json`, `SBOM.cdx.json`, and
`THIRD-PARTY-LICENSES.json` describe the exact package payload. Verify the
archive against the adjacent `.sha256` file before extraction.
