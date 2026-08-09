---
title: Scientific validation method
description: Understand the meteorological alignment, particle accounting, ensemble metrics, and frozen one-hour scientific comparison used for Trajecta.
---

# Scientific validation method

The published scientific comparison follows a compact advection case through
two independent trajectory implementations. Its purpose is to examine the
meteorological handoff and the evolution of a particle ensemble at common
physical times. Each program also completes its own lifecycle and product
checks before its output enters the comparison.

## Frozen case

| Setting | Value |
| --- | --- |
| Meteorology | Official ERA5 hybrid-level source content |
| Start of transport | 2018-12-01 03:00:00 UTC |
| Simulation duration | 3,600 seconds |
| Transport step | 600 seconds |
| Common output interval | 1,200 seconds |
| Compared output times | 03:20, 03:40, and 04:00 UTC |
| Particle populations | 10,000 and 50,000 |
| CPU allocation | Four threads on CPU set `0-3` |
| Processes enabled for comparison | Advection and the shared trajectory state used by the metric set |

The source location used by the ensemble metrics is 5.25° E, 49.25° N. Wet
deposition and convection are disabled in this case. Trajecta surface-flux
fields remain present in its prepared dataset and sit outside the shared
advection quantity set.

## Meteorological alignment

Both execution paths begin with the same official source files. Their reader
and staging formats differ, so alignment is recorded at several levels.

### Source identity

The source NetCDF and GRIB files are fixed by byte length and SHA-256. The
prepared Trajecta files and staged FLEXPART GRIB files have their own identities
derived from those sources. The complete lists are stored in
`M5_A5_METEOROLOGY_EQUIVALENCE.json`.

### Fields and coordinates

The alignment includes:

- four valid times;
- 137 model levels and 138 hybrid half-level coefficients;
- longitude and latitude axes;
- horizontal wind, vertical motion, temperature, and specific humidity;
- logarithmic surface pressure and its derived surface-pressure value;
- the surface and near-surface fields used by the selected advection setup.

Trajecta-ready arrays are compared directly with the prepared source arrays.
Surface pressure reconstructed from logarithmic pressure uses an explicit
round-off tolerance. GRIB values staged for FLEXPART are compared with their
source values within each message's ecCodes packing error. This accounts for
the quantization introduced by GRIB packing while keeping units and physical
interpretation aligned.

### Query coverage

Reader checks cover the physical times, grid locations, and vertical range
needed by the Case. A query must return the requested fields with finite values
inside the dataset's safe spatial core. The DatasetLock fixes the files and
coverage that the production run later uses.

## Trajecta run checks

Before a Trajecta output is used for cross-model metrics, the run closes with
`complete` and zero abnormal terminations. Its full verification includes:

| Area | Check |
| --- | --- |
| Population | Initial count, active count, and every termination remain mutually consistent |
| Lifecycle | Scheduled output states are ordered and each particle has one valid state path |
| Numeric values | Coordinates, height, time, and population quantities remain finite and within their declared constraints |
| Mass ledger | Initial, active, boundary, and terminated mass entries close at each event |
| Boundaries | Domain and vertical boundary classifications follow the selected Case |
| Output | Manifest counts agree with SQLite; integrity succeeds; the terminal WAL is truncated or absent |
| Provenance | Resolved inputs, software identity, and canonical output digest are complete |
| Determinism | Formal repetitions retain the required normalized content, SQL, and canonical identities |

The corresponding FLEXPART product run is also required to finish successfully
and to contain the expected particle count and three common output times.

## Ensemble metric definitions

Metrics are calculated from finite active particle coordinates at each common
time.

### Horizontal centroid

Longitude and latitude are converted to unit-sphere Cartesian coordinates,
averaged, and converted back to geographic coordinates. This spherical mean
handles longitude wraparound. Horizontal centroid separation is the great-
circle distance between the two ensemble centroids using a mean Earth radius of
6,371,008.8 m.

### Vertical centroid

Height is expressed as metres above sea level. For the FLEXPART particle
product, terrain height is added to the stored height before the ensemble mean
is calculated. The reported vertical difference is:

```text
Trajecta mean height ASL - FLEXPART mean height ASL
```

### Transport distance

Each centroid's great-circle distance from 5.25° E, 49.25° N is calculated.
The table reports the Trajecta distance minus the FLEXPART distance. A positive
value places the Trajecta centroid farther from the source at that output time.

### Horizontal dispersion

For each particle, the great-circle distance from its ensemble centroid is
calculated. The root mean square of those distances gives the horizontal
dispersion width. The published ratio is:

```text
Trajecta RMS dispersion / FLEXPART RMS dispersion
```

### Occupied audit cells

Coordinates are assigned to a diagnostic grid with 0.1° longitude, 0.1°
latitude, and 250 m height cells. Occupancy counts the distinct cells containing
at least one particle. The published value divides the Trajecta count by the
FLEXPART count.

## Published ensemble values

Formal repetitions produced the same scientific rows within each population,
so the median at each time equals the row shown below.

### 10,000 particles

| Elapsed time | Horizontal centroid separation | Vertical difference | Transport-distance difference | Dispersion ratio | Occupancy ratio |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 min | 214.89 m | -22.46 m | 214.68 m | 1.00116 | 0.98148 |
| 40 min | 229.48 m | -37.87 m | 214.22 m | 1.00191 | 1.01802 |
| 60 min | 276.55 m | -49.79 m | 175.45 m | 1.00373 | 0.98291 |

### 50,000 particles

| Elapsed time | Horizontal centroid separation | Vertical difference | Transport-distance difference | Dispersion ratio | Occupancy ratio |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 min | 60.66 m | -19.29 m | 30.45 m | 1.00288 | 0.98182 |
| 40 min | 143.45 m | -34.64 m | 22.25 m | 1.00344 | 0.99130 |
| 60 min | 271.88 m | -46.43 m | -20.33 m | 1.00514 | 0.97521 |

The 50,000-particle ensemble reduces the early centroid sampling difference;
the separation at 60 minutes is similar for the two population sizes. The
dispersion ratios remain close to one throughout this case, while the negative
vertical values place the Trajecta centroid modestly below the FLEXPART
centroid at the common times.

## Scope of the result

This comparison concerns a one-hour ERA5 hybrid-level advection case with the
settings in the table above. It uses ensemble statistics because the two
programs draw and integrate individual particles through different random
sampling and numerical paths. Particle ID `42` in one output has no assigned
counterpart in the other output.

Chemistry, wet or dry deposition, convection, turbulence configurations,
long-duration accumulation, other grids, and other release geometries require
their own aligned cases and metric choices. The
[comparability matrix](comparability.md) records which fields are exact,
aligned, aggregate-only, or outside this publication comparison.
