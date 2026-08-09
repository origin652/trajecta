---
title: Recover interrupted Trajecta jobs
description: Reconcile Trajecta jobs after power loss, worker disappearance, force cancellation, disk failure, or an interrupted output closeout.
---

# Recovery and interrupted attempts

Recovery starts from the persistent job catalog and the files already present
in an attempt directory. The daemon uses those two sources when it returns: it
can reattach a matching live worker, reconcile a valid terminal manifest, or
close a missing worker as an interrupted attempt.

A restart never creates a rerun on its own. Completed attempts remain
completed, queued attempts remain eligible for ordinary dispatch, and an
interrupted attempt waits for an explicit `job rerun`.

## First response

Keep the machine configuration, project, job catalog, and attempt directories
in their current locations while the initial state is being read. In
particular, leave SQLite databases together with their `-wal` and `-shm`
sidecars.

Run the following commands with the same `--config` path used for submission:

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
trajecta --config workstation.toml --format json job status JOB_ID
trajecta --config workstation.toml --format json job events JOB_ID --since 0
```

The first job command starts or reconnects to the local daemon. Read the most
recent events after startup reconciliation has had time to update the catalog.

Record these values for every affected attempt:

| Value | Where to read it | Why it matters |
| --- | --- | --- |
| Job-series ID | Submission receipt or `job status` | Selects the logical task for later status and rerun commands |
| Run ID and attempt number | `job status` and events | Identifies the exact process and result directory affected |
| Current state | `job status` | Determines whether the run is active, terminal, or still queued |
| Output directory | `job status` | Locates the manifest, SQLite database, logs, and temporary files |
| Last durable event sequence | `job events` | Gives the cursor for later event reads |

## Read the reconciliation outcome

The startup event log usually contains one of the following paths:

| Event or state | Interpretation | Next step |
| --- | --- | --- |
| `worker.reattached` and `running` | The owned worker is still alive and its lease matches | Continue monitoring; avoid moving its output directory |
| `worker.terminal_reconciled` and a terminal state | The worker had already written a matching terminal manifest | Inspect and verify the terminal result |
| `worker.lost` followed by `run.interrupted.worker_lost` | No matching live worker or valid terminal closeout was found | Inspect the partial directory, correct the cause, then rerun if needed |
| `queued` | The attempt had not started | Leave it queued or cancel it according to the current plan |
| Existing terminal state with no new dispatch event | The catalog already considered the attempt finished | Keep it terminal; use `job rerun` only when another attempt is wanted |

`worker.lease_attach_failed` indicates that the daemon could not attach an
accepted lease to the launched process. If ownership also cannot be recovered,
`worker.uncontrolled_after_attach_failure` records the containment failure.
Inspect the process list and the attempt directory before creating another
worker for the same inputs.

## Inspect the attempt directory

Start with the product-level reader:

```text
trajecta --format json result inspect JOB_ID
```

`result inspect` can resolve a job-series ID, run ID, or direct result path. A
readable manifest with an unavailable SQLite database produces a partial
inspection response and `result.inspect_sqlite_unavailable`. This still gives
the manifest identity and artifact inventory.

Then examine the directory without editing files. Useful items include:

| File or directory | What to note |
| --- | --- |
| `run-manifest.json` | Lifecycle status, run identity, counts, input identity, and recorded failure |
| `particles.sqlite` | Presence, byte size, and whether result inspection can open it |
| `particles.sqlite-wal` and `particles.sqlite-shm` | Whether the writer stopped before checkpoint and closeout |
| Provenance directory or bundle | Whether provenance closeout had started or completed |
| Worker stdout and stderr | Native-library messages, allocation failures, or write errors |
| Temporary and forensic entries | Paths retained by the worker or daemon during the failed closeout |

!!! warning "Keep the original attempt intact"

    A rerun uses a separate directory. Retain the old manifest, database,
    sidecars, and forensic files together.

For a terminal `complete` result, run:

```text
trajecta result verify JOB_ID --full
```

An interrupted attempt often lacks the artifacts needed for full verification.
Its partial inspection remains useful for locating the last manifest state and
written SQLite rows.

## Recovery after a power loss

1. Confirm that the project, data roots, output root, and machine configuration
   are mounted at their original paths.
2. Run `config validate` and project `doctor --deep`.
3. Start the daemon through `job list`.
4. Read `job status` and all events for each active series from before the
   outage.
5. Wait for reconciliation to produce a stable active or terminal state.
6. Inspect every affected attempt directory.
7. Run full verification for a reconciled complete result.
8. Create a rerun only for work that should be computed again.

If the output filesystem recovered with errors, copy the complete inactive
attempt directory before further filesystem repair. Keep the main SQLite file
and sidecars together.

## Recovery after safe or force cancellation

A safe cancellation ends in `cancelled` after the worker reaches a macro-step
boundary and performs output closeout. The resulting partial run can normally
be inspected, and its terminal WAL is checkpointed or absent.

A force cancellation stops the owned worker immediately and records
`interrupted`. A non-empty WAL, running manifest, or unfinished provenance
temporary file is expected in that directory. Read it as a partial attempt and
leave the files together.

Check the final state before another attempt is created:

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

Rerun keeps the series identity and creates a separate attempt:

```text
trajecta --format json job rerun JOB_ID
```

If the Case, Profile, data lock, or resource request should change, edit the
project and submit a new run instead of rerunning the stored series inputs.

## Recovery after memory pressure or OOM

`daemon.external_memory_pressure` pauses queued dispatch and leaves active
workers alone. Once host memory rises above the reserve, ordinary scheduling
continues. There is no recovery action for an attempt that simply remained
queued.

An operating-system OOM termination appears as worker loss and interruption.
Before rerunning:

1. Read the resource events and operating-system memory record for the old run.
2. Compare its working set with `execution.memory_budget_bytes`.
3. Reduce concurrent admission, increase the machine reserve, or adjust the
   Profile request to reflect the measured workload.
4. Run deep doctor if SQLite or provenance writing was interrupted.

Further sizing guidance is available in
[Shutdown and memory pressure](shutdown-memory.md).

## Recovery after disk exhaustion

Restore enough free space without moving an active result directory. Once no
worker is writing to the affected directory:

1. Keep the main database and sidecars together.
2. Run project `doctor --deep` to check create, sync, rename, WAL, and cleanup
   operations on the current filesystem.
3. Inspect the result and read its terminal event.
4. Verify any run reconciled as complete.
5. Rerun a failed or interrupted task into a new attempt directory after
   storage has been corrected.

The [storage guide](storage-sqlite.md) covers WAL handling, output-size
estimation, and moving terminal results.

## Completed tasks during queue recovery

The daemon does not dispatch terminal attempts again. This applies to
`complete`, `completed_with_particle_errors`, `failed`, `cancelled`, and
`interrupted`. A newer daemon process reads those states from the same catalog.

Use `job rerun` to request another attempt. After a later complete attempt has
passed full verification, older attempts may appear in the dry-run prune plan:

```text
trajecta --format json job prune
```

The plan is informational in `0.1.0-alpha.1`; it does not delete result
directories or catalog history.
