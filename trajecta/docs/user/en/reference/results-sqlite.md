---
title: Results and SQLite
description: Result directory contract, preferred product commands, and advanced read-only SQLite schema guidance.
---

# Results and SQLite

The result directory is an immutable attempt artifact. Read it through product
commands before opening SQLite directly.

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

`run-report.md` is a derived, idempotent view. It is excluded from scientific
content and canonical-output digests. Future exported products must use an
explicit external destination; no export command exists in this release.

## Principal files

| Path | Role |
| --- | --- |
| `run-manifest.json` | Run identity, state, inputs, execution, quality, counts, and digests |
| `particles.sqlite` | Particle, sample, mass, event, and termination tables |
| `provenance-bundle.json` | Source-record and field-set attribution for samples |
| `resolved-case.json` | Exact Case admitted to the attempt |
| `resolved-run-profile.json` | Exact execution Profile admitted to the attempt |
| `run-report.md` | Optional derived human report |
| `forensic/` | Preserved interruption or failure evidence when present |

## SQLite schema version 1

The public SQL definition is
[`testdata/M4_SQLITE_SCHEMA.v1.sql`](https://github.com/origin652/trajecta/blob/main/trajecta/testdata/M4_SQLITE_SCHEMA.v1.sql).

| Table | Content |
| --- | --- |
| `run` | One run identity and terminal status |
| `particle` | Stable particle identity and birth metadata |
| `particle_mass` | Substance mass by particle |
| `output_event` | Ordered physical output events |
| `particle_state` | Position, time, state, selected meteorology, quality, and provenance pointer |
| `termination` | One classified terminal reason per terminated particle |

Open the database read-only. Do not write tables, run a mutating migration, or
checkpoint an active attempt from another process. A terminal successful result
has `PRAGMA integrity_check = ok` and an empty terminal WAL. Copy the complete
result directory before custom analysis that cannot guarantee read-only access.
