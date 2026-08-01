---
title: Platform and reader support
description: Formally validated package platforms, meteorological families, readers, populations, and direction coverage for Trajecta 0.1.0-alpha.1.
---

# Platform and reader support

## Product packages

| Platform | Architecture | Package status | Local control transport |
| --- | --- | --- | --- |
| Windows | x86_64 | Formally validated | Local named pipe |
| Ubuntu 24.04 LTS | x86_64 | Formally validated | Local Unix-domain socket |

Other Linux distributions may run compatible binaries, but they are outside the
formal support claim for this release. macOS and non-x86_64 packages are not
claimed.

## Meteorological readers

| Family | Rust reader | Native reader | Forward/backward | Product matrix scope |
| --- | --- | --- | --- | --- |
| CFSR pressure | Supported; default | Included | Both | release, air-mass, ozone with Rust; release with native |
| ERA5 pressure | Supported; default | Included | Both | release, air-mass, ozone with Rust; release with native |
| ERA5 hybrid | Supported; default | Included | Both | release, air-mass, ozone with Rust; release with native |

The native package runtime contains ecCodes, netCDF-C, and HDF5. `native` is a
reader selection, not a different numerical core. Validation claims apply only
to the cells listed above and to the frozen fixture coverage. Run `data inspect`,
`project finalize`, and `doctor --deep` for each local dataset.

Concurrent result readers use indexed high-water snapshots. They are supported
during active writing through Trajecta product commands; third-party tools must
open SQLite read-only and tolerate WAL snapshots.
