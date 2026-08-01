---
title: Troubleshooting by symptom and diagnostic code
description: Find Trajecta recovery actions from an observed symptom or a stable diagnostic code.
---

# Troubleshooting index

Preserve the complete machine envelope when reporting a failure. In machine
mode, stdout contains one JSON envelope or a JSONL stream; stderr stays empty
for structured application diagnostics.

## Start from a symptom

| Symptom | First checks | Likely code family |
|---|---|---|
| Project stays configured | `project status`, `project data-plan` | `project.lock_missing`, `project.finalize_pending` |
| Finalize changes no locks | Review preflight diagnostics and actual data roots | `project.finalize_preflight_failed`, `data.*` |
| Job remains queued | `job events --follow`, resource configuration | `daemon.external_memory_pressure`, `scheduler.*` |
| Worker vanished | daemon reconciliation and retained run directory | `worker.lost`, `run.interrupted.worker_lost` |
| Result is partial | `result inspect`, manifest and SQLite state | `result.inspect_sqlite_unavailable`, `result.artifact_missing` |
| Full verify fails | Preserve result; compare manifest, SQLite, bundle | `result.*`, `doctor.sqlite_*` |
| Disk use grows | Check output schedule, WAL, free space, active writer | `project.output_root_*`, `result.io` |
| Data query fails | Inspect lock coverage and reader capability | `data.lock_*`, `met.*` |

## Start from a diagnostic code

| Code | Meaning | Action |
|---|---|---|
| `project.path_escape` | A project-relative path leaves the project jail | Replace it with a normalized path below the project root |
| `project.lock_missing` | A selected dataset has no finalized lock | Prepare data, inspect the plan, then finalize explicitly |
| `project.finalize_preflight_failed` | One or more finalize checks failed | Read nested diagnostics; no lock was changed |
| `daemon.external_memory_pressure` | Host memory pressure paused dispatch | Reduce pressure or future concurrency; do not kill healthy workers |
| `run.interrupted.worker_lost` | A running worker disappeared without a valid completion | Preserve forensics, allow reconciliation, then create a new attempt |
| `result.manifest_invalid` | Manifest shape or content is invalid | Keep bytes unchanged and inspect the producing attempt |
| `result.artifact_missing` | A terminal result lacks a required artifact | Treat verification as failed; retain the directory |
| `result.inspect_sqlite_unavailable` | Manifest is readable but SQLite inspection failed | Use partial inspect output and preserve SQLite sidecars |
| `result.manifest_sqlite_count_mismatch` | Manifest row counts disagree with SQLite | Stop analysis and retain both identities |
| `doctor.sqlite_integrity_failed` | Deep doctor's temporary SQLite check failed | Check filesystem health, permissions, and SQLite runtime environment |

The [complete generated diagnostic index](../reference/diagnostics.md) lists
every public code found in production source or executable regression tests.
