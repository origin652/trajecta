---
title: Trajecta operations manual
description: Operate local resource pools, queues, daemons, workers, and concurrent readers safely.
---

# Operations manual

Trajecta runs a local control plane. The CLI submits work to a daemon; the
daemon persists series, runs, attempts, events, resources, and leases in a job
database; workers execute immutable resolved inputs and write result
directories.

## Resource pools and scheduling

`resources.cpu_slots`, `resources.memory_pool_mib`, and
`resources.memory_reserve_mib` define admission capacity. Each RunProfile asks
for worker threads and a memory budget. The scheduler admits a job only when
the request fits available capacity.

Memory reserve must remain below the pool. Size the pool for Trajecta workers,
then retain reserve for the daemon, operating system, readers, and transient
allocation. External memory pressure pauses new dispatch. It does not cancel a
running worker.

## Daemon lifecycle

The CLI starts or reconnects to the local daemon as needed. The daemon records
an instance identity and process start token so a recycled process ID cannot
impersonate an old lease. Idle shutdown releases the process while the job
database and completed artifacts remain on disk.

After restart, the daemon reconciles catalog state, worker leases, terminal
manifests, and retained forensics. Completed attempts stay terminal and are not
rerun. An admitted worker may be reattached only when identity and lease checks
succeed.

## Worker lifecycle

A worker moves through queued, running, and terminal states. Safe cancel waits
for a numerical boundary and produces a terminal manifest, finalized SQLite,
provenance, and a zero or absent terminal WAL. Force cancellation preserves
available partial artifacts and forensic evidence.

Use event streaming for automation and `job status` for a point-in-time view.
Concurrent result readers must use the indexed high-water contract and read
snapshots; writers retain sole authority over lifecycle finalization.
