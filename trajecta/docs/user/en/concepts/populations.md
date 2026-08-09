---
title: Particle population models
description: Compare scheduled release, dry-air domain-fill, and stratospheric-ozone populations in Trajecta.
---

# Particle population models

A particle population defines how particles are born, what mass they carry,
how their stable identities are constructed, and which meteorological
capabilities are required before trajectory integration starts. A Case selects
one complete population strategy.

Trajecta `0.1.0-alpha.1` provides three built-in strategies:

| Strategy | Initial or scheduled source | Particle weight or mass | Common study shape |
| --- | --- | --- | --- |
| `release_driven` | Explicit release events | Declared substance mass divided over event particles | Source-to-receptor transport and receptor releases |
| `domain_fill_air_mass` | Dry-air mass within one meteorological domain | Equal dry-air carrier mass | Moisture attribution, residence time, and air-mass transport |
| `domain_fill_stratospheric_ozone` | Eligible dry-air mass within one domain | Dry-air carrier plus derived ozone mass | Stratospheric ozone transport |

All three use the same spherical trajectory integrator, meteorological query
engine, boundary policies, output schedule, SQLite sink, and result lifecycle.
Their initialization and carried mass are different.

## Release-driven populations

A release population contains one or more named events:

```yaml
particle_population:
  strategy: release_driven
  id: release
  events:
    - id: source-a
      start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
      end: { seconds_since_unix_epoch: 1230793200, nanosecond: 0 }
      particle_count: 1000
      mass:
        water: { value: 10, unit: kg }
      geometry:
        source: inline
        geometry: { type: Point, coordinates: [8.5, 47.4] }
      vertical:
        coordinate: above_ground
        lower: { value: 100, unit: m }
        upper: { value: 500, unit: m }
```

Each event defines an inclusive time interval, exact particle count, horizontal
geometry, vertical coordinate, and total mass for each declared substance.

### Birth time

An instantaneous event has equal `start` and `end`; all its particles are born
at that time. For an interval event, the interval is divided into one integer-
nanosecond stratum per particle. One deterministic birth time is sampled within
each stratum. This spreads births over the requested period without relying on
thread order.

The integrator starts every cohort at its own exact birth time. A particle born
inside a macro step receives only the remaining part of that step.

### Horizontal and vertical placement

Release geometry accepts Point, MultiPoint, line, multiline, polygon, and
multipolygon GeoJSON forms. Geometry can be inline or loaded from a local file
relative to the Case. Points are equally weighted; lines use geodesic length;
polygons use spherical area while respecting holes.

Vertical placement can use height above mean sea level, height above local
ground, or atmospheric pressure. A single lower value gives a fixed level; an
upper value adds a uniform interval in the named coordinate. Above-ground and
pressure releases are resolved through the selected meteorological state.

### Substance mass

The total mass declared for each event is divided over its exact particle
count. The last particle receives a compensated floating-point share so that
the stored shares reconstruct the event total. Release particles have
`dry_air_mass_kg = 0` and carry their declared substances in `particle_mass`.

Multiple events may use the same population and substance definitions. Event
IDs remain unique and become part of particle origin and stable identity.

## Dry-air domain-fill populations

The air-mass strategy creates particles from one meteorological domain:

```yaml
particle_population:
  strategy: domain_fill_air_mass
  id: regional-air
  domain_id: limited
  target_particle_count: 50000
```

Initialization derives dry-air mass from the meteorological grid and places
equal-carrier-mass particles through deterministic stratified sampling. A Case
chooses an exact initial particle count or a target dry-air mass per particle.

Limited domains add particles when inward boundary flux accumulates one carrier
mass and terminate particles that leave the domain. Global periodic domains do
not have lateral exchange. A per-step mass ledger follows active, incoming,
outgoing, terminated, and residual dry-air mass.

The [domain-fill concept](domain-fill.md) describes this lifecycle and its
moisture interpretation in detail.

## Stratospheric-ozone domain fill

The ozone strategy wraps a dry-air domain-fill specification. Its `air_mass`
field supplies the population ID, domain, and target count or carrier mass.
`ozone_rule` selects the named built-in assignment rule, while
`ozone_substance` identifies the Case substance column that receives derived
ozone mass. The shipped ozone example contains the complete configuration.

The strategy first derives the dry-air snapshot, then restricts initialization
to the stratospheric eligibility mask used by that rule. The current rule
requires geometric height above 3,000 m and hemisphere-normalized potential
vorticity above 2 potential-vorticity units.

Within the mask, each particle carries equal eligible dry-air mass. Ozone mole
fraction is derived at 60 parts per billion by volume per potential-vorticity unit and converted to
ozone mass using the dry-air carrier mass and molar-mass ratio. The derived
ozone mass is stored under `ozone_substance`.

Limited-domain births are restricted to eligible portions of inflow faces and
receive ozone mass from the same rule at their birth location. The dry-air mass
ledger remains the population-conservation account; ozone mass is a carried
substance.

This strategy requires the domain-fill capability plus the diagnostic fields
needed for potential vorticity. The ERA5 hybrid example provides the complete
137-level input path used by this workflow.

## Stable particle identity

Particle IDs are deterministic and independent of worker scheduling. Their
inputs depend on the population strategy:

| Origin | Identity ingredients |
| --- | --- |
| Release | Population ID, event ID, and event-local ordinal |
| Initial domain fill | Population ID, domain ID, and mass-stratum ordinal |
| Boundary domain fill | Population ID, domain ID, boundary-face ID, lifecycle event index, and birth ordinal |

The numerical particle ID is stored with origin details in SQLite. A rerun with
identical resolved scientific inputs produces the same stable particle
identities even though the run ID and attempt are new.

## Population and direction

Direction belongs to the Case time specification. The population establishes
particles at the Case start or at scheduled birth times, then the integrator
moves toward the end in the selected direction.

For release events, event times lie within the Case's physical interval. For
domain filling, boundary inward and outward flux are interpreted using the
integration direction. The same limited-domain face can therefore act as
inflow in one direction and outflow in the other.

## Meteorological capability requirements

Every population needs transport fields. Additional requirements are derived
from the strategy:

| Population | Additional capability |
| --- | --- |
| Release-driven | Geometry/vertical resolution fields required by the selected release coordinates |
| Dry-air domain fill | Domain-fill snapshot fields for mass, terrain, vertical support, and boundary flux |
| Ozone domain fill | Domain-fill fields plus potential-vorticity diagnostics |

`project data-plan` writes the resulting capability set. `project finalize`
checks it against the selected dataset profile and inspected files before
creating a DatasetLock.

## Choosing a strategy

Choose from the physical object that needs to be represented:

- A known source geometry and release period maps naturally to
  `release_driven`.
- A regional or global body of air, weighted by atmospheric mass, maps to
  `domain_fill_air_mass`.
- A stratospheric air-mass population carrying the built-in ozone proxy maps to
  `domain_fill_stratospheric_ozone`.

Population changes alter particle origin, carrier semantics, required
meteorology, and scientific identity. Generate a new data plan and finalize the
project again after changing the strategy or its domain.
