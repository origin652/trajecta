---
title: Platform and reader support
description: Check Trajecta 0.1.0-alpha.1 release archives, operating systems, architectures, bundled native libraries, dataset families, readers, populations, and directions.
---

# Platform and reader support

Trajecta `0.1.0-alpha.1` is distributed as a self-contained command-line
package for two x86-64 platforms. The formal product matrix runs the extracted
archive outside the source checkout and exercises both the local control plane
and the scientific result commands.

## Release packages

| Host | Architecture | Archive | Binary | Local control transport |
| --- | --- | --- | --- | --- |
| Windows x64 | x86-64 | `trajecta-0.1.0-alpha.1-windows-x86_64.zip` | `trajecta.exe` | Named pipe |
| Ubuntu 24.04 LTS | x86-64 | `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz` | `trajecta` | Unix-domain socket |

Each release also publishes a `.sha256` file. The archive contains:

- the Trajecta binary;
- `BUILD-MANIFEST.json` with source, binary, payload, platform, and native
  component identities;
- license and package README;
- complete example projects;
- bilingual quickstart, support matrix, and recovery notes;
- the separate Python data-fetch helper and its requirements file;
- native libraries and ecCodes definitions needed by the packaged readers.

The packaged binary can be placed anywhere after extraction. Project files and
data remain outside the program directory.

## Native runtime in the package

Both packages are probed after clean extraction. The native library versions
follow the distribution used to build each package:

| Component | Windows x86_64 | Ubuntu 24.04 x86_64 | Use |
| --- | ---: | ---: | --- |
| ecCodes | 2.47.0 | 2.34.1 | GRIB decoding and definitions |
| netCDF-C | 4.9.3 | 4.9.2 | NetCDF access for the native reader |
| HDF5 | 1.14.6 | 1.10.10 | Storage runtime used below netCDF-C |

Windows ships the required DLL set beside the executable. The Linux binary
loads packaged libraries through an `$ORIGIN/lib` runtime path. ecCodes
definitions are either included as package data or supplied by the packaged
memory-filesystem runtime recorded in the build manifest.

Users of the release archive do not install these three components separately.
A source build uses the development dependencies listed in the
[build guide](../developer/build.md).

!!! tip "The release package includes its native runtime"

    Install ecCodes, netCDF-C, and HDF5 development packages only when building
    Trajecta from source.

## Meteorological families

| Family | Typical source | Vertical coordinate | Rust reader | Native reader | Direction |
| --- | --- | --- | --- | --- | --- |
| CFSR pressure | NCEP CFSR pressure-level GRIB2 | Pressure levels | Supported and default | Supported | Forward and backward |
| ERA5 pressure | ERA5 pressure-level GRIB or NetCDF preparation | Pressure levels | Supported and default | Supported | Forward and backward |
| ERA5 hybrid | ERA5 model-level and surface fields | 137 hybrid model levels with half-level coefficients | Supported and default | Supported | Forward and backward |

Reader choice controls meteorological file access and query preparation. Both
readers feed the same Trajecta numerical core. A RunProfile can set
`execution.meteorology_reader`, and one dataset binding can supply a more
specific `reader_backend`.

The `rust` reader is the default created by `config init`. The `native` reader
uses the bundled libraries above. Confirm a local file and Profile with:

```text
trajecta data inspect FILE
trajecta --project PROJECT project finalize
trajecta --project PROJECT doctor --deep
```

## Formal product matrix

Each supported platform runs 30 formal cells from its clean package:

| Group | Cells per platform | Coverage |
| --- | ---: | --- |
| Rust 1k | 18 | Three dataset families × three populations × two directions, 1,000 particles, one worker |
| Rust 10k | 6 | Selected dataset, population, and direction combinations, 10,000 particles, four workers |
| Native 1k | 6 | Three dataset families × release population × two directions, 1,000 particles, one worker |

The three populations are regular release, dry-air-mass domain filling, and
stratospheric-ozone domain filling. Each cell prepares a project, finalizes its
data binding, runs through the daemon, reaches a terminal product, verifies the
manifest and SQLite database, reads trajectory output, and regenerates the run
report.

Before the formal matrix, a two-cell clean-package smoke covers CFSR forward
release through the Rust reader and CFSR backward release through the native
reader.

## Result-reader support

Trajecta product commands support read-only snapshots while a result is active.
They use the indexed high-water boundary published by the writer. A complete
attempt adds full verification, a terminal SQLite checkpoint, and a zero or
absent WAL.

Third-party SQLite programs can read the public version 1 schema on both
platforms. Open the database read-only and keep its WAL and SHM sidecars beside
it during active writing. The [SQLite reference](results-sqlite.md) lists query
ordering and lifecycle rules.

## Other systems

The current release workflow produces and formally exercises Windows x64 and
Ubuntu 24.04 x86-64 archives. Other Linux distributions may share a compatible
GNU C library and native runtime, though they do not have a release matrix in
`0.1.0-alpha.1`. macOS and ARM64 archives are not published in this release.

Running under WSL2 uses the Ubuntu 24.04 package inside the selected Linux
distribution. Store high-volume result and meteorological data on a filesystem
whose SQLite and large sequential I/O behavior suits the workload, then run
`doctor --deep` at the project location.
