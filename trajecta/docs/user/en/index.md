---
title: Trajecta — Lagrangian water-vapor tracking
description: Open-source framework for Lagrangian moisture tracking, domain filling, and forward and backward trajectories.
---

# Lagrangian water-vapor tracking with Trajecta

Trajecta computes atmospheric trajectories by following moving air parcels
through gridded meteorological fields. A
particle receives wind, pressure, temperature, humidity, and other required
fields at its current position. The trajectory integrator advances that
particle through physical time, applies the selected boundary rules, and writes
its state at scheduled output times. Runs may move forward from a source period
or backward from a receptor period.

The same engine supports two common forms of Lagrangian study. A release case
creates particles at specified places and times, which is useful for following
an emission or sampling air arriving at a receptor. A domain-fill case
represents the air mass inside a meteorological domain and follows changes in
its water-vapor or ozone state. Domain filling is the main route for moisture
source and sink studies.

Trajecta is operated through a command-line project workflow. A study keeps its
scientific Case in a portable file, while a RunProfile supplies local data
paths, output placement, reader choice, threads, and memory budget. Before a
run starts, `project finalize` examines the meteorological files and creates a
DatasetLock. The worker then executes resolved, fixed inputs and writes a new
result directory.

## What you can run

| Study | Population created by Trajecta | Typical question | Available data path |
|---|---|---|---|
| Domain-fill moisture tracking | Air-mass particles distributed over a global or limited domain | Where did water vapor in a region come from, and where does it leave? | CFSR pressure levels or ERA5 pressure levels |
| Scheduled release | Particles born from one or more timed source geometries | Where is material released at a source transported? | CFSR or ERA5 pressure levels |
| Air-mass tracking | Dry-air mass sampled across a limited domain | How does an atmospheric air mass enter, move through, and leave the domain? | ERA5 pressure levels |
| Stratospheric ozone tracking | Domain-filled air mass initialized with the PV60 ozone rule | How is stratospheric ozone transported through the selected period? | ERA5 137-level hybrid data |

All four workflows use the spherical second-order Runge–Kutta trajectory
integrator supplied in `0.1.0-alpha.1`. Trajecta supports forward and backward
trajectories through the same Case model. A Case chooses the integration
direction, time step, boundary policies, population, random seed, and output
schedule. The meteorological profile determines how source variables are
decoded and converted into the canonical fields consumed by the integrator.

## The path from a Case to a result

```text
Case + RunProfile + project index
                 │
                 ▼
             data-plan
                 │  prepare or download the required frames
                 ▼
          project finalize
                 │  inspect files and create DatasetLock
                 ▼
        local queue and worker
                 │  load meteorology, integrate particles, write outputs
                 ▼
           result directory
```

The project can be prepared before the data is present. `project data-plan`
lists the physical time coverage, dataset profile, required capabilities, lock
path, and data root for every selected Case/Profile pair. The separate Python
data helper can preview provider requests or carry them out after approval.
Finalization remains an explicit command, so downloading files never silently
changes the inputs admitted to a run.

The CLI waits for a run in the foreground by default. `--detach` returns after
the local daemon has admitted the job. The daemon stores queue state and starts
workers within configured CPU and memory pools. A completed attempt stays
complete after a restart, while `job rerun` creates a fresh attempt under the
same job series.

## What a run produces

Each run receives its own directory below the Profile's `output_root`.
`particles.sqlite` contains output events, particle metadata, ordered particle
states, carried masses, and termination records. `run-manifest.json` provides a
compact account of the run, including input identities, numerical settings,
counts, quality summaries, and the final status. The directory also contains
the resolved Case, resolved RunProfile, and `provenance-bundle.json`.

Routine inspection begins with product commands:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

`result inspect` answers practical questions about a completed run: how many
particles and output events were written, which readers were used, how
terminations were classified, and where the main files are located. The
trajectory command streams the time-ordered records for selected particles.
Direct SQLite access is available for larger analysis programs and is
documented as a read-only interface.

## Where to begin

| If you want to… | Continue with… |
|---|---|
| Install the package and understand its directory layout | [Getting started](getting-started/index.md) |
| Complete a small domain-fill run with supplied CFSR data | [Fifteen-minute quickstart](getting-started/quickstart.md) |
| Learn a full moisture, release, air-mass, or ozone workflow | [Tutorials](tutorials/index.md) |
| Change configuration, prepare data, or manage queued jobs | [How-to guides](how-to/index.md) |
| Understand Cases, Profiles, populations, coverage, and results | [Concepts](concepts/index.md) |
| Recover work after shutdown, memory pressure, or disk failure | [Operations](operations/index.md) |
| Look up commands, fields, diagnostic codes, and SQLite tables | [Reference](reference/index.md) |
| Build the workspace or change the implementation | [Developer guide](developer/index.md) |

Version `0.1.0-alpha.1` provides release packages for Windows x86_64 and
Ubuntu 24.04 x86_64. Remote scheduling, plugin loading, destructive pruning,
and a general result-export command are still outside the released product.
The [alpha limits](reference/alpha.md) page records the current boundaries.
