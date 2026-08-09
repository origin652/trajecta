---
title: Trajecta operations manual
description: Run the local Trajecta queue, size its resource pool, monitor workers, and keep completed results available across daemon restarts.
---

# Operations manual

Trajecta runs its queue and workers on the local machine. A command submitted
from the CLI enters a persistent job catalog, waits for CPU and memory, and is
then assigned to a worker process. Closing the submitting terminal does not
remove a job that the daemon has already accepted.

This section covers the machine-level work around a scientific run: resource
admission, queue monitoring, daemon restarts, interrupted workers, storage, and
result readers. Configuration of a Case or RunProfile is covered in the
[Concepts](../concepts/index.md) and [How-to Guides](../how-to/index.md).

## Files and processes involved

A local installation has three kinds of durable state:

| Item | Selected by | Contents |
| --- | --- | --- |
| Machine configuration | `--config`, `TRAJECTA_CONFIG`, or the platform default | Resource pool, daemon endpoint, job-catalog path, monitoring cadence, and local Profile templates |
| Job catalog | The selected machine configuration | Job series, attempts, state transitions, events, worker leases, and verification status |
| Result directory | Project and RunProfile | Run manifest, `particles.sqlite`, provenance bundle, reports, and files retained after interruption |

The daemon is a local coordinator. It uses a named pipe on Windows and a Unix
socket on Linux. Numerical work runs in child worker processes, so queue
inspection remains responsive while a simulation is active. The job catalog
and result directories remain on disk when an idle daemon exits.

Use the same configuration path whenever several shells operate on one queue:

```text
trajecta --config workstation.toml job list
trajecta --config workstation.toml job status JOB_ID
trajecta --config workstation.toml job events JOB_ID --follow
```

A command that selects another configuration may connect to a different daemon
and catalog on the same computer.

## Attempt lifecycle

Each submission creates a job series and its first attempt. A rerun stays in
the same series and receives a new run ID and attempt number.

| State | Operational meaning | Usual next state |
| --- | --- | --- |
| `queued` | The request is durable and waiting for admission | `starting` or `cancelled` |
| `starting` | Resources have been reserved and a worker is being launched | `running`, `failed`, or `interrupted` |
| `running` | The worker owns the result directory and advances the simulation | `complete`, `completed_with_particle_errors`, `cancelling`, `failed`, or `interrupted` |
| `cancelling` | A safe cancellation is waiting for a numerical boundary | `cancelled` or `interrupted` |
| `complete` | The run finished with a complete manifest and no abnormal particle terminations | Terminal |
| `completed_with_particle_errors` | Output completed while one or more particles ended abnormally | Terminal |
| `cancelled` | Cooperative cancellation completed its output closeout | Terminal |
| `failed` | Startup, execution, or output finalization returned an error | Terminal |
| `interrupted` | The worker disappeared or was force-stopped before normal closeout | Terminal |

The current snapshot is available through:

```text
trajecta --format json job status JOB_ID
```

The event log supplies the transitions that led to that snapshot:

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

Reading events does not consume them. Several terminals or monitoring tools can
follow the same queue independently.

## Resource admission

The machine configuration defines the shared capacity:

```text
resources.cpu_slots
resources.memory_pool_mib
resources.memory_reserve_mib
```

The selected RunProfile contributes two scheduler requests:

| RunProfile field | Scheduler request |
| --- | --- |
| `execution.worker_threads` | The same number of CPU slots |
| `execution.memory_budget_bytes` | Memory rounded upward to a whole MiB |

Schedulable memory is `memory_pool_mib - memory_reserve_mib`. An attempt starts
when both its CPU and memory request fit the currently free capacity. Resource
reservation prevents the daemon from admitting more configured work than the
pool can hold; the operating system still observes memory used by other
applications.

### Queue order and backfill

The scheduler begins with first-in, first-out order. When the oldest attempt is
temporarily too large for the free capacity, a younger attempt that fits may be
started. This safe backfill keeps an idle CPU or memory slot useful without
forgetting the queue head.

The oldest attempt may be bypassed at most three times. After the third bypass,
newer work waits until enough capacity is available for that head attempt. A
large request therefore makes progress even when a stream of smaller requests
continues to arrive.

### External memory pressure

The daemon compares available host memory with the configured reserve. When
available memory drops below that reserve, new dispatch pauses. Running workers
keep their reservations and continue unless the operating system or an
operator stops them.

Each affected queued attempt receives one
`daemon.external_memory_pressure` warning for that pressure episode. Queue
dispatch resumes after host memory recovers. The
[shutdown and memory guide](shutdown-memory.md) explains how to distinguish this
condition from a worker killed by the operating system.

## Daemon start, idle exit, and restart

Runtime commands connect to the selected local endpoint. If no daemon is
listening, the CLI starts one and waits for it to become ready. The daemon holds
an instance ID, process ID, and process-start token; this identity prevents a
reused process ID from being treated as an existing worker owner.

When the queue has no active work, `daemon.idle_shutdown_seconds` controls how
long the daemon stays alive. An idle exit closes the process and endpoint. It
does not remove catalog rows, events, result directories, or verification
records. Setting the value to `0` keeps the daemon available until the machine
or process is stopped.

On startup, reconciliation handles each durable attempt according to its
state:

| Catalog and worker state | Reconciliation action |
| --- | --- |
| Worker identity and lease are still valid | Reattach and emit `worker.reattached` |
| A matching valid terminal manifest already exists | Reconcile the terminal state and emit `worker.terminal_reconciled` |
| Active catalog row has no matching worker or terminal manifest | Mark the attempt interrupted through `worker.lost` and `run.interrupted.worker_lost` |
| Attempt is still queued | Leave it queued for ordinary admission |
| Attempt is terminal | Keep the terminal state; no new attempt is created |

The [recovery runbook](recovery.md) gives the command sequence for examining
these outcomes after a reboot or unexpected process loss.

## Monitoring a live queue

A short operational check can use three views:

```text
trajecta job list
trajecta --format json job status JOB_ID
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

`job list` gives one current entry per visible series. `job status` adds queue
position, resource request, run identity, timestamps, and output path. The
event stream records progress, resource samples, diagnostics, and artifact
transitions.

For a long-running local queue, store the durable event sequence from
`data.sequence`. Reconnect with `--since SEQUENCE` after a monitoring client
restarts. The outer JSONL sequence counts lines in that particular stream and
is not the reconnect cursor.

## Reading a result while it is being written

Result commands use read-only snapshots. During an active run, SQLite readers
observe data only through the current indexed high-water boundary. They do not
take ownership of the writer's checkpoint or lifecycle closeout.

Useful live checks include:

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 42
```

Counts may grow between snapshots. A terminal result adds the final manifest,
provenance closeout, and a checkpointed or absent WAL. Run full verification
after the attempt is terminal:

```text
trajecta result verify JOB_ID --full
```

For direct SQLite access, follow the read-only layout and snapshot notes in the
[results reference](../reference/results-sqlite.md).

## Routine operating checks

Before a set of long runs:

1. Select the intended machine configuration and run `config validate`.
2. Run `doctor --deep` against the finalized project.
3. Check free space under the catalog, data, and output roots.
4. Compare each Profile request with the CPU pool and schedulable memory.
5. Submit detached jobs and retain their complete JSON receipts.

After completion:

1. Confirm the terminal state with `job status`.
2. Run `result verify --full` for results that will be retained or analyzed.
3. Generate `run-report.md` when a human-readable run summary is useful.
4. Archive or copy the complete terminal result directory as one unit.
5. Review `job prune` when estimating removable storage; this release returns
   a dry-run plan only.

Related procedures:

- [Shutdown and memory pressure](shutdown-memory.md)
- [Storage, SQLite, and WAL](storage-sqlite.md)
- [Recovery and interrupted attempts](recovery.md)
- [Troubleshooting by symptom or code](troubleshooting.md)
