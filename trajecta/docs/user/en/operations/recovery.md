---
title: Recovery after power loss, OOM, disk failure, or interrupted output
description: Preserve evidence and recover Trajecta jobs after abrupt shutdown, worker loss, memory pressure, disk exhaustion, or SQLite/WAL problems.
---

# Recovery and forensics

Recovery begins by preserving state. Do not remove temporary files, WAL files,
job rows, or partial result directories during the initial assessment.

## Abrupt shutdown or worker loss

1. Restart the machine and run `trajecta doctor --deep` with the same config.
2. Run `trajecta job list`, then `job status JOB_ID` for non-terminal work.
3. Read `job events JOB_ID --since 0` and record the last durable transition.
4. Inspect the run directory without editing it.
5. Allow daemon reconciliation to mark an unreattachable worker as interrupted.
6. Use `job rerun JOB_ID` only after the attempt reaches a stable terminal state.

A recovered terminal manifest is authoritative when its run identity matches
the catalog. A missing worker with no valid terminal manifest becomes an
interrupted attempt. Existing partial SQLite, WAL, manifest, temporary, and
forensic files remain evidence.

## OOM and external memory pressure

External pressure emits `daemon.external_memory_pressure` once for an affected
queued attempt and pauses dispatch. Wait for memory to recover or lower future
requests. Running workers continue unless the operating system terminates one.

An OS-level worker loss is reconciled as interruption. Preserve the operating
system event, job events, worker stderr, manifest state, and peak-memory
evidence. Lower `execution.memory_budget_bytes`, reduce concurrent jobs, or
increase the configured pool only after identifying the actual working set.

## Disk exhaustion or write failure

Stop admitting new work. Restore free space on the same filesystem without
touching active result directories. Run deep doctor, inspect the output root,
then check each affected job. A write failure may leave a partial manifest,
SQLite sidecar, or provenance temporary file; retain all of them.

Move a terminal result only as a complete directory after full verification.
Do not redirect a live writer by moving its files.

## SQLite and WAL anomalies

Never delete `particles.sqlite-wal` or `jobs.sqlite3-wal` to make a warning
disappear. First copy the entire inactive directory for forensic preservation.
Use `result inspect` and `result verify`; use `doctor --deep` for the job
database environment. A successful terminal result requires SQLite integrity
and a zero or absent terminal WAL.

If integrity fails, keep the original bytes read-only. A rerun creates a new
attempt. It does not repair or replace the damaged evidence.

## Completed work during queue recovery

Daemon restart does not resubmit completed tasks. Check the series, run ID,
attempt number, terminal manifest, and artifact fingerprint. `job forget`
removes a terminal record from routine listing while leaving artifacts and
audit summaries intact. `job prune` only previews candidates in this release.
