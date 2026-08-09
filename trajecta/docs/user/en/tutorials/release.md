---
title: Scheduled release tutorial
description: Run a deterministic point release with explicit event time, geometry, height, particle count, and tracer mass.
---

# Scheduled release tutorial

A release Case begins with a source event. The event states when particles are
born, where they are placed horizontally and vertically, how many are created,
and how much named substance mass they carry. This is the natural population
model for a known source, injection, observation site, or receptor experiment.

The supplied project creates 1,000 particles at longitude 0°, latitude 0° and
1,000 m above sea level. All particles are born at 06:00 UTC on 1 January 2009,
carry a share of one kilogram of a tracer named `water`, and move forward for
ten minutes through global CFSR meteorology.

## Read the Case

The complete scientific source comes directly from the example project:

--8<-- "examples/release-cfsr/cases/release.yaml"

The Case uses the same numerical step, global boundary policies, endpoint
output, and CFSR interval as the domain-fill example. Its population section is
different:

| Release field | Value in the example |
|---|---|
| Population ID | `release` |
| Event ID | `event` |
| Event interval | One instant at 2009-01-01 06:00 UTC |
| Particle count | 1,000 |
| Substance mass | 1 kg of `water` across the event |
| Horizontal geometry | Inline GeoJSON point `[0, 0]` |
| Vertical coordinate | Fixed 1,000 m above sea level |
| Random seed | 4201 |

The `substances` section gives the stable identifier `water` a readable label.
Result tables use the identifier as the particle-mass column key.

## Understand event allocation

### Time and mass

When `start` equals `end`, every particle has the same birth time. For an event
covering a nonzero interval, Trajecta assigns deterministic stratified birth
times across that interval. The event-local particle ordinal and Case seed make
the schedule independent of worker order.

Mass is divided by particle count for each named substance. In this tutorial,
the first 999 particles receive the ordinary floating-point share of 0.001 kg;
the final share compensates any rounding remainder so the stored particle
masses sum to the declared one kilogram.

Release particles use zero dry-air carrier mass. Their scientific payload is
stored in `particle_mass`, keyed by particle and substance.

### Geometry and vertical position

A `Point` places every particle at one longitude and latitude. Other supported
GeoJSON forms are `MultiPoint`, `LineString`, `MultiLineString`, `Polygon`, and
`MultiPolygon`. Sampling follows equal point weight, great-circle line length,
or spherical polygon area as appropriate. Geometries may be embedded in the
Case or loaded from a project-relative `.geojson` file.

Vertical placement supports height above sea level, height above local ground,
and pressure. A missing upper bound gives a fixed value. Supplying both lower
and upper bounds samples a closed interval. Above-ground and pressure releases
are resolved against meteorology at each particle's exact birth position and
time before transport begins.

## Prepare the shared CFSR data

The release project uses the same four-frame asset as the quickstart. Copy the
files into its local data root:

=== "Windows PowerShell"

    ```powershell
    Copy-Item .\examples\domain-fill-cfsr\data\* `
      .\examples\release-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    cp examples/domain-fill-cfsr/data/* examples/release-cfsr/data/
    ```

Then validate and inspect the requirement:

```text
trajecta --project examples/release-cfsr project validate
trajecta --project examples/release-cfsr project data-plan --output release-plan.json
python tools/fetch_trajecta_data.py --project examples/release-cfsr --plan release-plan.json
```

This population requests `transport` and `near_surface_transport`. Its initial
positions come from the release geometry, so the capability set ends there.

Create the DatasetLock and run the deep local check:

```text
trajecta --project examples/release-cfsr project finalize
trajecta --project examples/release-cfsr doctor --deep
```

The new `locks/cfsr.lock.json` belongs to this project even when the underlying
CFSR files were copied from or shared with the domain-fill project.

## Run the release

Submit the Profile in the foreground:

```text
trajecta --project examples/release-cfsr run --profile product
```

Use the reported result directory in the following commands:

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

With the supplied endpoint schedule, a complete run contains two output events.
All 1,000 particles are present at the initial event. Particles that remain
inside the vertical model bounds also appear at the final event. The inspect
summary shows the exact state-row count and any normal terminations.

## Read origin and tracer mass

### Population and mass records

Release particles have `origin_kind: release` and `origin_event_id: event`.
This origin remains attached to the particle after it moves away from the
source. The `particle_mass` table stores one `water` row per particle, while
`particle_state` stores time-varying position, transport fields, status, and
termination details.

The run report summarizes declared and represented substance mass. For a
multi-event Case, origin event IDs let an analysis group trajectories and mass
by release without inferring the event from a timestamp.

### Individual trajectories

Read several paths in one stream:

```text
trajecta result trajectory RESULT --particle-id 0 --particle-id 1 --particle-id 2
```

At the first event, the three particles share the same point and height. Their
stable IDs and mass rows remain distinct. The final rows show how the local
wind separates or co-locates them over the two integration steps.

For a larger selection, JSONL avoids building one large in-memory response:

```text
trajecta --format jsonl result trajectory RESULT --all
```

Redirect that stream to a file when an external analysis tool will consume the
complete result.

## Adapt the release design

### Add time or multiple events

An event interval can represent continuous release by setting distinct
`start` and `end` values. The declared particle count is distributed across
the interval with exact per-particle birth times. Set the simulation interval
to contain the relevant event times in the chosen direction.

Additional entries under `events` can describe separate source periods,
locations, substances, or heights. Give every event a stable unique ID. Each
event has its own particle count and mass map, while all events share the
population seed and Case numerics.

After changing event time, regenerate data-plan so meteorological coverage
includes every birth and the subsequent trajectory interval.

### Change geometry or vertical coordinates

Use a project-relative GeoJSON file for a detailed line or polygon. The resolved
run records both the source-file hash and a canonical geometry hash. For a
point or short coordinate list, inline geometry keeps the Case self-contained.

Choose `above_ground` when the intended height follows terrain and
`above_sea_level` for an absolute geometric altitude. Pressure placement is
useful when the source is naturally described on an isobaric surface. A lower
and upper value creates a vertical layer rather than a single release surface.

Particle count controls spatial and temporal sampling of the release. Total
substance mass remains the event value, so increasing particle count reduces
mass per particle. Output cadence and run duration then determine the number of
state rows produced by those particles.

## Next steps

The [air-mass tutorial](air-mass.md) returns to domain filling and introduces a
limited ERA5 region with data downloaded through the project plan. The
[population concepts](../concepts/populations.md) page compares release,
air-mass, and ozone lifecycles in one place.
