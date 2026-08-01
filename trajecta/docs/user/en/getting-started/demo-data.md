---
title: CFSR demonstration dataset
description: Source, file inventory, SHA-256 identities, and use conditions for the Trajecta quickstart data asset.
---

# CFSR demonstration dataset

The quickstart asset contains four six-hourly CFSR pressure-level GRIB2 frames
for 1 January 2009. It is published separately from platform packages.

## Frozen inventory

| File | Bytes | SHA-256 |
|---|---:|---|
| `pgbl00.gdas.2009010100.grb2` | 5,465,079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| `pgbl00.gdas.2009010106.grb2` | 5,436,947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| `pgbl00.gdas.2009010112.grb2` | 5,452,202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |
| `pgbl00.gdas.2009010118.grb2` | 5,494,468 | `a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade` |

The deterministic ZIP contains these files, `MANIFEST.json`, and a source
notice. Its SHA-256 is
`cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8`.

## Source and conditions

The files originate from the
[NOAA NCEI CFSR six-hourly low-resolution archive](https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/2009/200901/20090101/).
The [official CFSR metadata record](https://www.ncei.noaa.gov/access/metadata/landing-page/bin/iso?id=gov.noaa.ncdc:C00765)
identifies the GRBLOW product label for the low-resolution GRIdded Binary (GRIB)
collection. Its Use Constraints require a
dataset citation and state the provider's warranty limitations. Its Access
Constraints contain the distribution-liability notice and no prohibition on
electronic redistribution. The
[NOAA Open Data page](https://www.noaa.gov/information-technology/open-data-dissemination)
describes NOAA data as publicly available and documents full and open access.
These official pages were checked on 2 August 2026.

The archive preserves the source bytes and names. It adds a manifest, hashes,
and a source notice. Cite Saha et al. (2010),
[*The National Centers for Environmental Prediction (NCEP) Climate Forecast System Reanalysis*](https://doi.org/10.1175/2010BAMS3001.1),
when using the data. The MIT license covers Trajecta code and metadata; it does
not replace source-data conditions. No NOAA endorsement is implied.
