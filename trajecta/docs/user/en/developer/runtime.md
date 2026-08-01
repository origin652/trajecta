---
title: Output and runtime
description: Worker execution, durable job control, SQLite output, manifests, provenance, cancellation, and recovery internals.
---

# Output and runtime

## Job lifecycle

The CLI admits a request to the local durable catalog. The scheduler checks CPU
and memory pools, allocates an attempt, and launches a separate worker process.
Daemon ownership allows admitted work to survive a client disconnect.

The catalog records state transitions and monotonically sequenced events. A
rerun creates a new attempt under the same job series. Recovery reconciles
worker leases, terminal manifests, and interrupted processes without rerunning a
completed attempt.

## Output commit

The core writes `particles.sqlite` in WAL mode with foreign keys and bounded
transactions. It records the run, particles, masses, physical output events,
states, and terminations. Terminal completion performs integrity checks and a
WAL checkpoint before manifest closure.

The run manifest binds software, resolved inputs, execution settings, numerical
settings, row counts, termination classes, mass ledger, SQLite identity, and
provenance identity. The provenance bundle maps sampled fields back to source
records and transformations.

## Cancellation and interruption

Safe cancellation requests a worker checkpoint and classified terminal close.
Force cancellation retains forensic state when a cooperative close does not
arrive. Process loss, host shutdown, memory pressure, and disk errors are
reconciled as explicit failure or interruption paths. Cleanup code must retain
evidence needed by `result verify` and operations diagnosis.

No daemon or worker path may perform scientific calculation independently of
`trajecta-core`.
