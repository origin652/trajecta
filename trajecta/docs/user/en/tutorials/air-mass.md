---
title: ERA5 pressure-level air-mass tutorial
description: Prepare official ERA5 pressure-level data and run a limited-domain dry-air population with Trajecta.
---

# ERA5 pressure-level air-mass workflow

This tutorial keeps the equal dry-air carrier population introduced by the
CFSR domain-fill project and moves it to a limited ERA5 pressure-level grid.
The change introduces three practical topics: planning a provider request from
an unfinished project, preparing pressure and surface files together, and
interpreting normal particle outflow at a finite horizontal boundary.

The Case runs from 06:00 to 06:10 UTC on 1 December 2018. The supplied data
helper uses a tutorial box from 53° N to 45° N and 0° E to 10° E. One thousand
particles are initialized by dry-air mass inside the safe core of that grid.

## Read the project binding

The project index connects logical dataset `era5-pressure` with public profile
`era5-cf-pressure-netcdf-v0`:

--8<-- "examples/air-mass-era5-pressure/trajecta-project.yaml"

The Case selects the limited domain and dry-air population:

--8<-- "examples/air-mass-era5-pressure/cases/air-mass.yaml"

The RunProfile contains the machine-facing paths and execution request:

--8<-- "examples/air-mass-era5-pressure/profiles/product.yaml"

The three files divide the study cleanly. The Case supplies time, direction,
population, boundary rules, and output. The index selects the dataset profile.
The Profile points to `data/`, `locks/era5-pressure.lock.json`, and `runs/`,
then requests one Rust-reader worker with a 1 GiB memory budget.

## Understand the ERA5 request

The pressure-level profile has a six-hour frame cadence. For the ten-minute
Case, the helper rounds physical coverage and plans 00, 06, 12, and 18 UTC on
1 December 2018. Each date produces three provider requests:

| Request | Variables and role |
|---|---|
| Pressure levels | Temperature, two horizontal wind components, pressure vertical velocity, specific humidity, and geopotential on 37 levels from 1 to 1,000 hPa |
| Surface base | Surface pressure, geopotential, 10 m wind, 2 m temperature and dew point, roughness, boundary-layer height, and friction velocity |
| Surface flux | Instantaneous sensible-heat and moisture flux |

The provider files first enter a private work directory below the project's
data root. The preparation stage checks their variables and coordinates, adds
the dataset-family metadata expected by Trajecta, and promotes ready NetCDF
files to `data/ready/`. The resulting files can be read through either the Rust
or native NetCDF path selected by a Profile.

## Create the plan before the data exists

Project validation succeeds in a configured state while `data/` is still
empty:

```text
trajecta --project examples/air-mass-era5-pressure project validate
trajecta --project examples/air-mass-era5-pressure project data-plan --output era5-plan.json
```

The plan requirement contains:

| Field | Expected value |
|---|---|
| Case/Profile | `air-mass` / `product` |
| Dataset | `era5-pressure` |
| Dataset profile | `era5-cf-pressure-netcdf-v0` |
| Reader | `rust` |
| Lock path | `locks/era5-pressure.lock.json` |
| Data root | `data` |
| Capabilities | `domain_fill`, `near_surface_transport`, `transport` |
| Physical coverage | 2018-12-01 06:00–06:10 UTC |

This staged state is useful when a Case is prepared before a data request is
submitted. Editing the Case changes `project_sha256`; the helper compares that
identity with the supplied plan and asks for a fresh plan when they differ.

## Preview and fetch data

The package's data dependencies can be installed in a Python environment:

```text
python -m pip install -r requirements-data.txt
```

Preview the exact request first:

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json
```

The JSON output lists the area in north-west-south-east order, acquisition
anchors, provider dataset names, variables, pressure levels, target paths, and
current file states. The supplied project reports the built-in tutorial area
`[53, 0, 45, 10]`.

The Climate Data Store client reads account credentials from its standard
configuration or environment. The helper output and
`TRAJECTA_FETCH_MANIFEST.json` contain request metadata and file hashes rather
than credential values.

Start the download and preparation pipeline with:

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json --execute
```

The service may take time to stage a request. Existing matching targets are
reused. Downloads and transformations are kept under `data/.trajecta-fetch/`
until ready files can be promoted. A successful completion removes that work
directory and writes `data/TRAJECTA_FETCH_MANIFEST.json`.

## Finalize the prepared project

Inspect one ready file, finalize, and run the local filesystem check:

```text
trajecta data inspect examples/air-mass-era5-pressure/data/ready/ERA5_READY_FILE.nc
trajecta --project examples/air-mass-era5-pressure project finalize
trajecta --project examples/air-mass-era5-pressure doctor --deep
```

Replace `ERA5_READY_FILE.nc` with a file name shown by the fetch helper. Data
inspection reports the family marker, valid times, grid, pressure levels, and
source roles.

Finalization scans the ready set as one logical dataset. The DatasetLock records
every prepared file, its content hash and size, time coverage, grid signature,
vertical signature, profile identity, and capabilities. The project status then
moves from `configured` to `finalized`.

## Run the limited-domain Case

Submit the Profile in the foreground:

```text
trajecta --project examples/air-mass-era5-pressure run --profile product
```

After completion, read the result through the product commands:

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

The initial output event contains the 1,000 dry-air particles. The final count
depends on whether a trajectory reaches the safe grid boundary during the ten
minutes. A particle intersecting that boundary receives a terminal state at the
intersection time and a `domain_boundary` normal termination.

The limited-domain policy therefore makes a smaller final row count meaningful:
it describes resolved outflow. The inspect summary separates that outcome from
particle errors such as invalid meteorology. Mass-ledger rows place outgoing
and terminated carrier mass alongside the mass that remains active.

## Read motion near a finite boundary

For one particle, compare its two scheduled states and any terminal state:

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

`physical_time` gives the actual meteorological time. `integration_offset_ns`
is positive for this forward Case, and `elapsed_age_ns` measures time since the
particle's birth. When a particle terminates between endpoint events, its final
record carries the exact intersection time rather than being shifted to the
next scheduled output.

The sampled wind, pressure, and temperature columns also carry validity and
quality labels. These are useful when comparing a Rust-reader Profile with a
native-reader Profile on the same locked inputs.

## Adapt the tutorial area

The generic helper obtains an area from release geometry when a Case has one.
This domain-fill Case has no source geometry, so the supplied ERA5 tutorials
use the documented 53–45° N, 0–10° E default. For another limited region, the
provider utility accepts an explicit north-west-south-east box:

```text
python tools/fetch_era5_pressure_cds.py --out-dir PROJECT_DATA_WORK --date YYYY-MM-DD --times 00:00 06:00 12:00 --area NORTH WEST SOUTH EAST
```

Run `prepare_era5_pressure_anchors.py` on that work root, place its ready files
under the Profile's data root, and finalize the project. The prepared grid
defines the meteorological domain used by `limited_domain_terminate/v0`.

When increasing the spatial box, also consider file size, memory budget, and
query locality. When extending time, regenerate the project plan so acquisition
anchors cover every physical sample. Keep `horizontal_halo_cells` large enough
for the interpolation stencil used near the grid edge.

## Next steps

The [ozone tutorial](ozone.md) retains the same limited tutorial box and moves
to 137 hybrid model levels, three-hour anchors, backward execution, and a
potential-vorticity-based population. The
[data-family comparison](data-families.md) summarizes the different source
layouts.
