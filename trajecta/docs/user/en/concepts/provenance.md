---
title: Provenance and result directories
description: Understand the run manifest, SQLite output, resolved inputs, field provenance, digests, reports, and interrupted artifacts.
---

# Provenance and result directories

Every accepted attempt receives its own result directory. The directory keeps
the numerical output together with the input identities and field lineage that
describe how it was produced. A later project edit has no effect on this stored
attempt.

## Directory layout

A completed product run normally contains:

```text
attempt-1/
├── run-manifest.json
├── resolved-case.json
├── resolved-run-profile.json
├── particles.sqlite
├── particles.sqlite-wal
├── provenance-bundle.json
└── run-report.md                 # present after `run report`
```

An interrupted attempt can also contain worker logs, partial database state,
or files whose names include `forensic-aborted`. The manifest status and
artifact list indicate which terminal path was reached.

## Run manifest

`run-manifest.json` is the directory index. It starts in `running` state and is
atomically replaced as lifecycle information changes. A terminal manifest
contains:

| Section | Contents |
| --- | --- |
| Identity | Job series, run ID, attempt, Case name, start and finish time |
| Software | Package and crate versions plus source revision when available |
| Inputs | Case, RunProfile, DatasetLock, dataset profile, and dataset-content hashes |
| Execution | Worker count, memory budget, executor, readers, wall time, peak RSS, and available I/O counters |
| Numerical | Random seed, integrator, boundary policies, population, sink, tolerance registry, and deterministic flag |
| Geometry | Resolved release source identities and canonical geometry summaries |
| Outputs | Effective product and schedule selected by the Case |
| SQLite | Schema version, journal settings, table row counts, and relative path |
| Provenance | Bundle path, exact and normalized digests, and record counts |
| Lifecycle | Status, termination summary, mass ledger, and optional failure |

Paths declared by the manifest are relative to the attempt directory. Product
commands check containment before opening them.

## Resolved input documents

`resolved-case.json` contains the normalized scientific Case after local
component references have been expanded. `resolved-run-profile.json` contains
the exact execution and dataset binding used by the worker.

These files answer practical questions that the current project cannot answer
after it has changed:

- Which direction and time step did this attempt use?
- Which population, seed, and output schedule were active?
- Which logical dataset mapped to which lock and reader?
- How many workers and how much memory did the attempt request?

The manifest stores SHA-256 identities for both documents. Keep the resolved
copies with the result when archiving or sharing it.

## Particle-state SQLite database

`particles.sqlite` is the main queryable result. Its public schema has six
tables:

| Table | Role |
| --- | --- |
| `run` | One row for run identity and terminal status |
| `particle` | Stable particle identity, population, origin, birth, carrier mass, and sensitivity weight |
| `particle_mass` | Substance mass associated with a particle |
| `output_event` | Ordered physical output times and event kinds |
| `particle_state` | Particle position, selected meteorology, quality, and provenance pointer at each sample |
| `termination` | One classified terminal reason for every terminated particle |

The compound keys include `run_id`, so copied rows remain attributable to one
attempt. Particle states are ordered by stable particle ID and per-particle
sample sequence in product streams.

SQLite uses write-ahead logging while the worker is active. Normal terminal
finalization checkpoints and truncates the WAL. Readers can use indexed high-
water snapshots during a run, while archival and full verification wait for a
terminal state.

## Field-level provenance bundle

`provenance-bundle.json` connects selected meteorological values in
`particle_state` to their source records. It covers five stored fields:

- eastward wind;
- northward wind;
- geometric vertical velocity;
- air pressure;
- air temperature.

For each distinct field lineage, a content-addressed record stores:

| Part | Meaning |
| --- | --- |
| Field token | Canonical field name |
| Quality | Source, derived, estimated, or other declared quality |
| Sources | Locked source identities used in scientific order |
| Transforms | Ordered operation IDs and parameters applied by the profile/query path |
| Fallback reason | Optional explanation when a fallback field path was selected |
| Profile SHA-256 | Exact interpretation-profile identity |

The bundle interns repeated records by their canonical JSON SHA-256. A second
dictionary groups the five optional field slots into content-addressed field
sets. Finally, every `(particle_id, sample_sequence)` points to one field-set
SHA-256. This layout avoids repeating the complete source and transform chain
for every particle sample.

The sample assignment order matches the SQLite particle-state order. A missing
stored meteorological value has a null field-set slot; its SQLite validity and
quality fields describe that sample.

## Exact-file and normalized digests

Several hashes serve different comparison needs:

| Digest | What changes it |
| --- | --- |
| SQLite SHA-256 | Any byte-level change to the finalized database file |
| Canonical SQL digest | A change to normalized public table content or order |
| Provenance-bundle SHA-256 | Any byte-level change to the final JSON bundle |
| Provenance content digest | A change to normalized records, field sets, or sample assignments |
| Canonical output digest | A change to canonical SQL content or normalized provenance content |

The manifest connects these identities: the provenance block includes the
SQLite exact hash, canonical SQL hash, bundle hash, normalized provenance hash,
and canonical output hash. `result verify` recalculates them from the files.

Exact-file hashes are useful during transfer. Normalized digests are useful
when comparing equivalent output whose container bytes may differ for a benign
storage reason.

## Lifecycle and provenance completion

The formal provenance bundle is finalized after SQLite output closes, because
it contains the final SQLite SHA-256. Successful states—`complete`,
`completed_with_particle_errors`, and safely `cancelled`—therefore have both a
terminal database and formal bundle.

If output finalization stops partway through, a partially built bundle is moved
to a unique `forensic-aborted` name. It does not occupy the formal
`provenance-bundle.json` path. `result inspect` lists those files so that an
operator can see what remained after interruption.

## Derived run report

`trajecta run report --result RESULT` creates `run-report.md` from
`result inspect`. The report collects identity, lifecycle, inputs, resources,
particle counts, quality, mass accounting, verification state, and artifact
paths in one Markdown file.

The report is regenerated atomically and is excluded from the canonical output
digest. It can be refreshed after a full-verification or forget operation,
because those catalog views may have changed while the numerical result stayed
the same.

## Reading provenance in routine analysis

Start with progressively deeper views:

1. `result inspect` for lifecycle, counts, identities, and artifact paths.
2. `result verify` for file/digest checks after transfer.
3. `result verify --full` for row, lifecycle, quality, termination, and mass
   audits.
4. `result trajectory` for particle metadata and ordered state records.
5. Direct read-only SQLite and provenance-bundle queries for specialized
   analysis.

The trajectory stream includes `provenance_id` on each state. The public
provenance bundle uses the matching particle ID and sample sequence to connect
that state to a five-field set.

## Copying and retaining results

Copy the attempt directory as one unit after it reaches a terminal state. Keep
the manifest, resolved documents, SQLite database, WAL entry when present,
provenance bundle, report, and forensic files together. Run quick verification
at the destination, followed by full verification for a long-term archive.

Analysis outputs belong in a neighboring directory chosen by the user. This
keeps the attempt directory stable and lets figures, tables, or future exported
products have their own naming and retention policy.
