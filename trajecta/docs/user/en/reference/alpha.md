---
title: Alpha release scope
description: Understand the platforms, local runtime, data preparation, result interfaces, compatibility, and reserved extension points in Trajecta 0.1.0-alpha.1.
---

# Alpha release scope

`0.1.0-alpha.1` provides a complete local workflow from project preparation to
a verified trajectory result. The alpha label leaves room to refine public
documents and operational details before the first stable release. This page
collects the current boundaries in one place.

## Included workflow

The release supports:

1. incremental machine and project configuration;
2. Case and RunProfile validation and resolution;
3. deterministic data planning, inspection, locking, and project finalization;
4. foreground or detached submission to a persistent local queue;
5. multi-job CPU and memory admission with durable events;
6. safe cancellation, force cancellation, rerun, and restart recovery;
7. result inspection, quick or full verification, trajectory reading, and
   Markdown report generation;
8. packaged execution on Windows x64 and Ubuntu 24.04 x86-64.

The supported meteorological families are CFSR pressure levels, ERA5 pressure
levels, and ERA5 hybrid model levels. Regular release, dry-air-mass domain
filling, and stratospheric-ozone domain filling are included in the package
matrix.

## Local control plane

The daemon, worker processes, job catalog, and IPC endpoint run on one machine.
Detached mode supports background work on that machine and survives the
submitting terminal. Remote hosts, network queue submission, and distributed
multi-node scheduling are outside the current command surface.

One machine configuration selects one local endpoint and catalog. Several
configurations can describe separate local queues, provided their endpoint and
catalog paths do not collide.

## Data preparation

Projects may be configured before meteorological files are available.
`project data-plan` reports the missing or partial requirements, and
`project finalize` is run explicitly after data is present.

The optional download helper is a separate Python tool:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan DATA_PLAN
```

Its default mode previews provider requests and target paths; `--execute`
performs downloads. Provider credentials are read from official configuration
or environment locations and are not written into the Project, DatasetLock,
logs, or provenance. The helper leaves DatasetLock creation to explicit
finalization.

## Result product

The primary result is the run directory with a manifest, SQLite particle
history, provenance bundle, resolved inputs, and optional Markdown report.
Product commands provide inspection, verification, and trajectory streaming.
The version 1 SQLite schema is available for advanced read-only analysis.

A general export command is not included. Later exporters are planned to write
to a user-selected directory outside the immutable run product. This keeps the
current result identity independent of derived files.

## Queue retention

Completed and other terminal attempts remain terminal after a daemon restart.
Rerun is explicit and creates a new attempt. `job forget` changes routine list
visibility while retaining the catalog and artifacts.

`job prune` returns a deterministic dry-run plan with
`delete_enabled: false`. `0.1.0-alpha.1` has no command that applies that plan or
deletes result directories.

## Extensibility

Plugin loading, a plugin manifest, and a stable plugin API have not been
implemented. The reserved [Extensibility](../concepts/extensibility.md) page
describes the boundaries a later design would need to preserve. Existing Rust
traits are contributor interfaces inside the workspace and do not form a
loadable plugin mechanism.

## Compatibility

| Surface | Alpha compatibility rule |
| --- | --- |
| Software version | Remains `0.1.0-alpha.1` for M5.1; ordinary commits do not create another version |
| Configuration and machine streams | Carry their own `schema_version`; a format change uses a new schema identity |
| Case and RunProfile | Carry their current numeric document schema and reject unknown fields |
| SQLite | Public `user_version = 1` schema; open read-only for external analysis |
| CLI | Public command tree is generated from the binary and checked against the frozen contract |
| Rust crates | Contributor-facing and free to evolve within the alpha series |

Alpha schema changes may require a documented migration when a future release
introduces another identifier. Resolved documents and result artifacts retain
their original identity; they are not silently rewritten by installing a newer
binary.

## Validation range

Clean-package product validation covers the two published platforms and the
matrix in the [platform reference](platforms.md). Scientific and performance
measurements use the frozen one-hour ERA5 comparison described in
[Validation](../validation/index.md). A study with another duration, domain,
dataset resolution, or physical module can add a validation case suited to that
workflow.
