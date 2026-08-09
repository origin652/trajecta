---
title: CFSR demonstration dataset
description: Download, verify, inspect, and place the four-frame CFSR dataset used by the Trajecta quickstart.
---

# CFSR demonstration dataset

The first Trajecta project uses a compact slice of the Climate Forecast System
Reanalysis. It contains four global pressure-level analyses from 1 January
2009, one every six hours. Together they occupy about 22 MiB and are small
enough to keep the first run convenient on a laptop.

The data is published as a release asset separate from the Windows and Ubuntu
packages. One copy can therefore serve either package and can remain in place
when the program is updated.

## What the archive contains

Download
[`trajecta-demo-cfsr-20090101-v1.zip`](https://github.com/origin652/trajecta/releases/download/v0.1.0-alpha.1/trajecta-demo-cfsr-20090101-v1.zip).
The archive expands to a directory with `data/`, `MANIFEST.json`, and a source
notice.

| Valid time | File | Bytes | SHA-256 |
|---|---|---:|---|
| 2009-01-01 00:00 UTC | `pgbl00.gdas.2009010100.grb2` | 5,465,079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| 2009-01-01 06:00 UTC | `pgbl00.gdas.2009010106.grb2` | 5,436,947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| 2009-01-01 12:00 UTC | `pgbl00.gdas.2009010112.grb2` | 5,452,202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |
| 2009-01-01 18:00 UTC | `pgbl00.gdas.2009010118.grb2` | 5,494,468 | `a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade` |

The archive SHA-256 is:

```text
cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8
```

`MANIFEST.json` repeats the file names, byte counts, hashes, source addresses,
and valid times in machine-readable form.

## Why the example uses four frames

The built-in `cfsr-pgbl-pressure-v0` dataset profile describes six-hourly
frames. During a trajectory step, Trajecta chooses the pair surrounding the
sample time and interpolates meteorological fields in time. The quickstart
itself runs from 06:00 to 06:10 UTC, so its samples remain near the 06 UTC
analysis and the following frame.

The demonstration project locks the complete four-frame asset. That inventory
covers a full day of source analyses and gives the longer tutorial room to
extend the Case without acquiring another small fragment immediately. The
DatasetLock records all four file identities; the query engine still selects
the time brackets required by each sample.

## Download and verify it

After the ZIP is downloaded, compare its hash and extract it:

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-demo-cfsr-20090101-v1.zip -Algorithm SHA256
    Expand-Archive `
      .\trajecta-demo-cfsr-20090101-v1.zip `
      -DestinationPath .\demo-data
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-demo-cfsr-20090101-v1.zip
    unzip trajecta-demo-cfsr-20090101-v1.zip -d demo-data
    ```

The extracted path used below is:

```text
demo-data/trajecta-demo-cfsr-20090101-v1/data/
```

The hashes in the inventory can also be calculated for an individual file when
copying the data between machines.

## Place the files in the example

The packaged project expects the files under
`examples/domain-fill-cfsr/data/`:

=== "Windows PowerShell"

    ```powershell
    Copy-Item `
      .\demo-data\trajecta-demo-cfsr-20090101-v1\data\* `
      .\examples\domain-fill-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    cp demo-data/trajecta-demo-cfsr-20090101-v1/data/* \
      examples/domain-fill-cfsr/data/
    ```

The release example uses a separate project directory. It can point at the same
physical data collection by changing the Profile's `data_roots.met` path, or it
can receive its own copy for an entirely self-contained tutorial directory.

## Inspect a frame with Trajecta

`data inspect` reads source metadata without running particles:

```text
trajecta --format json data inspect examples/domain-fill-cfsr/data/pgbl00.gdas.2009010100.grb2
```

For this file, the response identifies:

| Property | Value |
|---|---|
| Container | GRIB2 |
| Dataset family | `cfsr_pgbl_pressure` |
| Horizontal grid | 144 × 73, periodic longitude |
| Vertical coordinate | 37 pressure levels |
| Pressure range | 1 to 1,000 hPa |
| Frame interval used by the profile | 21,600 seconds |

The grid and level signatures reported by the command become part of the
DatasetLock when the project is finalized.

## Fields used by the example

The source files contain many CFSR messages. The dataset profile selects the
fields needed by Trajecta and maps their source units and level types into the
canonical field model.

| Purpose | Selected fields |
|---|---|
| Three-dimensional transport | Eastward and northward wind, pressure vertical velocity, air temperature, specific humidity, geopotential height |
| Surface state | Surface pressure and surface geopotential |
| Near-surface transport | 10 m wind, 2 m temperature, 2 m humidity, roughness length, boundary-layer height, heat fluxes, friction velocity |
| Domain-fill initialization | Wind, temperature, humidity, geopotential height, surface pressure, and terrain |

The quickstart uses domain-fill initialization and transport. Surface and
boundary-layer fields are present so that the same profile can support the
near-surface physics used by longer cases. Potential vorticity is derived from
the decoded pressure-level fields when a workflow requests the diagnostics
capability.

## Source and citation

The files are byte-for-byte copies from the
[NOAA NCEI CFSR six-hourly low-resolution archive](https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/2009/200901/20090101/).
The [CFSR metadata record](https://www.ncei.noaa.gov/access/metadata/landing-page/bin/iso?id=gov.noaa.ncdc:C00765)
describes the low-resolution GRBLOW collection and its use conditions. NOAA's
[open data page](https://www.noaa.gov/information-technology/open-data-dissemination)
provides the broader access policy. These source pages were reviewed on
2 August 2026.

The dataset citation is:

Saha et al. (2010),
[*The National Centers for Environmental Prediction Climate Forecast System Reanalysis*](https://doi.org/10.1175/2010BAMS3001.1).

The source notice inside the asset records the dataset title, provider, source
addresses, citation, and access date alongside the four preserved files.

## Use the data in another project

A RunProfile can bind the logical dataset to any local directory containing the
same files. The binding names the data root, reader, and DatasetLock path; the
Case continues to refer only to the logical dataset name. This allows several
projects to share one read-only data collection while keeping separate locks
and result directories.

For a different date or a wider period, generate a project data-plan first and
pass it to `tools/fetch_trajecta_data.py`. The helper turns the Case coverage
into provider requests and local target paths. After the requested frames are
present, `project finalize` inspects them and creates the lock used by the run.
