---
title: Trajecta how-to guides
description: Task-oriented procedures for configuration, deferred data, finalization, queues, events, cancellation, reruns, pruning, results, and AI assistance.
---

# How-to guides

These procedures assume an installed `0.1.0-alpha.1` package and a selected
machine configuration.

## Build configuration incrementally

Use selectors to inspect and change one value at a time:

```text
trajecta config list
trajecta config get resources.memory_pool_mib
trajecta config set resources.memory_reserve_mib 1024
trajecta config set resources.memory_pool_mib 8192
trajecta config unset profile_templates.experimental
trajecta config validate
```

Keep one file as the configuration truth. Failed `set` or `unset` operations do
not replace a valid file with partial content.

## Configure a project before data exists

`project init`, `project set`, `case validate`, and `project validate` can run
before meteorological files are present. The project remains `configured` or
`draft`, and missing locks appear as diagnostics.

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project validate
trajecta --project PROJECT project data-plan --output data-plan.json
```

This state is useful for review and provider planning. It is not admissible for
a run.

## Use data-plan and finalize

Review a data plan before network access:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json
```

CFSR downloads use the Python standard library. Before executing an ERA5
pressure or hybrid request, install the pinned preparation dependencies:

```text
python -m pip install -r requirements-data.txt
```

After approving the provider request, add `--execute`. The helper writes only
inside declared data roots and never creates or replaces a DatasetLock.

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json --execute
trajecta --project PROJECT project finalize
```

Finalization expands the selected documents, inspects actual data, checks
capabilities and coverage, then writes locks atomically. Re-run `project
data-plan` after changing a Case or Profile.

## Run in front or background

Foreground wait is the default:

```text
trajecta --project PROJECT run --profile PROFILE
```

Submit to the local queue and return after admission with:

```text
trajecta --project PROJECT run --profile PROFILE --detach
trajecta job wait JOB_ID
```

The daemon admits work against configured CPU and memory pools. A detached job
continues after the submitting terminal closes. A foreground client disconnect
also leaves admitted work under daemon ownership.

## Consume task events

Read all queue events from the beginning:

```text
trajecta --format jsonl job events --since 0 --follow
```

Read one job only:

```text
trajecta --format jsonl job events JOB_ID --since SEQUENCE --follow
```

Persist the last event sequence after processing it. Reconnect with `--since`
to avoid losing or duplicating state transitions. JSONL emits a stream header,
items, and one summary when the stream ends.

## Cancel, rerun, forget, and prune

```text
trajecta job cancel JOB_ID
trajecta job cancel JOB_ID --force
trajecta job rerun JOB_ID
trajecta job forget JOB_ID
trajecta job prune
```

Safe cancel requests a cooperative stop. Force cancel is reserved for a worker
that does not reach a safe point. A rerun creates a new attempt and retains the
old attempt. Completed jobs are not run again during daemon recovery.

`job prune` is **dry-run only** in this release. It returns a deterministic plan
and does not delete results, job rows, forensic data, or attempts. Review the
plan outside an active incident.

## Read results

Start with product commands:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

Use the SQLite schema only for advanced read-only analysis. Do not modify the
database, manifest, provenance bundle, or result directory in place.

## Use an AI configuration companion

Give the assistant the same Case, Profile, project index, data plan, and public
schema files that a human reviews. Ask it to propose `config set` or `project
set` commands rather than a second configuration copy. Never provide CDS
credentials or private paths that are unnecessary for the task.

Before accepting an AI-assisted setup, run:

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta --project PROJECT doctor --deep
```

Diagnostics and the resolved documents remain the acceptance evidence.
