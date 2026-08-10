---
title: ERA5 hybrid-level ozone tutorial
description: Prepare ERA5 hybrid model-level data and run a backward stratospheric-ozone population with Trajecta.
---

# ERA5 hybrid-level ozone workflow

The ozone project combines three features introduced separately in earlier
tutorials. It fills a meteorological domain by dry-air mass, assigns a carried
substance at particle birth, and integrates backward from a later receptor time
to an earlier state. Its meteorology uses all 137 ERA5 hybrid model levels.

The example starts at 06:00 UTC on 1 December 2018 and runs backward to 05:50
UTC. It initializes exactly 1,000 particles inside the stratospheric region
selected by the PV60 rule. The data helper uses the same 53–45° N, 0–10° E box
as the pressure-level tutorial.

## Read the project and Profile

The project index maps logical dataset `era5-hybrid` to public profile
`era5-cds-hybrid137-v0`:

```yaml
--8<-- "examples/ozone-era5-hybrid/trajecta-project.yaml"
```

The machine-facing Profile selects the Rust reader and local paths:

```yaml
--8<-- "examples/ozone-era5-hybrid/profiles/product.yaml"
```

The Case is stored at `examples/ozone-era5-hybrid/cases/ozone.yaml`. Its key
settings are:

| Case choice | Tutorial value |
|---|---|
| Physical start | 2018-12-01 06:00 UTC |
| Physical end | 2018-12-01 05:50 UTC |
| Direction | Backward |
| Domain | Limited ERA5 hybrid grid |
| Initial population | 1,000 dry-air carriers inside the PV60 eligibility mask |
| Carried substance | `ozone` |
| Time step | 300 seconds |
| Boundaries | Surface reflection, model-top termination, limited-domain termination |
| Output | Both physical endpoints in SQLite |
| Random seed | 4203 |

The RunProfile requests one worker thread and 1 GiB of memory. Its lock path is
`locks/era5-hybrid.lock.json`, and prepared files are discovered below
`data/`.

## Understand the hybrid coordinate

ERA5 model levels follow the atmosphere and terrain. Pressure on level `k` is
reconstructed from the level's coefficients and local surface pressure. The
vertical coordinate therefore changes with horizontal position and time; a
standalone three-dimensional variable file is only one part of a complete
anchor.

The preparation pipeline assembles these inputs:

| Input group | Contents |
|---|---|
| Hybrid three-dimensional fields | Temperature, eastward and northward wind, specific humidity, pressure vertical velocity on levels 1–137 |
| Surface-pressure input | Logarithmic surface pressure used to derive canonical surface pressure |
| Surface base fields | Geopotential, 10 m wind, 2 m temperature and dew point, roughness, boundary-layer height, friction velocity |
| Surface flux fields | Instantaneous sensible-heat and moisture flux |
| Vertical-coordinate metadata | Official half-level coefficient table and preparation identities |

The profile derives surface pressure and near-surface humidity, maps flux
signs, and calculates potential vorticity for the `diagnostics` capability.
Hybrid frames are three hours apart. For the 05:50–06:00 physical interval,
the helper plans 00, 03, 06, and 09 UTC.

## Understand PV60 initialization

The named ozone rule first restricts dry-air mass to locations above 3,000 m
where hemisphere-normalized potential vorticity is greater than 2
potential-vorticity units. Southern-hemisphere values are sign-normalized
before applying the threshold.

Within the eligible dry-air mass, Trajecta forms exactly 1,000 equal carrier
strata and samples one particle from each. Ozone mole fraction is proportional
to potential vorticity with a slope of 60 parts per billion by volume per
potential-vorticity unit. The rule then converts mole fraction to carried ozone
mass with the ozone and dry-air molar masses.

This produces two linked quantities in the result. `dry_air_mass_kg` is the
carrier weight for the eligible atmosphere, while the `ozone` row in
`particle_mass` is the substance mass assigned at birth. Transport moves that
carried mass with the particle. The population mass ledger follows dry-air
carrier mass through limited-domain inflow, outflow, and terminations.

## Plan data before downloading

Validate the project and save its data-plan:

```text
trajecta --project examples/ozone-era5-hybrid project validate
trajecta --project examples/ozone-era5-hybrid project data-plan --output hybrid-plan.json
```

The single requirement selects:

| Field | Expected value |
|---|---|
| Dataset | `era5-hybrid` |
| Dataset profile | `era5-cds-hybrid137-v0` |
| Reader | `rust` |
| Capabilities | `diagnostics`, `domain_fill`, `near_surface_transport`, `transport` |
| Coverage | 2018-12-01 05:50–06:00 UTC in chronological order |
| Lock path | `locks/era5-hybrid.lock.json` |

Coverage remains chronological in the plan even though numerical execution is
backward. The direction stays in the resolved Case.

Install the data-tool dependencies and preview the request:

```text
python -m pip install -r requirements-data.txt
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json
```

The preview lists each Climate Data Store request, model levels, variable
codes, surface companions, area, anchors, and local target. Credentials are
read by the provider client from its standard configuration or environment;
the printed plan contains request parameters only.

## Download and prepare anchors

Start the complete provider and preparation pipeline:

```text
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json --execute
```

Raw service files and derived coefficient material are assembled under
`data/.trajecta-fetch/`. Preparation checks dimensions, variables, valid times,
surface companions, level ordering, and coefficient identity. Completed
NetCDF anchors move into `data/ready/`, and the helper writes
`data/TRAJECTA_FETCH_MANIFEST.json` with request and file identities.

Hybrid requests are larger and more involved than the pressure-level tutorial.
The provider may stage the model-level request asynchronously. Re-running the
same command reuses matching work already present in the provider utilities.

## Finalize and run

Inspect one ready anchor, create the lock, and check local I/O:

```text
trajecta data inspect examples/ozone-era5-hybrid/data/ready/ERA5_HYBRID_READY_FILE.nc
trajecta --project examples/ozone-era5-hybrid project finalize
trajecta --project examples/ozone-era5-hybrid doctor --deep
```

The inspection response identifies `era5_cds_hybrid137`, 137 model levels,
valid times, source roles, and the prepared grid. Finalization brings the main
fields, logarithmic surface pressure, surface files, and coefficient metadata
together in one DatasetLock.

Run the Profile in the foreground:

```text
trajecta --project examples/ozone-era5-hybrid run --profile product
```

Population initialization reads the 06:00 UTC snapshot, derives potential
vorticity, selects eligible dry-air strata, and assigns ozone. The worker then
steps toward 05:50 UTC and writes the endpoint result.

## Read a backward result

Verify and summarize the completed directory:

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

Event sequence follows execution: the first scheduled event is 06:00 and the
second is 05:50 UTC. `integration_offset_ns` becomes negative as the trajectory
moves into earlier physical time. `elapsed_age_ns` remains non-negative and
describes how long the particle has been integrated since its 06:00 birth.

The particle summary separates total dry-air carrier mass from total ozone
mass. Origins identify the population and domain. A boundary-intersection
termination carries its exact physical time, which may fall between the two
scheduled endpoints.

For analysis, particle records can be streamed in JSONL and joined by
`particle_id` with the `ozone` mass rows in SQLite. Grouping by final position,
initial position, or termination face provides different views of the backward
transport over the chosen interval.

## Extend the ozone study

Longer backward periods begin by moving `time.end` earlier. Regenerate
data-plan so three-hour anchors cover the expanded chronological interval, then
prepare and finalize the additional files. Endpoint output can be replaced by
an interval schedule when intermediate transport is needed.

Increasing `target_particle_count` reduces sampling noise within the eligible
stratospheric mass and lowers the carrier mass represented by each particle.
It also raises the cost of potential-vorticity sampling, trajectory integration,
and SQLite output. Profile memory and worker threads can be changed separately
from the scientific Case.

When moving the region, prepare a new hybrid grid with its surface companions
and coefficient metadata. The limited-domain boundary is defined by that ready
grid, so outflow counts and inflow births acquire a new spatial meaning.

## Next steps

The [population concepts](../concepts/populations.md) compare the three
initialization strategies. The [data-family page](data-families.md) explains
why hybrid and pressure-level anchors use different cadences and vertical
signatures. Scientific comparison material is collected separately under
[Validation](../validation/index.md).
