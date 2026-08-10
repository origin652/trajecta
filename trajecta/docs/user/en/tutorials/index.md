---
title: Trajecta tutorials
description: Learn Trajecta through complete domain-fill moisture, scheduled release, air-mass, and stratospheric-ozone projects.
---

# Tutorials

The tutorials follow four small projects from their scientific Case to a
readable result. They use the same project layout, queue, numerical engine, and
result products as a longer study. Each one changes a single major part of the
problem: the particle population, meteorological family, spatial domain, or
trajectory direction.

The projects are stored under `examples/` in both the repository and product
package. Their files are included directly in these pages, so the configuration
shown in the manual is the configuration accepted by the command-line program.

## Command navigation

| Entry | When to use it | Contents |
| --- | --- | --- |
| [Everyday command roadmap](command-roadmap.md) | Running a first project or finding the next routine step | Common commands ordered from machine setup through project finalization, queue execution, and result reading |
| [Complete command index](command-index.md) | Browsing every available command for a known task | Every current command grouped under six task areas |
| [CLI command tree](../reference/cli.md) | Checking full arguments, output modes, and runtime behavior | Command synopses, global options, placeholder conventions, and exact binary help |

## What the tutorials use

| Tutorial | Scientific population | Meteorology | Direction | Data preparation |
|---|---|---|---|---|
| [Domain-fill moisture](domain-fill.md) | Equal dry-air carrier particles over a global domain | CFSR pressure levels | Forward | Four-frame demonstration asset |
| [Scheduled release](release.md) | 1,000 particles released from one point | CFSR pressure levels | Forward | Same four-frame asset |
| [Air-mass workflow](air-mass.md) | Equal dry-air carrier particles over a limited domain | ERA5 pressure levels | Forward | Data helper and Climate Data Store request |
| [Stratospheric ozone](ozone.md) | Dry-air carriers initialized inside the PV60 ozone mask | ERA5 hybrid model levels | Backward | Data helper and hybrid preparation pipeline |

All example runs use 1,000 particles, a 300-second integration step, endpoint
output, and a 1 GiB Profile memory budget. Their ten-minute physical intervals
keep data preparation and result inspection manageable. The population and
data differences remain visible without turning the tutorial into a long
performance run.

## Choose a starting point

The domain-fill tutorial is the best continuation from the quickstart. It
explains how a meteorological snapshot becomes equal-mass particles and how to
read the dry-air and water-vapor quantities in a result.

The release tutorial starts from an explicit source. It is useful when the
question begins with a known place, time, vertical position, and tracer mass.
It also introduces release geometries and event timing.

The air-mass tutorial moves to official ERA5 pressure-level preparation and a
limited domain. It shows how data-plan coverage becomes a provider request and
how particles leave a non-periodic domain.

The ozone tutorial combines a backward Case with the 137-level ERA5 hybrid
coordinate. It covers the prepared-file layout, logarithmic surface pressure,
hybrid coefficients, and ozone initialization from the named PV60 rule.

## Shared project workflow

Each project follows the same sequence:

```text
project validate
      ↓
project data-plan
      ↓
prepare meteorological files
      ↓
project finalize
      ↓
doctor --deep
      ↓
run --profile product
      ↓
result verify / inspect / trajectory
```

`project validate` checks the documents that are already present. The
data-plan lists physical coverage, the public dataset profile, required
capabilities, local target roots, and lock path. Finalization inspects the
prepared files and creates the DatasetLock selected by the RunProfile.

The run command submits the resolved project to the local queue. Endpoint
output produces one particle-state event at each end of the ten-minute
interval. The result commands then provide a summary, full consistency check,
and time-ordered records for selected particles.

## Reading the example files

Every tutorial project has the same four-part shape:

| Path | Question answered by the file |
|---|---|
| `trajecta-project.yaml` | Which Cases and Profiles belong to this project? |
| `cases/*.yaml` | What scientific simulation runs? |
| `profiles/*.yaml` | Where are local data and results, and what resources does the worker use? |
| `locks/*.lock.json` | Which exact meteorological files satisfy the selected Case and Profile? |

The lock appears after finalization. Result directories are created under the
Profile's `output_root`, with a distinct run identity for each attempt.

The snippets on these pages come from the files above. To change a tutorial,
edit the example file in the project directory and run `project validate`
again. This avoids a second configuration copy hidden in a notebook or shell
script.

## From a tutorial to a study

Extend one dimension at a time. A longer time interval changes the data-plan
and provider anchors. A larger particle population primarily changes compute
and memory demand. A denser output schedule increases SQLite rows and result
size. A different geographic domain changes both the meteorological request
and the physical meaning of boundary outflow.

Keep the original example as a small preflight and create a new project for the
study. Run the ten-minute form after moving to a new machine or data reader;
then expand time, population, and output once that local path is working. The
[data-family table](data-families.md) helps choose the matching profile and
time cadence.
