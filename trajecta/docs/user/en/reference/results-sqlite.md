---
title: Results and SQLite reference
description: Read the Trajecta result-directory layout, product commands, SQLite version 1 tables, query ordering, WAL lifecycle, and digest relationships.
---

# Results and SQLite

Every accepted attempt receives a separate result directory. Its manifest,
database, resolved documents, and provenance belong to one run ID and attempt
number. A rerun creates another directory and leaves the earlier attempt in
place.

Product commands are the first entry point:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

`RESULT` accepts a job-series ID, exact run ID, or result-directory path. ID
resolution uses the job catalog selected by `--config`; a direct path can be
read without catalog lookup.

## Directory layout

| Path | Lifecycle | Content |
| --- | --- | --- |
| `run-manifest.json` | Created as running and replaced at closeout | Run identity, state, inputs, resources, counts, quality, SQLite identity, provenance identity, and digests |
| `particles.sqlite` | Written throughout the run | Particles, mass, output events, states, meteorological samples, and terminations |
| `particles.sqlite-wal` | Active-write sidecar; zero or absent after safe terminal closeout | Committed pages waiting for checkpoint |
| `particles.sqlite-shm` | SQLite shared-memory sidecar when present | WAL coordination state |
| `resolved-case.json` | Written before numerical execution | Exact normalized scientific Case admitted to the worker |
| `resolved-run-profile.json` | Written before numerical execution | Exact local execution and dataset binding admitted to the worker |
| `provenance-bundle.json` | Finalized during output closeout | Source-record, field-set, and sample attribution used by output rows |
| `run-report.md` | Created on request | Idempotent human-readable view of the other result artifacts |
| `provenance-bundle.json.forensic-aborted*` | Present after selected interrupted provenance closeouts | Retained partial provenance stream or bundle state |

Temporary and worker diagnostic files can appear after a failed or interrupted
attempt. `result inspect` returns the actual sorted artifact inventory instead
of assuming that every terminal directory has the successful layout.

## Inspection

```text
trajecta --format json result inspect RESULT
```

Inspection strictly parses the public manifest. For a readable manifest and
unavailable SQLite database, it returns the manifest and artifact inventory as
a partial response with `result.inspect_sqlite_unavailable`. SQL-derived fields
are null in that response. A successful terminal manifest that lacks SQLite or
provenance returns `result.artifact_missing`.

Manifest SQLite row counts are compared with the opened database. A mismatch
returns `result.manifest_sqlite_count_mismatch` rather than displaying two
different totals as one result.

## Quick and full verification

```text
trajecta result verify RESULT
trajecta result verify RESULT --full
```

Quick verification checks the result identity and low-cost structural
relationships. Full verification adds lifecycle coverage, finite-value and
quality checks, mass-ledger closure, SQLite counts and integrity, provenance
semantics, and the canonical output digest.

When a catalog-backed `complete` attempt passes full verification, the daemon
records the verified run ID and canonical digest. That record can make older
terminal attempts in the same series eligible for the dry-run prune plan.

## Trajectory reading

Select one or more particle IDs:

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

The parser sorts IDs and rejects duplicates. It validates every requested ID
before writing trajectory output, so a missing particle returns one diagnostic
without a partial stream. `--all` selects every particle and cannot be combined
with `--particle-id`:

```text
trajecta --format jsonl result trajectory RESULT --all
```

| Output mode | Shape |
| --- | --- |
| Human | Deterministic field-oriented lines |
| JSON | One envelope containing header and records; SQLite rows are first spooled to a create-new temporary file so validation finishes before stdout begins |
| JSONL | Header item, one data item per trajectory row, and one final summary |

Records are ordered by particle ID and sample sequence. For a large complete
population, JSONL avoids one large JSON document and lets a consumer process
rows incrementally.

## Run report

```text
trajecta run report --result RESULT
```

The output path is fixed at `RESULT/run-report.md`. The command writes a
same-directory create-new temporary file, flushes and synchronizes it, then
renames it into place. Running the command again over unchanged artifacts
produces the same report bytes.

The report excludes itself from the artifact digest calculation. It does not
change scientific content, SQLite SQL identity, provenance content, or the
canonical output digest.

## SQLite version 1

The public definition is
[`testdata/M4_SQLITE_SCHEMA.v1.sql`](https://github.com/origin652/trajecta/blob/main/trajecta/testdata/M4_SQLITE_SCHEMA.v1.sql).
The database declares `PRAGMA user_version = 1`, 32 KiB pages, foreign keys,
strict tables, WAL mode, and disabled automatic checkpointing. The Trajecta
writer owns the terminal checkpoint.

### `run`

One row identifies the database's run. Important columns are `run_id`,
`manifest_schema`, `case_name`, `status`, and nanosecond-resolution start and
finish timestamps. A running row has null finish fields; every terminal row has
both finish components.

### `particle`

One row per stable particle ID records `population_id`, origin, birth time, dry-
air mass, and optional sensitivity weight. `origin_kind` is `release`,
`domain_initial`, or `domain_boundary`; the accompanying event, domain, and
boundary-face columns follow that choice.

### `particle_mass`

The composite key `(run_id, particle_id, substance_id)` stores non-negative
mass in kilograms for each tracked substance.

### `output_event`

Events are ordered by `event_sequence` and carry physical seconds,
nanoseconds, and one of `birth`, `start`, `interval`, `end`, or `termination`.
The time index also orders by event sequence for coincident physical times.

### `particle_state`

The primary key is `(run_id, particle_id, sample_sequence)`. Each row contains:

- the output event and physical timestamp;
- integration offset and elapsed particle age;
- longitude, latitude, and height above sea level;
- `alive` or `terminated` status and optional termination reason;
- selected wind, pressure, and temperature values;
- validity and quality labels for sampled fields;
- an optional provenance ID into the bundle's attribution records.

The public time index is:

```sql
CREATE INDEX particle_state_by_time
ON particle_state (run_id, physical_seconds, physical_nanosecond, particle_id);
```

### `termination`

One row per terminated particle records reason, normal or abnormal
classification, physical time, and an optional boundary-intersection fraction
from zero through one.

## Read-only query patterns

Open the database with SQLite read-only mode. Keep `particles.sqlite-wal` and
`particles.sqlite-shm` beside the main file while an attempt is active.

Read one trajectory:

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1 AND particle_id = ?2
ORDER BY sample_sequence;
```

Read one physical output snapshot:

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1
  AND physical_seconds = ?2
  AND physical_nanosecond = ?3
ORDER BY particle_id;
```

Read termination totals:

```sql
SELECT classification, reason, COUNT(*) AS particles
FROM termination
WHERE run_id = ?1
GROUP BY classification, reason
ORDER BY classification, reason;
```

During active writing, use a read transaction and the indexed high-water state
visible to that snapshot. Do not run schema changes, write transactions,
`VACUUM`, or a checkpoint from a second process. Product commands already
apply the supported snapshot behavior.

## WAL and terminal closeout

A non-empty WAL is ordinary while the writer is active. Safe completion and
safe cancellation checkpoint the database and truncate the terminal WAL. A
force-stopped or lost worker may leave a non-empty WAL and running manifest in
its attempt directory.

Keep the main database and sidecars together. Move or archive only an inactive
complete directory, then run full verification at the destination. The
[storage guide](../operations/storage-sqlite.md) gives recovery procedures for
disk exhaustion and interrupted WAL closeout.

## Digest relationships

The manifest records separate identities for normalized provenance content,
SQLite SQL content, and canonical output. Path-normalized digests allow the same
scientific product to be compared across different attempt roots. The optional
report is excluded, so refreshing `run-report.md` leaves those identities
unchanged.

Future export products are planned to use a user-selected directory outside the
run product. `0.1.0-alpha.1` provides the result commands and read-only SQLite
interface shown on this page; it has no export command.
