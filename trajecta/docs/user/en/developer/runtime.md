---
title: Output and runtime
description: Durable job control, daemon and worker processes, output transactions, cancellation, attempts, and recovery internals.
---

# Output and runtime

The runtime surrounds one `trajecta-core` simulation with durable local job
control. It accepts a validated request, reserves host resources, starts a
separate worker, records progress, and reconciles the final result. The worker
uses the same numerical runner exercised by direct core tests.

Two SQLite databases appear in this design:

| Database | Owner | Purpose |
| --- | --- | --- |
| Local job catalog | `trajecta-job` | Queue state, resource reservations, worker leases, events, attempts, verification history |
| `particles.sqlite` in an attempt directory | `trajecta-core` | Scientific particle, mass, event, state, and termination products |

They have independent schemas and lifecycles. Job status can be read without
opening a scientific result, and result inspection can continue after the daemon
has stopped.

## Request admission

`trajecta-cli` turns `run` arguments into a `SubmitRequest`. An input is either a
finalized project plus a named Profile, or an explicit resolved Case and
RunProfile pair. The request also contains:

- CPU slots reserved in the local resource pool;
- memory reservation in mebibytes;
- worker-thread count, which cannot exceed the CPU reservation.

The daemon validates the request against its configured capacity before writing
it to the catalog. Admission creates two UUIDv7 identities:

| Identity | Lifetime |
| --- | --- |
| Job series ID | Groups the original submission and every later rerun |
| Run ID | Identifies one immutable attempt and its output directory |

Attempt numbers begin at one and increase within a series. A rerun copies the
validated input and resource request into a new queued attempt; it does not
reuse or overwrite an earlier directory.

## Durable state machine

The scheduler-visible states are:

```text
queued -> starting -> running -> complete
                         |       completed_with_particle_errors
                         |       failed
                         |       interrupted
                         +-> cancelling -> cancelled
```

`starting` covers resource reservation and process launch. `cancelling` records
a cooperative request while the worker waits for a safe macro-step boundary.
Terminal states cannot transition again and are never dispatched by recovery.

Every transition is applied through a catalog transaction. The state row,
resource reservation, worker lease, diagnostic, and persistent event are
updated as one catalog operation where the transition requires them. Illegal
state edges fail with a typed backend error.

## Catalog storage

`LocalJobCatalog` uses SQLite with foreign keys, WAL journal mode, a five-second
busy timeout, and full synchronous durability for daemon-owned queue changes.
Worker telemetry uses a separate connection with normal synchronous mode because
heartbeats and progress samples are advisory; attempt state transitions remain
on the daemon connection.

The catalog records:

- one current snapshot for each attempt;
- resource requests and output-directory allocation;
- daemon and worker leases;
- monotonically sequenced fan-out events;
- full-verification identity for successful attempts;
- supersession relationships between reruns;
- visibility changes made by `job forget`.

Event reads never consume rows. Two readers can ask for the same sequence range,
and `job events --follow` continues from its own cursor. This property lets a
terminal, a monitoring process, and an AI assistant observe the same job without
competing for messages.

## Scheduler and resource pools

`plan_dispatch` is a pure scheduling function. It receives configured capacity,
current active reservations, the durable FIFO queue, and an external-memory-
pressure flag. It returns selected run IDs plus any queue-head bypass counters
that the catalog must persist atomically.

Ordinary behavior is FIFO. If the head does not fit in the currently available
CPU or memory pool, a smaller later request may backfill unused capacity. The
same blocked head can be bypassed at most three times. Once it reaches that cap,
the scheduler reserves future capacity for it and stops selecting later work.

Dispatch pauses when external memory pressure is active. A request larger than
the daemon's total capacity is rejected at submission, so an impossible job
cannot remain at the head indefinitely.

The scheduler only reserves declared capacity. The operating system can still
reduce available memory after admission. Worker termination and recovery handle
that condition; the resource model does not claim to control other processes on
the host.

## Daemon ownership and local IPC

One daemon owns a catalog at a time. Its lease contains an instance ID, process
ID, process-start token, and heartbeat. The start token distinguishes the
original process from a later process that reused the same numeric PID.

CLI clients communicate with the same-release daemon through the local IPC
transport. Requests and responses are typed by `trajecta-job::ipc`; a frame is
bounded to 4 MiB. Job events remain in the catalog, so IPC is used to request a
query rather than to hold a transient event queue.

The daemon dispatch loop is non-blocking:

1. Read queued attempts and active reservations.
2. Calculate one dispatch plan.
3. Move selected rows to `starting` and persist bypass updates.
4. Launch the current executable in hidden `__worker` mode.
5. Probe its process identity and attach the worker lease.
6. Mark a controlled launch or lease failure with a terminal diagnostic.

Windows launches the worker without a visible console window. Ubuntu uses an
independent child process. Foreground and detached CLI modes differ in whether
the client waits; both submit to the same daemon and create the same attempt
shape.

## Worker execution

The worker receives one run ID and reopens the catalog. It validates that its
lease and start token match the attempt before starting scientific work. It then:

1. reloads and validates the resolved project input;
2. constructs the production `SimulationRunner` for the allocated attempt;
3. changes catalog state to `running`;
4. executes macro steps through `trajecta-core`;
5. reports heartbeat and progress at a bounded sampling interval;
6. closes output artifacts and returns the manifest terminal state to the
   catalog.

Progress includes the completed macro-step count, simulation time, active
particles, and normal and abnormal termination totals. Resource observation
currently has a durable shape for wall time, CPU time, and RSS; fields that are
not sampled remain absent rather than being estimated.

Scientific logic is not reproduced in the worker adapter. The adapter resolves
documents, supplies runner control, and maps `RunOutcome` to the catalog.

## Scientific output transaction

An attempt directory receives a running manifest, `particles.sqlite`, temporary
provenance files, worker logs, and any retained forensic material. The SQLite
sink uses the versioned schema described in
[Result and SQLite reference](../reference/results-sqlite.md).

During a run:

- foreign keys remain enabled;
- output events define physical sample times;
- particles are inserted before their masses and states;
- lifecycle events use bounded transactions;
- ordinary scheduled output commits its transaction;
- SQLite remains in WAL mode so indexed readers can observe a high-water
  snapshot without blocking the writer.

Terminal completion follows a strict order:

1. Commit pending SQLite rows and set the terminal run row.
2. Run `PRAGMA wal_checkpoint(TRUNCATE)` and require an empty terminal WAL.
3. Check schema pragmas and `PRAGMA integrity_check`.
4. Count every public table and compute the canonical SQL digest.
5. Close the writer connection and hash the standalone SQLite database.
6. Stream the provenance bundle from sorted temporary runs while validating it
   against the SQLite sample cursor.
7. Validate the temporary bundle, atomically rename it, and compute content and
   canonical-output digests.
8. Add row counts, termination summaries, mass ledgers, SQLite identity, and
   provenance identity to the terminal manifest.
9. Write the manifest through a same-directory temporary file, `sync_all`, and
   atomic rename.

This sequence makes the terminal manifest the last binding record. A `complete`
manifest therefore refers to already-closed artifacts with known identities.

If bundle or SQLite finalization fails, the runner aborts or quarantines the
affected output and writes a failed manifest when it can do so safely. Temporary
files and forensic files have different roles: successful cleanup removes
ephemeral sort runs, while interrupted material remains available for diagnosis.

## Cancellation

Safe and force cancellation have different artifact semantics.

### Safe cancellation

The daemon writes a safe-cancel request into the worker lease. At a sampled
macro-step boundary, `CatalogRunnerControl` returns `Cancel` to the runner. The
runner finishes the legal partial output, writes a `cancelled` manifest, and the
catalog adopts that terminal state.

!!! tip "Safe cancellation has no resume checkpoint"

    `job rerun` creates a new attempt from the resolved input.

### Force cancellation

The daemon verifies both PID and process-start token before stopping the worker.
It then inspects the attempt:

- if the worker completed its terminal manifest during the narrow stop race,
  the catalog adopts that validated terminal state;
- otherwise the attempt becomes `interrupted`, and forensic files are retained.

Queued work can be cancelled without starting a worker. Repeating cancellation
against a terminal attempt does not reopen it.

## Restart and crash recovery

After daemon restart, recovery compares durable leases with platform process
identity and attempt manifests. Each active attempt receives one disposition:

| Observation | Recovery action |
| --- | --- |
| Worker PID and start token are still live | Reattach the lease to the new daemon instance |
| A valid terminal manifest is already present | Reconcile that state into the catalog |
| No matching live worker and no valid terminal result | Mark the attempt interrupted and keep its directory |

The process-start token prevents an unrelated process with a reused PID from
being treated as a Trajecta worker. Recovery also releases the attempt's CPU and
memory reservation when it becomes terminal, allowing queued jobs to proceed.

Completed attempts are immutable. Daemon restart, client restart, or a new queue
submission does not rerun them. `job rerun` is the explicit operation that
creates another attempt.

## Attempt history and cleanup planning

History keeps all attempts in ascending attempt order. A successful full verify
can record the canonical output identity on a complete attempt. If a later
attempt in the same series is fully verified, history can mark the earlier
attempt as superseded while leaving its files intact.

`job forget` changes catalog visibility; it does not delete an attempt directory.
The current prune operation returns a deterministic dry-run plan listing paths
eligible for later cleanup. No production command removes those paths in
`0.1.0-alpha.1`.

## Runtime change checklist

| Area | Checks to add or rerun |
| --- | --- |
| State transition | Legal and illegal edge tests, event order, resource release |
| Catalog schema | Fresh create, reopen, WAL behavior, concurrent readers, schema version rejection |
| Scheduler | FIFO order, three-bypass cap, capacity overflow, external pressure |
| IPC message | Encode/decode round trip, frame bound, same-release client/server |
| Worker launch | PID token probe, launch failure, lease-attachment race, retained log |
| Safe cancellation | Macro-step polling, cancelled manifest, final event, partial result verification |
| Force cancellation | PID reuse protection, terminal race reconciliation, interrupted forensic |
| Output sink | Transaction rollback, table counts, reader snapshots, terminal checkpoint |
| Manifest or provenance | Atomic replacement, identity mismatch, failure quarantine, full verify |
| Rerun/history | New run ID, increasing attempt, completed-attempt preservation, dry-run prune |

Runtime integration tests should launch the production daemon and worker binary.
Mock process managers remain useful for rare launch and recovery races, while the
end-to-end contract confirms that the adapters reach the real runner.
