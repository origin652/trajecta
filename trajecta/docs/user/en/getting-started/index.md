---
title: Getting started with Trajecta
description: Choose a Trajecta package or source build, verify the archive, inspect the installation, and prepare a first project.
---

# Getting started

Trajecta is distributed as a command-line program with example projects and
offline recovery notes. Meteorological data is kept in a separate asset, so a
software update leaves an existing data collection in place. A new user can
start with the small CFSR project supplied in the package and later move the
same project structure to a longer run or another data family.

This section covers the path from a downloaded archive or source checkout to a
working executable. The [fifteen-minute quickstart](quickstart.md) continues
from there with a complete domain-fill moisture run.

## Choose a package or source build

| Route | Best fit | What you receive |
|---|---|---|
| Release package | Running Trajecta or following the tutorials | Product binary, native reader libraries, examples, data tools, offline notes, license inventory, and build manifest |
| Source build | Developing Trajecta or inspecting the implementation | A locally compiled `trajecta-cli` binary and the full Rust workspace |

The release package is the shortest route to a first run. It carries both the
Rust reader and the packaged native reader in one executable. The source route
used by the quickstart builds the Rust reader and is described directly in the
[build section](quickstart.md#1-obtain-the-executable).

## Supported packages

Version `0.1.0-alpha.1` publishes the following package targets:

| Platform | Architecture | Archive | Product binary | Reader choices |
|---|---|---|---|---|
| Windows | x86_64 | ZIP | `trajecta.exe` | Rust and packaged native |
| Ubuntu 24.04 | x86_64 | `tar.gz` | `trajecta` | Rust and packaged native |

The commands in this manual and the clean-package checks use these two
platforms. A source checkout may also be built on a compatible system with a
Rust 1.85 toolchain; such a build keeps its local binary name
`trajecta-cli`.

The reader choice belongs to the RunProfile. `rust` uses the decoder compiled
from Rust dependencies. `native` uses the ecCodes or netCDF-C runtime included
in the product archive. Both routes feed the same meteorological field model
used by the trajectory engine. The
[platform and reader matrix](../reference/platforms.md) lists the data formats
available through each reader.

## Install a release package

Download the archive for your platform and its adjacent `.sha256` file from
the [0.1.0-alpha.1 release](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1).
Keep both files in the same download directory.

### Verify the archive

Calculate the SHA-256 before extraction:

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -Algorithm SHA256
    Get-Content .\trajecta-0.1.0-alpha.1-windows-x86_64.zip.sha256
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    cat trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz.sha256
    ```

The calculated 64-character value matches the value in the checksum file. This
comparison catches an incomplete download before the package is opened.

!!! tip "Check before extracting"

    Keep the archive and its checksum together. A mismatch is easier to resolve
    before an incomplete package creates an installation directory.

### Extract and open the package

=== "Windows PowerShell"

    ```powershell
    Expand-Archive `
      .\trajecta-0.1.0-alpha.1-windows-x86_64.zip `
      -DestinationPath .
    Set-Location .\trajecta-0.1.0-alpha.1-windows-x86_64
    .\trajecta.exe --help
    ```

=== "Ubuntu 24.04"

    ```bash
    tar -xzf trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    cd trajecta-0.1.0-alpha.1-linux-x86_64
    ./trajecta --help
    ```

The help screen begins with the command synopsis and lists the `config`,
`project`, `run`, `job`, and `result` command groups. Run the remaining
getting-started commands from this extracted directory so that paths under
`examples/` and `tools/` resolve as shown.

## What the package contains

| Path | Purpose |
|---|---|
| `trajecta.exe` or `trajecta` | Command-line program, local daemon entry point, and worker entry point |
| `examples/` | Complete domain-fill, release, air-mass, and ozone projects |
| `tools/` | Data request helper and preparation utilities |
| `docs/quickstart/` | English and Chinese quickstarts, support matrix, and recovery sheet for offline use |
| `requirements-data.txt` | Python packages used by the ERA5 data preparation tools |
| `BUILD-MANIFEST.json` | Build identity and a hash for every packaged file |
| `SBOM.cdx.json` | Software component inventory |
| `THIRD-PARTY-LICENSES.json` | Third-party license inventory |
| `LICENSE` | Trajecta's MIT license text |

The example directories contain project documents and empty local data
locations. The CFSR demonstration asset supplies the files used by the first
two tutorials. ERA5 projects use the data-plan and download helper described in
the later guides.

## Prepare the first project

A normal first session has four stages:

1. Create a machine configuration with the CPU and memory available for local
   jobs.
2. Place the CFSR demonstration files under the supplied domain-fill project.
3. Finalize the project, which binds its Case and RunProfile to the local data.
4. Run the Profile, verify the result, and read one particle trajectory.

The [configuration and doctor](configuration.md) page explains the local
resource file. The [demonstration data](demo-data.md) page lists every CFSR file
and its checksum. The quickstart then combines both pieces into a complete run.

## Where to continue

| Goal | Next page |
|---|---|
| Build the executable from source | [Quickstart: obtain the executable](quickstart.md#1-obtain-the-executable) |
| Finish the first domain-fill run | [Fifteen-minute quickstart](quickstart.md) |
| Understand local CPU and memory settings | [Configuration and doctor](configuration.md) |
| Download and inspect the sample files | [CFSR demonstration data](demo-data.md) |
| Prepare a larger scientific workflow | [Tutorials](../tutorials/index.md) |
