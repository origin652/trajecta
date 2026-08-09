---
title: Inspect and read Trajecta results
description: Resolve a run result, inspect its lifecycle and artifacts, run quick or full verification, stream trajectories, and generate a report.
---

# Inspect and read results

A result directory belongs to one exact attempt. It combines the run manifest,
particle-state SQLite database, provenance bundle, resolved source documents,
and any forensic files produced during interruption or failure. Product commands
provide stable views of this directory before advanced analysis opens SQLite.

## Select a result

Commands accept any of these values as `RESULT`:

| Form | Example | Resolution |
| --- | --- | --- |
| Job-series ID | `0198...` | Current attempt in the selected local catalog |
| Run ID | `0198...` | Exact attempt in the selected local catalog |
| Directory path | `PROJECT/runs/.../attempt-1` | Exact on-disk directory |

ID lookup uses the catalog selected by the machine configuration. A directory
path is convenient after copying a run to another machine:

```text
trajecta --config configs/workstation.toml result inspect JOB_ID
trajecta result inspect archived-runs/study/attempt-2
```

## Inspect lifecycle, counts, and artifacts

Start with human output:

```text
trajecta result inspect RESULT
```

Use JSON to obtain the complete `trajecta.result-inspection/v1` product:

```text
trajecta --format json result inspect RESULT
```

The inspection groups information into these areas:

| Area | Contents |
| --- | --- |
| `identity` | Job series, run ID, attempt number, and Case name |
| `lifecycle` | Status, run-success flag, start and finish times, and failure detail |
| `software`, `inputs`, `numerical` | Software identity, source data identity, and numerical settings from the manifest |
| `execution` | Resource settings and runtime counters |
| `particles` | Particle, state, event, mass, and termination counts from SQLite |
| `quality` | Wind, pressure, and temperature validity/quality buckets |
| `mass_ledger` | Ledger count and maximum observed imbalance |
| `artifacts` | Manifest, SQLite, WAL, provenance, report, and forensic paths with observed sizes |
| `catalog` | Visibility, full-verification record, and supersession when the catalog is available |

For `complete`, `completed_with_particle_errors`, and `cancelled` results,
inspection expects the terminal SQLite and provenance products declared by the
manifest. An interrupted or failed attempt can return a partial inspection
when its manifest is readable. If SQLite is present but cannot be inspected,
the JSON envelope keeps manifest-derived fields and includes one structured
warning; SQLite-derived sections are `null`.

## Distinguish lifecycle success from readable output

`lifecycle.status` describes the run:

| Status | Interpretation |
| --- | --- |
| `complete` | The run finished with zero abnormal particle terminations |
| `completed_with_particle_errors` | The run finalized, with one or more abnormal particle terminations |
| `cancelled` | Safe cancellation produced a finalized partial result |
| `failed` | A controlled run-level failure reached a terminal manifest |
| `interrupted` | Worker loss or force-stop left forensic output |
| `running` | The directory is still owned by an active attempt |

Inspection can describe a terminal directory whose run did not succeed. Read
`lifecycle.run_success` in machine output when downstream processing requires a
successful scientific run.

## Run quick verification

Quick verification checks the immutable artifact identities and structure:

```text
trajecta result verify RESULT
```

It parses and validates `run-manifest.json`, requires a terminal status eligible
for verification, checks terminal WAL state and public SQLite row counts,
recomputes exact-file and canonical SQL digests, validates the provenance
bundle, and recomputes the canonical output digest.

The response is a `trajecta.result-verification/v1` record containing the
manifest, SQLite, SQL, provenance, normalized-provenance, and canonical-output
SHA-256 values. Keep this compact verification in transfer or archival logs.

## Run full verification

Full mode adds row-level scientific and lifecycle audits:

```text
trajecta result verify RESULT --full
```

It checks particle/sample ordering, event coverage, finite coordinates and
state values, meteorological quality declarations, termination coherence, and
mass-ledger tolerances. Its `full` object reports audited particle, sample,
termination, output-event, and mass-ledger counts.

When a catalog-resolved `complete` attempt passes full verification, Trajecta
records the canonical output digest in the local catalog. That record is used
for attempt supersession and dry-run pruning decisions.

Verification reads a finished directory. A `running`, `failed`, or
`interrupted` result remains available for inspection and forensic work but
does not produce a successful verification record.

## Read selected particle trajectories

Request one or more stable particle IDs:

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

IDs are sorted by the CLI parser, and duplicate IDs are rejected. Every
requested ID is checked before stream output begins; an absent ID produces a
diagnostic without a partial trajectory response.

To read every particle, use:

```text
trajecta result trajectory RESULT --all
```

Large selections are streamed in particle and sample order. JSONL is the most
direct format for a pipeline:

```text
trajecta --format jsonl result trajectory RESULT --all
```

The first data item is a `trajecta.trajectory-stream/v1` header describing run
identity, status, and selection. For each particle, the stream then emits one
`record_kind: "particle"` record followed by its ordered
`record_kind: "state"` records. The final item is a stream summary.

Particle records contain origin, birth time, dry-air mass, sensitivity weight,
and substance masses. State records contain physical time, position, event and
sample sequence, particle status, selected meteorology, quality flags,
provenance ID, and terminal information when the particle ends on that sample.

JSON output uses a temporary spool so that the command can produce one valid
CLI envelope without holding every trajectory record in memory. JSONL is still
preferable for an unbounded downstream reader because it can process each line
as it arrives.

## Generate the Markdown run report

Write the standard report inside the resolved run directory:

```text
trajecta run report --result RESULT
```

The path is fixed to `RESULT/run-report.md`. The command inspects the result,
renders identity, lifecycle, inputs, execution, particles, quality, mass,
verification, artifacts, and forensic pointers, then replaces the report
atomically. Running it twice on unchanged input produces the same report bytes.

The report is a human-readable derived view. It is excluded from the canonical
scientific output digest, which allows the report to be regenerated without
changing result identity.

## Move or archive a result

Copy the complete attempt directory, including a zero-byte WAL file when one is
present. After transfer, run both verifiers against the directory path:

```text
trajecta result verify archived-runs/attempt-1
trajecta result verify archived-runs/attempt-1 --full
```

Catalog fields are `null` when the copied directory is not registered in the
selected local catalog. Manifest and artifact identities remain available.

## Open SQLite for advanced analysis

The [SQLite reference](../reference/results-sqlite.md) documents the public
tables and joins. Open `particles.sqlite` through a read-only connection after
the attempt is terminal. Begin with `result trajectory` for particle histories
and use direct SQL when the analysis needs cross-particle aggregation or a
specialized query.

Keep derived tables, NetCDF files, figures, and other exports in a separate
analysis directory. The current release has no export command, and the run
directory remains the immutable source product.
