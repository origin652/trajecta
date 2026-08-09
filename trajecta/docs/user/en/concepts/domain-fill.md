---
title: Domain-fill water-vapor tracking
description: Understand equal-dry-air-mass sampling, limited-domain exchange, mass accounting, and moisture interpretation in Trajecta.
---

# Domain-fill water-vapor tracking

Domain filling constructs a Lagrangian representation of the air within a
meteorological domain. Particles are distributed by dry-air mass, so each
initial particle represents an equal share of the atmosphere rather than an
equal geometric volume. Their trajectories and carrier weights provide the
moving air-mass basis for water-vapor attribution and residence-time analysis.

The Case selects this population with `domain_fill_air_mass`:

```yaml
particle_population:
  strategy: domain_fill_air_mass
  id: moisture-air
  domain_id: limited
  target_particle_count: 50000
```

The referenced `domain_id` selects one meteorological domain from the Case.
Initialization and boundary exchange use that domain's safe core, vertical
support, and boundary geometry.

## From meteorology to dry-air mass

At the Case start time, Trajecta asks the meteorology engine for a domain-fill
snapshot. For each usable horizontal cell and vertical layer, the snapshot
combines:

| Input | Role in the mass representation |
| --- | --- |
| Horizontal coordinates | Spherical grid-cell area |
| Interface pressure | Hydrostatic layer mass |
| Temperature and specific humidity | Moist-air state and dry-air fraction |
| Geopotential or height | Vertical geometry and layer placement |
| Surface pressure and terrain | Local lower boundary |
| Available model top | Upper transport boundary |

The calculation retains dry air above the local transport floor and below the
available top. Humidity affects the conversion from total air state to dry-air
density. Cells outside the domain's safe support do not enter the initial mass
budget.

The resulting snapshot contains an ordered collection of mass strata plus the
total dry-air mass represented by those strata.

## Two ways to choose population resolution

A Case supplies exactly one of these fields:

| Field | Effect |
| --- | --- |
| `target_particle_count` | Creates that exact number of initial particles and divides total dry-air mass equally among them |
| `target_dry_air_mass_per_particle` | Uses the requested carrier mass; the initial count is the floor of total mass divided by that carrier mass |

Count mode is convenient for controlling memory and output size. Mass-per-
particle mode keeps the carrier quantum comparable across domains or seasons.
In the second mode, dry-air mass below one complete carrier remains as a
residual in the mass ledger.

The carrier value is written as `dry_air_mass_kg` on each particle. A particle
represents a weighted sample of the air mass; it is not assigned the entire
mass of the grid cell in which it happens to start.

## Equal-mass stratified placement

Trajecta orders the dry-air strata by grid column and layer, then partitions the
cumulative mass axis into equal intervals. One particle is placed within every
interval. The draw selects:

1. a dry-air mass coordinate within the interval;
2. the column and layer containing that coordinate;
3. longitude within the column;
4. latitude uniformly in spherical area through a uniform draw in sine of
   latitude;
5. pressure within the hydrostatic layer, followed by log-pressure placement
   in geometric height.

Where terrain or the complete transport floor varies across a grid cell, the
sample is relocated into the local valid vertical range. This avoids placing a
particle below the transport floor or above local data support.

The Case random seed, population ID, domain ID, particle identity, lifecycle
event, and sampling dimension form deterministic random keys. Worker count and
thread scheduling therefore do not change the initial sample.

## What the population represents

At initialization, the represented dry-air mass is:

```text
sum(initial particle carrier mass) + initial residual mass
```

In count mode, the initial residual is zero. Increasing the particle count
reduces the carrier mass and gives spatial aggregates more Lagrangian samples.
It also increases meteorological queries, particle-state rows, provenance
assignments, and worker memory.

Specific humidity participates in construction of the dry-air snapshot and in
meteorological queries along the path. The particle-state product supplies
positions, times, carrier mass, and selected meteorological state. Moisture
analyses can group those weighted trajectories by source region, receptor
region, crossing time, or residence interval and combine them with the
humidity fields appropriate to the study.

## Global-domain lifecycle

A global periodic domain has no lateral inflow or outflow edge. Longitude wraps
through the selected periodic boundary policy. Particle count generally stays
fixed until another declared boundary rule terminates a particle, for example
at the model top. Surface contact is handled by the configured reflection
policy.

The global CFSR quickstart uses endpoint output and a short interval, so its
initial and final states show the basic population without lateral exchange.

## Limited-domain inflow

A limited domain has four lateral faces divided by vertical layers. At each
macro step, Trajecta evaluates inward dry-air mass flux using the face area,
dry-air density, and wind normal to the boundary. Forward and backward runs
interpret the inward normal according to their physical integration direction.

Incoming mass accumulates separately for each boundary face. Whenever it
reaches one full carrier mass, a particle is born at the exact threshold time.
Its origin records the domain and boundary-face ID. Tangential and vertical
coordinates are sampled deterministically within that face layer.

Mass below the next carrier threshold remains as boundary residual and carries
into the following macro step. This keeps a finite particle representation
aligned with continuous dry-air flux.

## Limited-domain outflow

When an active particle crosses the selected limited-domain boundary, the
continuous boundary solver locates the crossing along the proposed path. The
particle terminates at that physical intersection with reason
`population_outflow`, classified as a normal termination.

Outflow can reduce particle count even when an equal mass enters during the
same step, because incoming dry-air mass is quantized into complete carrier
particles and residuals. Particle count is therefore a sampling diagnostic;
the mass ledger is the conservation record.

## Per-step mass ledger

For every completed macro step, Trajecta records these quantities:

| Quantity | Meaning |
| --- | --- |
| `opening_active_kg` | Carrier mass on live particles at step start |
| `opening_residual_kg` | Initial and boundary residual carried into the step |
| `incoming_kg` | Dry-air mass entering through limited-domain faces |
| `outgoing_kg` | Carrier mass assigned to population outflow |
| `normal_terminated_kg` | Carrier mass ending under another declared normal rule |
| `abnormal_terminated_kg` | Carrier mass associated with an abnormal particle termination |
| `closing_active_kg` | Carrier mass on live particles at step end |
| `closing_residual_kg` | Remaining sub-carrier mass after births |

The ledger evaluates:

```text
opening active + opening residual + incoming
  = outgoing + normal terminated + abnormal terminated
    + closing active + closing residual
```

The manifest records the observed imbalance and its numerical tolerance for
each step. Full result verification repeats the ledger checks and also evaluates
the accumulated final balance.

## Normal and abnormal terminations

Termination classification matters when reading population history:

| Class | Examples | Interpretation |
| --- | --- | --- |
| Normal | Population outflow, model-top rule | A declared physical lifecycle path |
| Abnormal | Invalid meteorology, numerical boundary failure | A particle could not continue under the selected model |

A terminal run with any abnormal particle termination has status
`completed_with_particle_errors`. The affected particle and its final state
remain in the result. Use `result inspect` to read counts by reason and
`result trajectory` to examine its path.

## Forward and backward domain filling

Forward integration follows the initialized air mass toward later physical
times. Backward integration follows it toward earlier times. Initial sampling
still occurs at `time.start`, and the clock advances in the Case direction.
Boundary inflow, outflow, birth time, elapsed age, and event ordering are all
defined relative to that direction.

For receptor-oriented moisture work, a backward Case can fill a regional air
mass at the receptor time and trace its carrier trajectories into earlier
meteorology. A forward Case can follow a source-period air mass toward later
regions. Comparable experiments keep the domain, population resolution,
meteorological preparation, integration step, and output schedule explicit.

## Choosing particle count and output cadence

Population size controls sampling density; output cadence controls the temporal
resolution of stored paths. They affect different parts of the analysis.

For an initial study:

1. Run a short, low-count Case and inspect the spatial distribution.
2. Confirm mass-ledger closure and termination reasons.
3. Choose an output interval that resolves the crossings or residence periods
   of interest.
4. Increase particle count until the target aggregate is stable at the study's
   spatial scale.
5. Record count, carrier mass, seed, integration step, and output schedule with
   the analysis.

The [domain-fill tutorial](../tutorials/domain-fill.md) follows this sequence
with the compact CFSR project.
