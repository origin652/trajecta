---
title: Domain-fill moisture tutorial
description: Build, run, and interpret a CFSR domain-fill air-mass project for Lagrangian moisture tracking.
---

# Domain-fill moisture tutorial

Domain filling represents the atmosphere inside a meteorological domain with
particles of equal dry-air carrier mass. Denser parts of the atmospheric
column receive more particles than lighter parts, while the geographic sample
remains tied to the source grid and vertical coordinate. The resulting
particle histories form the Lagrangian air-mass basis used in a moisture
tracking study.

This tutorial uses the same project as the fifteen-minute quickstart and then
looks more closely at its population, mass ledger, output schedule, and result
tables. The Case runs 1,000 particles on global CFSR pressure-level data from
06:00 to 06:10 UTC on 1 January 2009.

## What this Case represents

At the initial time, Trajecta constructs a dry-air snapshot from pressure,
temperature, specific humidity, grid-cell area, terrain, and the available
vertical layers. The snapshot is divided into 1,000 equal mass strata. One
particle is sampled within each stratum with the Case seed, giving every
particle the same dry-air carrier mass.

This procedure has two practical consequences. First, counting particles is
also a mass-weighted view of the represented atmosphere. Second, a fixed
particle count controls computational cost while the carrier mass adapts to the
total dry-air mass of the selected domain.

During integration, each particle follows the three-dimensional wind. Surface
crossings use the reflection policy, crossings above the available model top
terminate, and longitude wraps around the global grid. A mass-ledger record is
written for each macro step so active mass, boundary exchange, terminations,
and residual mass remain connected to the initialized domain.

## Read the project

The project index names the Case `moisture` and Profile `product`. The Profile
binds logical dataset `cfsr` to `data/`, selects the Rust reader, writes under
`runs/`, and requests one worker thread with a 1 GiB budget.

The scientific Case is included below from the executable example:

--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"

The main choices are easier to read as a study description:

| Case section | Meaning in this tutorial |
|---|---|
| `time` | Ten-minute forward interval beginning at 06:00 UTC |
| `meteorology.domains` | One global CFSR domain with a one-cell interpolation halo |
| `particle_population` | Exactly 1,000 dry-air domain-fill particles |
| `numerics.time_step` | Two 300-second trajectory steps |
| `numerics.integrator` | Spherical second-order Runge–Kutta integration |
| `boundaries.policies` | Surface reflection, model-top termination, periodic longitude |
| `random_seed` | Stable population and stochastic sampling key |
| `outputs` | Particle state at both physical endpoints, stored in SQLite |

The Case contains no operating-system path. The dataset name `cfsr` is resolved
through the project index and local Profile.

## Prepare data and finalize

Place the four demonstration frames under
`examples/domain-fill-cfsr/data/` as described on the
[demonstration data page](../getting-started/demo-data.md). Then inspect the
project state and write a data-plan:

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --project examples/domain-fill-cfsr project data-plan --output data-plan.json
```

The requirement selects `cfsr-pgbl-pressure-v0`, reader `rust`, and the
`transport`, `near_surface_transport`, and `domain_fill` capabilities. Its
physical coverage is 06:00–06:10 UTC; the helper expands that interval to the
four six-hourly acquisition anchors.

Preview the local data states and create the lock:

```text
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
trajecta --project examples/domain-fill-cfsr project finalize
trajecta --project examples/domain-fill-cfsr doctor --deep
```

Finalization writes `locks/cfsr.lock.json`. It includes the four source hashes,
valid times, grid and pressure-level signatures, selected profile, and
capabilities. Deep doctor then opens the locked data and exercises the result
filesystem before a worker is started.

## Run the Profile

Submit the default `product` Profile in the foreground:

```text
trajecta --project examples/domain-fill-cfsr run --profile product
```

The terminal shows the queue transition and the final result location. Keep the
reported path as `RESULT` for the commands below. A complete run normally
finishes in seconds with this dataset, though the first access can take longer
when the operating system has not cached the GRIB2 files.

Create a readable report after the worker has reached its terminal state:

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

The supplied ten-minute Case has two scheduled output events. With no early
termination, 1,000 particles produce 2,000 rows in `particle_state`. The
inspection summary also reports population origins, active and terminated
counts, mass totals, reader choice, output files, and quality categories.

## Read the population

### Start with the summary

`result inspect` separates the number of particles from the number of state
rows. A particle is created once in the `particle` table and may have one state
for every scheduled output event it survives. Domain-fill particles carry
`origin_kind: domain_initial` at the first event. A longer limited-domain run
may also contain particles born at an inflow boundary.

The summary's termination counts distinguish normal lifecycle outcomes from
abnormal execution outcomes. On the global tutorial grid, longitude crossings
wrap around. Normal terminations can still occur at the model top or through a
population outflow decision. Invalid meteorology and exhausted reflection
handling appear in the abnormal group and are useful starting points for a
failed run investigation.

### Follow one particle

Read particle 0 in human format:

```text
trajecta result trajectory RESULT --particle-id 0
```

The two records show birth identity, physical time, position, integration
offset, elapsed age, particle status, sampled wind, pressure, temperature, and
field quality. The second row's longitude, latitude, and height describe the
displacement produced by two numerical steps.

For analysis software, stream JSONL:

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

The stream begins with a header, emits one item per state row, and ends with a
summary. Multiple `--particle-id` options can be supplied in one command.

## Interpret mass and lifecycle

Each domain-fill particle stores `dry_air_mass_kg`. For a target-count Case,
all initial particles use the same carrier value. Their compensated sum plus
any residual represents the dry-air mass calculated from the meteorological
snapshot.

The manifest's mass ledger follows that representation through each macro
step. The useful columns for a longer run are:

| Quantity | Interpretation |
|---|---|
| Initial or incoming mass | Dry-air carrier mass introduced at initialization or a boundary |
| Active mass | Carrier mass still represented by live particles |
| Outgoing mass | Mass leaving through the domain-fill boundary process |
| Normal terminated mass | Carrier mass ending through a declared physical lifecycle rule |
| Abnormal terminated mass | Carrier mass associated with a particle error |
| Residual mass | Dry-air amount smaller than one carrier quantum when mass-per-particle mode is used |

Specific humidity participates in the dry-air snapshot and meteorological
queries. The particle-state product supplies the trajectories and carrier
weights used to organize later moisture diagnostics by source region, receptor
region, or time interval.

## Extend the experiment

### Extend physical time

Change `time.end` in `cases/moisture.yaml`, then run `project validate` and
regenerate data-plan. The helper's acquisition anchors will move to cover the
new interval. Once the additional frames are present, finalization creates an
updated lock for the expanded Case.

For a multi-hour study, endpoint output only shows the start and finish. Select
an interval schedule in the Case when intermediate positions are needed. The
number of state rows is approximately the surviving particle count multiplied
by the number of scheduled events, so output cadence has a direct effect on
result size.

### Increase population or change resolution

Raise `target_particle_count` to reduce sampling noise in spatial aggregates.
The dry-air carrier mass per particle falls as the count rises. Worker memory,
meteorological query volume, and SQLite output all grow with the population.

Keep the ten-minute 1,000-particle project as a local preflight. Create a new
Profile when changing worker threads, memory budget, reader, or output root.
That preserves the scientific Case while making the execution environment
visible in the result.

## Next steps

The [scheduled release tutorial](release.md) replaces mass-weighted domain
initialization with an explicit point source. The
[air-mass tutorial](air-mass.md) keeps domain filling and moves it to a finite
ERA5 pressure-level region, where boundary outflow becomes part of the normal
lifecycle.
