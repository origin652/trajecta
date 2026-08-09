---
title: Packaging and release
description: Deterministic product archives, native runtime inventory, supply-chain files, documentation snapshots, and GitHub release flow.
---

# Packaging and release

Trajecta publishes host-native archives for Windows x86_64 and Ubuntu 24.04
x86_64. The archive is assembled from a release build and a declared payload;
users do not need a Rust or C development environment after extraction.

Software packages, demonstration data, and documentation are separate release
artifacts. This keeps the meteorological sample from being duplicated inside
both platform archives and allows the manual to be hosted as a versioned static
site.

## Release artifact set

For `0.1.0-alpha.1`, the expected public files are:

| Artifact | Purpose |
| --- | --- |
| `trajecta-0.1.0-alpha.1-windows-x86_64.zip` | Windows GNU-hosted product package |
| `trajecta-0.1.0-alpha.1-windows-x86_64.zip.sha256` | Archive checksum |
| `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz` | Ubuntu 24.04 product package |
| `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz.sha256` | Archive checksum |
| `trajecta-demo-cfsr-20090101-v1.zip` | Four-file CFSR demonstration dataset |
| Demonstration-data checksum and build result | Dataset archive identity |
| GitHub release notes | User-visible scope, install links, changes, and limits |
| `0.1.0-alpha.1` documentation snapshot | Bilingual immutable manual on GitHub Pages |

Validation CSV, JSON, and charts live in the documentation source and published
site. They are generated from frozen comparison outputs rather than copied into
the executable archives.

## Package contents

`tools/m5_a4_package.py` creates one root directory named for the version and
platform. Its payload contains:

| Group | Files |
| --- | --- |
| Executable | `trajecta.exe` on Windows or `trajecta` on Linux |
| Native runtime | Non-system ecCodes, netCDF-C, HDF5, and transitive shared libraries |
| ecCodes data | A copied definitions tree or a verified embedded-MEMFS marker |
| Supply chain | `BUILD-MANIFEST.json`, CycloneDX 1.5 `SBOM.cdx.json`, `THIRD-PARTY-LICENSES.json` |
| Legal and package information | Project `LICENSE` and package `README.md` |
| Offline help | English and Chinese quickstart, recovery notes, and support matrix |
| Example projects | Domain-fill CFSR, release CFSR, ERA5 pressure air mass, ERA5 hybrid ozone |
| Data utilities | Data-plan helper, CFSR and ERA5 retrieval/preparation tools, quickstart runner |
| Python requirements | `requirements-data.txt` for optional provider helpers |

Meteorological data and provider credentials are absent from the software
archives. Users download the demonstration asset or prepare their own locked
dataset.

## Host requirements for a formal build

Formal packages are built natively on their target operating system:

| Package | Rust host | Build host |
| --- | --- | --- |
| Windows | `x86_64-pc-windows-gnu` | Windows x64 with matching GNU native libraries |
| Linux | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86_64 |

The package tool checks the Rust host triple. Linux builds outside Ubuntu 24.04
are accepted only with an explicit nonformal preflight flag and cannot serve as
the release archive.

The release binary uses:

```text
cargo build --offline --locked --release --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

On Linux, the package build adds an `$ORIGIN/lib` runtime search path. On
Windows, the builder walks the executable's DLL imports recursively and copies
non-system dependencies from the supplied native-library directories.

## Deterministic archive construction

Package construction records a source identity before compiling. It includes
the Git commit, a digest over tracked and unignored source files, the file count,
and any dirty paths. Release review expects a deliberate source tree; generated
target directories and unrelated run artifacts are outside the package input.

The timestamp comes from `SOURCE_DATE_EPOCH`, defaulting to the commit time. The
archive writer then normalizes:

- member paths to forward-slash relative paths;
- root directory name;
- member ordering;
- file and directory modes;
- owner information in tar archives;
- ZIP and gzip timestamps;
- gzip filename metadata;
- JSON key order and final line endings.

!!! tip "Use a new package output root"

    The builder never overwrites an existing stage, archive, or checksum. Put
    each build in a separate root.

## Native library collection and probe

The archive must contain ecCodes, netCDF-C, and HDF5 exactly once in its declared
component inventory. The builder loads the copied runtime libraries and calls
their version APIs:

| Component | Probe |
| --- | --- |
| ecCodes | `codes_get_api_version` |
| netCDF-C | `nc_inq_libvers` |
| HDF5 | `H5get_libversion` |

The probed versions must equal the versions supplied to `--native-component`.
This catches a package assembled from a different DLL or shared object than the
release metadata describes.

Windows system DLLs are left to the operating system. Linux skips the system C
runtime and loader while copying other resolved dependencies into `lib/`.
Unresolved `ldd` entries or missing recursive DLL imports stop the build.

## Build manifest and supply-chain inventory

`BUILD-MANIFEST.json` is the package index. It records:

- product version, target triple, and minimum operating system;
- Rust and Cargo versions, feature set, source date, and source-tree identity;
- binary path, byte count, and SHA-256;
- archive format and root directory;
- every payload path, role, size, and SHA-256;
- native components and the version probe result;
- SBOM and license-inventory identities.

The CycloneDX document is built from the locked Cargo dependency closure plus
the three native components. The license inventory uses the same component set
and records source links and license expressions. Package verification cross-
checks these files instead of treating them as unrelated attachments.

## Package verification

Verification starts from the archive and adjacent checksum:

```text
python tools/m5_a4_package.py verify \
  --archive <archive> \
  --extract-root <new-directory> \
  --result <verification-result.json>
```

The verifier:

1. checks the archive SHA-256 and expected checksum filename;
2. rejects absolute paths, parent traversal, duplicate members, links, and an
   unexpected archive root;
3. extracts into a new directory without overwriting an existing tree;
4. validates the manifest schema and product/platform identity;
5. recalculates every payload digest and rejects unlisted files;
6. checks executable name and Linux execute permission;
7. validates the CycloneDX and license inventories;
8. loads the extracted native libraries and repeats the version probe.

The resulting JSON names the exact extracted root and identities used by later
smoke tests. Matrix runners should consume that root rather than guessing an
archive directory name.

## Demonstration-data asset

`tools/build_m5_1_demo_asset.py` builds the separately distributed CFSR sample.
The input manifest fixes four GRIB filenames, byte counts, SHA-256 values,
source information, and data-use notes. The ZIP uses a fixed root and timestamp.

```text
python tools/build_m5_1_demo_asset.py \
  --source <directory-with-four-cfsr-files> \
  --output <release-directory>/trajecta-demo-cfsr-20090101-v1.zip
```

The tool writes the archive, adjacent checksum, and a JSON build result. The
quickstart runner verifies the four file hashes again after extraction.

## Pre-release gates

A release candidate passes the following groups on the source commit that will
be tagged:

| Group | Required result |
| --- | --- |
| Source | Workspace tests, clippy with warnings denied, rustdoc, fmt, M4/M5 contract validators |
| Documentation | Generated references, bilingual validator, validation assets, strict release and dev builds |
| Packages | Build and verify both host-native archives in clean directories |
| Product smoke | Two clean-package CFSR cells on each platform |
| Product matrix | Thirty formal cells on Windows and thirty on Ubuntu |
| Tutorials | Domain-fill and release on both packages; air-mass and ozone on Ubuntu |
| Validation | Frozen scientific and performance comparison aggregate plus chart regeneration |
| Operations | No retained live daemon or worker after each matrix; failed attempts kept with summaries |

Formal matrices use stop-on-first-failure. A fixed implementation produces a new
source identity, package, extraction root, and attempt tree; a failed artifact is
not edited into a passing one.

## GitHub workflows

Documentation uses three workflows under `.github/workflows`:

| Workflow | Trigger | Work |
| --- | --- | --- |
| `m5-1-docs-ci.yml` | Documentation-related pull request or manual run | Build CLI reference, validate helpers and docs, strict-build release/dev sites, run the domain-fill source quickstart, check external links |
| `m5-1-docs-publish.yml` | Main documentation change, release, or explicit dispatch | Build static sites, upload artifacts, publish mutable `dev` or immutable release docs with `mike` |
| `m5-1-product-docs.yml` | Weekly schedule, release, or manual run | Download published packages and demo data, run packaged tutorials, check public Pages and crawler assets |

Pull requests do not publish Pages. Main-branch documentation changes update the
`dev` version. A release event or approved manual snapshot creates the immutable
`0.1.0-alpha.1` directory and updates the `latest` alias.

## Documentation publication

The release site is built with `mkdocs.yml`. The dev site inherits it through
`mkdocs.dev.yml`, changes the site URL to `/dev/`, and emits `noindex` metadata.
Both static trees are uploaded as workflow artifacts before publication.

`mike` stores versioned HTML on the `gh-pages` branch. Before creating a release
snapshot, the workflow checks that the version directory does not already exist.
Root `robots.txt` and `sitemap.xml` are generated from the release build; the
root sitemap points at versioned canonical URLs, while `/dev/` is disallowed for
crawlers.

The resulting `site` directory is ordinary static content. The same artifact can
be served by GitHub Pages, Nginx, or Caddy without a Node or Python server.

## Credentials and publication permissions

Release workflows use GitHub's short-lived token for release downloads and
Pages branch updates. ERA5 tutorials read `CDSAPI_URL` and `CDSAPI_KEY` from
repository secrets. Values are passed through process environment only.

Before staging release files, scan the source and intended artifacts for common
credential forms, local home paths, provider configuration files, and private
keys. Runtime logs and provenance should contain request identity and public
dataset metadata, not authorization headers or secret environment values.

## Release sequence

1. Select the commit and confirm workspace version `0.1.0-alpha.1`.
2. Run source, documentation, validation, and product gates.
3. Build each host-native archive in a new output root.
4. Verify each archive into a new extraction directory and run its package
   smoke and formal matrix.
5. Build and verify the demonstration-data asset.
6. Review `BUILD-MANIFEST.json`, SBOM, license inventory, checksums, and release
   notes together.
7. Tag the selected commit and publish the software and data assets.
8. Publish the immutable bilingual documentation snapshot.
9. Open the public download, quickstart, versioned manual, sitemap, language
   switch, and chart pages from a clean browser session.
10. Retain workflow summaries and failed-attempt artifacts according to their
    configured artifact retention.

Documentation corrections made after the release continue on `dev`. The
software version changes only when a new software release is prepared.
