---
title: Data families and trajectory direction
description: Compare Trajecta's CFSR pressure, ERA5 pressure, and ERA5 hybrid profiles and plan forward or backward coverage.
---

# Data families and trajectory direction

A Case refers to a logical dataset such as `cfsr` or `era5-hybrid`. The project
index maps that name to a public dataset profile, and the RunProfile supplies
the local files. This indirection lets the numerical Case describe its physical
intent without embedding a provider directory or file naming convention.

Dataset profiles define more than a container format. They identify source
messages or variables, map units, describe frame cadence and vertical
coordinates, derive additional fields, and publish capability groups used by
the selected population and boundary model.

## Profile comparison

| Family | Public profile | Source container | Vertical coordinate | Frame cadence | Tutorial domain |
|---|---|---|---|---:|---|
| CFSR pressure | `cfsr-pgbl-pressure-v0` | GRIB2 | 37 isobaric levels, 1–1,000 hPa | 6 h | Global 144 × 73 grid |
| ERA5 pressure | `era5-cf-pressure-netcdf-v0` | NetCDF3 or NetCDF4 | 37 isobaric levels, 1–1,000 hPa | 6 h | 53–45° N, 0–10° E tutorial box |
| ERA5 hybrid | `era5-cds-hybrid137-v0` | Prepared NetCDF3 or NetCDF4 | 137 model levels with surface-pressure-dependent coordinates | 3 h | 53–45° N, 0–10° E tutorial box |

All three profiles expose transport, near-surface transport, domain-fill, and
diagnostics capabilities. Their source variables and preparation steps differ,
while the query engine presents a common field model to the integrator.

## CFSR pressure levels

The CFSR tutorial reads low-resolution global `pgbl` analysis files. Wind,
pressure vertical velocity, temperature, specific humidity, and geopotential
height are stored on pressure levels. Surface pressure, terrain, 10 m wind,
2 m state, boundary-layer quantities, and instantaneous fluxes provide the
surface companions.

Longitude is periodic on the global grid. A trajectory that crosses 180°
continues from the opposite edge, so the Case selects `global_periodic/v0`.
The six-hour source cadence requires time brackets around the simulation
interval. For the 06:00–06:10 quickstart, the data helper plans 00, 06, 12, and
18 UTC because it adds one neighboring frame on each side of the rounded
coverage.

The Rust reader consumes the original GRIB2 files directly. The native reader
uses packaged ecCodes. A DatasetLock records the selected profile, source
hashes, valid times, grid signature, pressure-level signature, and capability
set.

## ERA5 pressure levels

The ERA5 pressure tutorial requests six three-dimensional variables on 37
pressure levels. Surface requests add pressure, geopotential, near-surface
wind and thermodynamic fields, boundary-layer height, roughness, friction
velocity, and instantaneous turbulent fluxes.

The preparation tool produces files carrying the
`era5_cf_pressure_netcdf` family marker. The profile accepts prepared NetCDF3
and NetCDF4 containers and maps both to the same canonical fields. Geopotential
is converted to geopotential height; two-metre specific humidity is derived
from dew-point temperature and surface pressure; source flux signs are mapped
to Trajecta's upward-positive convention.

The tutorial box is finite. It uses `limited_domain_terminate/v0`, which ends a
particle at the continuous intersection with a horizontal domain boundary.
That outflow is recorded as a normal termination and reduces the number of
particles present at later output events.

## ERA5 hybrid model levels

The hybrid profile uses all 137 ERA5 model levels. Pressure at a model level
depends on the level coefficients and the local surface pressure. The
preparation pipeline therefore combines the three-dimensional model-level
fields with logarithmic surface pressure, surface companions, and the official
coefficient table.

The main three-dimensional request includes temperature, two wind components,
specific humidity, and pressure vertical velocity. The profile derives surface
pressure from logarithmic surface pressure. Potential vorticity is derived for
the diagnostics capability used by the ozone initializer.

Hybrid anchors are three hours apart. The backward ozone example spans 05:50
to 06:00 UTC and the helper plans 00, 03, 06, and 09 UTC. Its prepared files
retain the metadata needed to reconstruct the vertical coordinate at each
horizontal point and time.

## Forward and backward physical time

For a forward Case, `time.start` is earlier than `time.end`. The integrator
adds the signed time step, output events appear in increasing physical time,
and release events are encountered in that direction.

For a backward Case, `time.start` is later than `time.end`. The same positive
step magnitude is applied with a negative direction. Result records retain
their physical timestamps, while the event sequence reflects the order in
which the worker visited them.

Data-plan always writes `coverage_start` and `coverage_end` in chronological
order. The download helper then rounds that physical interval to the profile
cadence and adds surrounding anchors. Provider requests therefore look similar
for forward and backward runs even though the numerical execution direction is
different.

## Spatial coverage and halos

Each Case domain declares `horizontal_halo_cells`. The safe query region lies
inside the prepared grid by that number of cells, leaving interpolation support
near the edge. The global CFSR grid wraps in longitude. A limited ERA5 grid has
real northern, southern, eastern, and western boundaries.

The supplied ERA5 tutorials have no release geometry from which to infer an
area, so the helper uses the documented tutorial box: north 53°, west 0°,
south 45°, east 10°. A release Case with explicit coordinates produces a box
from that geometry with a two-degree acquisition halo. A global periodic Case
requests global coverage.

## Change a family or direction

Changing a filename in a Profile leaves several scientific choices unresolved.
A complete change includes:

1. map the logical dataset to the matching public dataset profile;
2. update the Case domain and boundary policy for the new spatial coverage;
3. regenerate data-plan and review its capability set, area, cadence, and time
   anchors;
4. prepare files in the family-specific layout;
5. finalize to create a new DatasetLock;
6. run the small Case before expanding its time or population.

Direction changes also affect scheduled releases and the interpretation of
event order. Keep `time.start`, `time.end`, and `direction` consistent, then
regenerate the plan. The [coverage and identity](../concepts/coverage-identity.md)
page describes how these choices enter the run identity.
