---
title: Storage, SQLite, and WAL
description: Plan Trajecta storage, interpret SQLite write-ahead logs, inspect active outputs, and handle a full or unhealthy filesystem.
---

# Storage, SQLite, and WAL

Trajecta writes the local job catalog, meteorological data, and run products to
separate paths. They may share a filesystem on a workstation, although their
growth and recovery requirements differ.

## Storage areas

| Area | Typical content | Growth pattern |
| --- | --- | --- |
| Configuration directory | Machine TOML file and local endpoint selection | Small and infrequent |
| Job-catalog directory | SQLite catalog, WAL, daemon ownership, events, and attempt metadata | Grows with submitted attempts and event history |
| Data roots | GRIB or NetCDF files covered by DatasetLocks | Grows when datasets are prepared |
| Output root | One directory per run attempt | Grows with particle-state rows, provenance, and retained interrupted runs |

`project show`, `project status`, and `project data-plan` expose project-relative
paths. `config list` shows the machine settings that select the catalog and
resource policy. Check free space at every filesystem involved before a large
queue is submitted.

## Result-directory layout

A completed result normally includes:

```text
RESULT/
  run-manifest.json
  particles.sqlite
  provenance/
  run-report.md          # after `run report --result RESULT`
```

Additional resolved documents, worker logs, temporary files, or forensic files
may be present according to the attempt state. `result inspect` returns the
actual artifact inventory and identifies files that exist for that run:

```text
trajecta --format json result inspect RESULT
```

The manifest is written through lifecycle transitions. An active or interrupted
attempt can therefore have a manifest without having the same terminal
guarantees as `complete`.

## What a WAL means

SQLite write-ahead logging places recent committed pages in a sidecar before
they are checkpointed into the main database:

```text
particles.sqlite
particles.sqlite-wal
particles.sqlite-shm
```

The job catalog uses the same SQLite WAL mechanism. A WAL beside an active
database is part of the database state. Readers may need it to see committed
rows, and the writer may still be using it.

During normal result closeout, Trajecta checkpoints the particle database and
truncates its WAL. A successfully closed terminal result therefore has a zero-
length or absent terminal WAL. A non-empty WAL in a running or interrupted
directory describes a different lifecycle state and should stay with its main
database.

!!! warning "Keep SQLite sidecars together"

    Never delete or move `*-wal` by itself. Until closeout and verification,
    treat the main database, `-wal`, and `-shm` as one set.

## Concurrent read access

The Trajecta result commands open read snapshots and respect the current
indexed high-water boundary. This avoids treating a row that is only partly
published by the writer as a complete trajectory record.

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 100
```

An active snapshot is a momentary view. Repeat the command to see later rows.
For a stable export or complete scientific analysis, wait for a terminal state
and run full verification first.

If another program opens `particles.sqlite` directly:

- use read-only mode;
- keep the WAL and shared-memory sidecars beside the database;
- avoid schema changes, checkpoints, vacuum, or write transactions;
- use the indexed ordering described in the
  [SQLite reference](../reference/results-sqlite.md);
- close the connection before moving or archiving the result directory.

## Estimate output growth

Particle-state storage is primarily driven by the number of particles that
remain active at each output event. A useful planning estimate is:

```text
particle rows ≈ sum of active particles over all output events
```

The byte size per row also reflects indexes, SQLite pages, metadata, and
provenance. Early population outflow can reduce later row counts. Shorter output
intervals increase the number of scheduled snapshots even when the simulation
duration and particle population remain fixed.

Measure one representative smaller run with the intended output schedule, then
apply a margin for the larger population, provenance bundle, WAL during active
writing, and retained failed attempts. The scheduler's memory budget does not
reserve disk space.

## Respond to a full filesystem

A full filesystem may interrupt SQLite growth, manifest replacement,
provenance closeout, or the job catalog itself. Start by reducing new writes:

1. Stop submitting additional work.
2. Check `job list` and identify active result directories.
3. Request safe cancellation when the filesystem still accepts the closeout
   writes it requires.
4. Free space outside active result directories.
5. Run `doctor --deep` after enough space is available.
6. Inspect each affected attempt and its event log.

If no space remains for a safe closeout, the worker may fail or become
interrupted. Keep its attempt directory intact. A rerun later uses a new
directory; it does not continue writing into the old SQLite database.

## Check SQLite health

Deep doctor performs a temporary create, WAL write, integrity check,
checkpoint, truncate, and cleanup in the selected environment:

```text
trajecta --project PROJECT doctor --deep
```

It checks whether new work can use the current SQLite runtime and filesystem.
For an existing run, use the result commands:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
```

`result inspect` can return a partial response when the manifest is readable
and SQLite is unavailable. Full verification checks the terminal database,
manifest counts, lifecycle, and canonical output identity.

When integrity or count checks fail, retain the original result directory and
create a new attempt after the storage problem has been corrected. This keeps
the damaged attempt available for diagnosis and prevents two run identities
from sharing one database.

## Move or archive a result

Wait for a terminal state and close any direct readers. Run:

```text
trajecta result verify RESULT --full
trajecta run report --result RESULT
```

Copy the complete directory, including any zero-length sidecars and retained
forensic files. Verify the copied result again at its destination before
removing the source through the site's storage procedure.

`job prune` can estimate space associated with superseded attempts:

```text
trajecta --format json job prune
```

In `0.1.0-alpha.1`, the response is a dry-run plan with
`delete_enabled: false`; it does not remove catalog rows or files.
