---
title: Shutdown and memory pressure
description: Prepare long Trajecta runs for machine shutdown, respond to external memory pressure, and recover workers stopped by the operating system.
---

# Shutdown and memory pressure

Long trajectory runs share a machine with the operating system, file cache,
interactive applications, and sometimes other scientific programs. Trajecta
uses scheduler reservations to limit its own admitted workload and separately
checks current host memory before starting another worker.

This page covers three distinct situations:

| Situation | What happens to the queue | What happens to active workers |
| --- | --- | --- |
| Available host memory falls below the configured reserve | New dispatch pauses | Workers continue |
| A safe cancellation is requested | Queued work becomes cancelled; a running worker stops at a macro-step boundary | The worker closes a partial result and exits |
| The machine or worker process stops abruptly | No further dispatch until the daemon returns | The active attempt is reattached, reconciled from a terminal manifest, or marked interrupted |

## Prepare a workstation for a long run

Inspect the configured pool:

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get resources.memory_reserve_mib
```

Then inspect the selected Profile:

```text
trajecta --project PROJECT project show
trajecta --project PROJECT project get profiles.PROFILE
```

`execution.memory_budget_bytes` is the request for one worker. Multiply it by
the number of workers that can be active at once when estimating the queue's
maximum configured working set. Leave room for the daemon, SQLite page cache,
meteorological readers, the operating system, and other applications through
`memory_reserve_mib`.

For a shared workstation, it is often more predictable to reduce
`resources.cpu_slots` or raise the reserve before submitting a batch. Changing
the machine configuration affects later admission; it does not rewrite the
resource request stored for an existing series.

Finish the pre-run check with:

```text
trajecta --project PROJECT doctor --deep
trajecta job list
```

The first command exercises the selected filesystem, data readers, existing
locks, and a temporary SQLite WAL cycle. The second shows work already using
the local pool.

## Observe memory while a run is active

Open a second terminal and follow the job:

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

Resource events include the observations available from the worker and daemon.
Use `job status` for the current scheduler reservation:

```text
trajecta --format json job status JOB_ID
```

Operating-system tools remain useful for total host memory because the daemon's
catalog describes Trajecta requests, not the complete set of unrelated
processes. On Windows, Task Manager or Resource Monitor gives the machine-wide
view. On Ubuntu 24.04, `free -h` and `ps` provide the corresponding snapshot.

## Respond to external memory pressure

The warning code is:

```text
daemon.external_memory_pressure
```

It means the host's currently available memory is below
`resources.memory_reserve_mib`. The attempt remains queued and receives one
warning rather than a repeated message on every scheduler poll.

A practical response is:

1. Check which non-Trajecta programs changed the host's available memory.
2. Let an active Trajecta worker finish when its memory use remains stable.
3. Close or postpone unrelated memory-heavy work.
4. Watch `job events` until dispatch resumes.
5. Adjust the pool or future Profile requests before the next batch when the
   pressure recurs under the same workload.

Safe cancellation is available when a running task should release its
reservation:

```text
trajecta job cancel JOB_ID
trajecta job wait JOB_ID
```

The worker reaches a macro-step boundary, finalizes the partial result, and
enters `cancelled`. The wait can last as long as the current macro step and its
output closeout.

!!! tip "Cancellation does not pause a simulation"

    A later rerun starts from the resolved input in a new attempt. The cancelled
    particle state is not a resume checkpoint.

## Handle an operating-system OOM termination

If the operating system terminates a worker, the daemon can no longer receive
its normal closeout. The catalog first reflects a lost worker and then records
an interrupted attempt. Typical event codes are:

```text
worker.lost
run.interrupted.worker_lost
```

Read the final catalog state and retain the complete attempt directory:

```text
trajecta --format json job status JOB_ID
trajecta --format json job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

An interrupted directory may contain a running manifest, a partial SQLite
database, a WAL, worker output, and temporary provenance files. These files
describe the point reached by the old attempt. A later rerun receives a new
run ID and a separate directory.

Before rerunning, compare the observed peak use with the Profile's memory
budget. Common adjustments are:

- lower the number of simultaneously admitted jobs;
- lower `execution.worker_threads` when thread-local working memory is
  significant;
- provide a realistic `execution.memory_budget_bytes` for admission;
- increase `resources.memory_reserve_mib` on a shared workstation;
- move the batch to a machine with a larger physical memory margin.

Run `doctor --deep` after an OOM event when the worker was writing SQLite or
provenance at the time of termination. It checks the current filesystem and
SQLite runtime before another attempt is submitted.

## Stop before a planned shutdown

For a running job, request safe cancellation and wait for `cancelled`:

```text
trajecta job cancel JOB_ID
trajecta job events JOB_ID --follow
```

For several jobs, inspect `job list`, cancel active series individually, and
wait for each current attempt. Queued attempts can also be cancelled without
launching a worker.

Once all current attempts are terminal, the machine may shut down without a
live result writer. A later rerun is explicit:

```text
trajecta job rerun JOB_ID
```

The existing result directory remains associated with the cancelled attempt.

## Recover after an unplanned shutdown

Use the same machine configuration and project after startup:

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
```

The first runtime command starts the local daemon if needed. Startup
reconciliation may reattach a still-live worker, accept an already completed
terminal manifest, or mark a missing worker interrupted. Wait for that state
to settle before calling `job rerun`.

The complete post-shutdown sequence is listed in the
[recovery runbook](recovery.md).
