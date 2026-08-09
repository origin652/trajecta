---
title: FLEXPART comparability matrix
description: See which Trajecta and FLEXPART inputs, coordinates, outputs, ensemble measures, and runtime components share a comparison boundary.
---

# FLEXPART comparability matrix

A trajectory comparison begins by matching the physical quantity represented
by each field. File format, variable name, vertical convention, output cadence,
and particle sampling can differ even when two products describe atmospheric
transport. The matrix below records the relationship used by the published
one-hour case.

## Classification terms

| Classification | Meaning in this comparison |
| --- | --- |
| `exact` | Both sides select the same source quantity or contract value, and equality is checked directly |
| `aligned` | Values share units and physical interpretation after a declared conversion or packing tolerance |
| `aggregate-only` | A statistic of the particle ensemble is compared; individual records are not paired |
| `not-comparable` | The products do not expose a shared quantity or cost boundary in this matrix |

These terms apply per row. One run can contain exact input times, aligned
heights, aggregate-only particle statistics, and implementation-specific output
work at the same time.

## Matrix

| Dimension | Classification | Alignment used |
| --- | --- | --- |
| Official meteorological source files | `exact` | Both staging paths begin with the same frozen source bytes |
| Trajecta prepared meteorology | `exact` | Prepared arrays are compared with their source arrays; derived surface pressure uses bounded round-off |
| FLEXPART staged GRIB values | `aligned` | Each common field stays within the ecCodes packing error of its source value |
| Valid times | `exact` | Four source frames and their UTC instants are fixed |
| Hybrid vertical definition | `aligned` | Model levels, half-level coefficients, pressure interpretation, and units are matched |
| Wind, temperature, humidity, and common surface fields | `aligned` | Common physical fields use the same quantity and unit after staging |
| Output instants | `exact` | Both products contain 20, 40, and 60 minute states |
| Particle count | `exact` | Each population contains 10,000 or 50,000 particles as selected |
| Released mass contract | `exact` | The shared case uses equal total released mass and population size |
| Longitude and latitude | `aligned` | Geographic degrees are converted to a common spherical metric calculation |
| Height | `aligned` | Both sides enter ensemble metrics as metres above sea level |
| Ensemble centroid | `aggregate-only` | Great-circle separation is calculated between spherical ensemble means |
| Horizontal dispersion | `aggregate-only` | Root-mean-square great-circle distance from each ensemble centroid is compared as a ratio |
| Spatial occupancy | `aggregate-only` | Distinct cells on a common 0.1° × 0.1° × 250 m audit grid are compared as a ratio |
| Individual particle trajectory | `not-comparable` | Random source sampling and integrators do not create a shared particle-ID assignment |
| SQLite and provenance work | `not-comparable` | The FLEXPART product in this matrix has no matching indexed history and provenance product |
| Particle NetCDF layout | `not-comparable` | Trajecta's default product is SQLite and has a different schema and lifecycle |

## Meteorological value alignment

The meteorological conversion has two routes:

```text
official source → Trajecta-ready NetCDF → Trajecta reader
official source → staged GRIB → FLEXPART reader
```

Exact Trajecta-ready checks include the coordinate axes, valid times, hybrid
coefficients, and required four-dimensional fields. FLEXPART's staged values
are read back from GRIB and compared with the common source quantities. GRIB
packing error supplies the per-message tolerance for that route.

Two zero precipitation messages are present to satisfy the selected FLEXPART
input structure while wet deposition and convection are disabled. Trajecta
surface-flux fields remain outside the common advection quantity set. Their
presence does not add a cross-model metric to this case.

## Time alignment

The simulation starts at 03:00 UTC on 2018-12-01. Both products are read at:

| Elapsed time | UTC instant | Unix time |
| ---: | --- | ---: |
| 20 minutes | 2018-12-01 03:20:00 UTC | 1543634400 |
| 40 minutes | 2018-12-01 03:40:00 UTC | 1543635600 |
| 60 minutes | 2018-12-01 04:00:00 UTC | 1543636800 |

The programs may perform internal work at other points inside a 600-second
transport step. The comparison reads only these common physical output states.

## Coordinate alignment

Longitude and latitude enter a spherical mean and great-circle distance
calculation. Height enters as metres above sea level. FLEXPART's stored particle
height is combined with its terrain field before the vertical mean is taken;
Trajecta already stores `height_asl_m` in that convention.

The occupancy grid is a diagnostic binning scheme used only for this
comparison. It does not replace either program's computational grid and does
not alter either trajectory.

## Why individual particle IDs are not paired

The two programs initialize the same population count and total mass, while
their random generators and source-sampling order differ. Their integrators
then advance those samples through separate numerical implementations. A
numeric particle ID therefore identifies a record inside its own product.

Ensemble centroid, dispersion, vertical mean, transport distance, and occupancy
remain well-defined after the IDs are set aside. These measures describe the
location and spread of the population at the shared times.

## Runtime comparability

The [performance method](performance.md) provides one transport-oriented
boundary and one complete-product boundary. The equivalent-core ratio compares
the shared transport question. Complete-product times show the operational cost
of each program's selected default output, with their product differences left
visible.

SQLite and provenance time has no matched FLEXPART component in this matrix.
For this reason, the complete-product columns are descriptive values rather
than an equivalence ratio.

## Extending the matrix

A new comparison contract is appropriate when a study changes any of the
following:

- simulation duration or transport step;
- meteorological family, resolution, or vertical coordinate;
- release geometry and particle sampling;
- turbulence, convection, chemistry, or deposition settings;
- output cadence or product format;
- worker count, CPU allocation, or host platform.

The new contract can reuse the classifications above while defining the exact
fields, conversions, times, metrics, and timing boundaries for its workload.
Raw values from the current matrix are available on the
[data and charts page](data.md).
