---
title: Cancel, rerun, forget, and prune Trajecta jobs
description: Stop a queued or running attempt, create a new attempt in the same job series, hide terminal work, and review the dry-run prune plan.
---

# Cancel, rerun, forget, and prune

Trajecta keeps a durable history for every accepted attempt. Lifecycle commands
change queue state or create another attempt; they preserve the earlier result
directories so that an interrupted run can be examined alongside a later
successful run.

Begin by recording the current snapshot:

```text
trajecta --format json job status JOB_ID
```

The `JOB_ID` used on this page is the job-series ID. The snapshot identifies
the current `run_id`, attempt number, state, and output directory.

## Request a safe cancellation

The regular cancel command is cooperative:

```text
trajecta job cancel JOB_ID
```

Its effect depends on the current state:

| Current state | Safe-cancel behavior |
| --- | --- |
| `queued` | Removes the attempt from dispatch and records `cancelled` |
| `starting` or `running` | Records `cancelling`; the worker observes the request at a macro-step boundary |
| `cancelling` | Returns the current cancellation state |
| Terminal state | Returns the existing terminal snapshot |

At a numerical safe point, the worker stops advancing particles, finalizes the
partial result it has produced, checkpoints its output database, and reaches
`cancelled`. The time needed to stop is therefore bounded by the work required
to reach and close the current macro step rather than by the event-polling
interval alone.

Watch the transition in another terminal:

```text
trajecta job events JOB_ID --follow
trajecta job wait JOB_ID
```

`job wait` returns exit code `1` for the cancelled terminal state. The result
directory remains available to `result inspect` and forensic review.

## Force-stop an unresponsive worker

Use force cancellation when the worker needs to stop immediately:

```text
trajecta job cancel JOB_ID --force
```

The daemon terminates the owned worker process and records `interrupted`.
Files already written remain in the attempt directory, including a SQLite WAL
or temporary output that had not reached its normal checkpoint. Treat that
directory as a partial forensic artifact. Start with:

```text
trajecta result inspect JOB_ID
trajecta --format json job events JOB_ID --since 0
```

Force cancellation is also the useful path when a safe cancellation remains in
`cancelling` beyond the expected macro-step duration. The
[recovery guide](../operations/recovery.md) describes checks for worker loss,
SQLite/WAL state, and interrupted manifests.

## Create a rerun attempt

Rerun preserves the logical job series and creates a new run identity:

```text
trajecta --format json job rerun JOB_ID
```

The new receipt has:

- the same `job_series_id`;
- a new UUID-v7 `run_id`;
- an attempt number increased by one;
- initial state `queued`.

The original normalized input selection and scheduler resource request are
copied from the series. Edit the project and submit a separate new run when the
Case, Profile, dataset lock, or resource request needs to change.

If the current attempt is still active, `job rerun` first requests safe
cancellation and waits for a terminal state. It then creates the next queued
attempt. This avoids two active attempts sharing one logical series.

Follow the new attempt with the same job-series ID:

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --follow
```

Events and snapshots include `run_id` and `attempt`, so a consumer can separate
the old and new histories.

## Mark a successful attempt as fully verified

After a new attempt completes, run the full result verifier:

```text
trajecta result verify JOB_ID --full
```

The full verifier checks lifecycle coverage, particle quality, mass accounting,
SQLite row counts, and the canonical output digest. A successful full
verification is recorded in the local catalog. It can mark older terminal
attempts in the same series as superseded by this verified complete run.

This record is what allows an older attempt to appear as an eligible item in
the prune plan. A merely newer attempt, or a newer attempt without successful
full verification, does not make the earlier directory eligible.

## Hide a terminal series from routine lists

`job forget` removes a terminal series from routine `job list` output:

```text
trajecta job forget JOB_ID
```

The operation changes list visibility. It keeps the catalog rows, events,
attempt identities, result directories, and verification history. Direct
lookups and result paths remain usable.

Forgetting is useful after an exploratory series has been reviewed and its
routine queue presence is no longer helpful. An active series reaches a
terminal state before it can be forgotten.

## Review the dry-run prune plan

In `0.1.0-alpha.1`, pruning only calculates a plan:

```text
trajecta --format json job prune
```

The response follows `trajecta.prune-plan/v1` and always contains:

```json
{
  "mode": "dry_run",
  "delete_enabled": false
}
```

Candidates are sorted by series, attempt, and run identity. A removable-looking
entry needs a later `complete` attempt in the same series that passed full
verification. Each item includes the observed path, recursive byte size,
reason, protection flag, and superseding run ID.

The command does not remove files, SQLite rows, events, reports, or provenance.
There is no apply or delete option in this release. Save the JSON plan when
estimating storage; perform any manual archival according to the site's own
retention procedure.

!!! tip "Prune is a report in this release"

    Treat its paths as candidates for review. Running the command never frees
    disk space.

## What happens after a daemon restart

The daemon reads durable attempt states and owned-worker identities at startup.
Terminal attempts—including `complete`, `failed`, `cancelled`, and
`interrupted`—remain terminal and are not dispatched again. Queued work can be
admitted when resources become available. An attempt whose worker disappeared
during an active state is reconciled as interrupted and retains its output
directory.

Use `job rerun` when another attempt is wanted. The restart itself never creates
one.
