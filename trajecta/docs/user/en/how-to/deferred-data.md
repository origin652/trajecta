---
title: Configure a Trajecta project before data arrives
description: Prepare Cases and Profiles, generate a data plan, acquire meteorology, and finalize a project in separate sessions.
---

# Configure before data arrives

A Trajecta project can be reviewed before its meteorological files have been
downloaded. The Case establishes the simulation interval and spatial domain;
the RunProfile selects a dataset family and local data root. From those
documents, `project data-plan` calculates the required coverage without opening
a provider connection.

This workflow is useful when data preparation runs on another machine, when an
ERA5 request is queued at the provider, or when project configuration needs
review before network access.

## 1. Create the project skeleton

Initialize an empty project directory and inspect the generated index:

```text
trajecta project init PROJECT --name "Moisture study"
trajecta --project PROJECT project show
```

Place Case and RunProfile files beneath that directory. Then add their
project-relative paths to `trajecta-project.yml`, either with a text editor or
with `project set`. The [document reference](../reference/documents.md) lists
the index fields, and the complete examples under `examples/` provide working
starting points.

One project can contain several Cases and several Profiles. Each named Profile
selects one indexed Case. This keeps experiments together while preserving an
unambiguous Case/Profile pair for each run.

## 2. Validate documents without meteorological files

Validate each document first, then the project index:

```text
trajecta case validate PROJECT/cases/moisture.yml
trajecta case resolve PROJECT/cases/moisture.yml
trajecta case validate PROJECT/profiles/cfsr.yml
trajecta --project PROJECT project validate
trajecta --project PROJECT project status
```

At this point, a complete logical setup normally reports `configured`. A file
with missing required Profile values remains `draft`. Invalid field names,
types, references, or population settings produce an error that can be fixed
before data acquisition begins.

`case resolve` expands referenced components and prints normalized content.
That view is useful when a Case is composed from shared domain, time, or
population fragments.

## 3. Generate a deterministic data plan

Write the plan inside the project directory:

```text
trajecta --project PROJECT project data-plan --output data-plan.json
```

The plan records, for every selected dataset:

- the Case-derived coverage interval;
- the required meteorological capabilities;
- the dataset profile and logical dataset ID;
- the project-relative data root and lock path;
- the project identity used to detect stale plans.

Generate the plan again after editing a Case, Profile, or dataset mapping. The
download helper compares the supplied plan with the current project and stops
when their identities differ.

## 4. Preview provider requests

Run the helper without `--execute`:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

The preview reports `dry-run` mode. It expands interpolation anchors, provider
requests, target paths, and the current state of each local file. No network
request or project edit is made.

The helper supports these production dataset families:

| Family | Provider path | Local preparation |
| --- | --- | --- |
| CFSR pressure levels | NOAA NCEI HTTPS archive | GRIB2 files retained in the declared root |
| ERA5 pressure levels | Copernicus Climate Data Store | Provider files converted to the reader-ready NetCDF layout |
| ERA5 hybrid levels | Copernicus Climate Data Store | Model-level and companion fields prepared into reader-ready NetCDF files |

For ERA5, install the pinned preparation dependencies from the repository:

```text
python -m pip install -r requirements-data.txt
```

CDS authentication follows the official CDS client configuration. Credentials
stay in that configuration or in the provider's supported environment
variables; the helper does not copy them into the project or its fetch
manifest.

## 5. Acquire or stage the files

After reviewing the request list, execute it:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json --execute
```

All targets are resolved beneath data roots declared by the project. Existing
CFSR files with the expected frozen identity are reused. A conflicting file is
reported instead of being overwritten. ERA5 preparation uses an owned work
directory below the selected data root and promotes complete files into the
reader-ready directory.

When another system performs the download, copy its finished files into the
same declared data root. The project is still unfinalized at this stage, so a
partial transfer can remain in place until the remaining frames arrive.

## 6. Inspect representative source files

`data inspect` reads metadata from one GRIB or NetCDF source without launching
a simulation:

```text
trajecta --format json data inspect PROJECT/data/pgbl00.gdas.2009010100.grb2
```

Inspect at least one file from each dataset family or preparation batch. The
response identifies the container, grid, vertical coordinate, time, fields,
and reader capability information that will later be checked during
finalization.

## 7. Finalize the project

Once the planned files are in place, run:

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

Finalization resolves the selected Case/Profile pairs, scans the actual data
roots, checks time and capability coverage, and writes DatasetLocks atomically.
The state becomes `finalized` after every selected mapping is coherent.

The fetch manifest and DatasetLock serve different purposes. The fetch manifest
describes what the helper obtained. The DatasetLock is the inventory Trajecta
uses at submission and records in run provenance. Only `project finalize`
creates the latter in this workflow.

!!! tip "Downloading does not finalize the project"

    Run `project finalize` after the last file arrives. The fetch helper leaves
    DatasetLocks unchanged.

## Continue after an interrupted preparation

Run the helper in preview mode again. Its file-state table distinguishes
`present`, `missing`, and `conflict` targets:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

Present CFSR files with matching size and SHA-256 are kept. Missing requests
can then be executed. Resolve a conflict by comparing the local file with the
provider inventory and placing the intended file at a clean target path.

After a Case interval or domain changes, create a fresh data plan before
resuming. Finalization will also report coverage gaps, so the existing project
can remain configured while a larger provider request is prepared.

## Share one collection between projects

Several projects may point to the same read-only meteorological collection.
Each project keeps its own relative mapping and DatasetLock, while the file
bytes can be shared through a directory copied or mounted beneath each project
root. Keep result directories and fetch work directories separate from the
shared read-only collection.
