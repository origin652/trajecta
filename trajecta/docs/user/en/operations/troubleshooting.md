---
title: Troubleshooting Trajecta by symptom and diagnostic code
description: Diagnose Trajecta configuration, project, queue, worker, storage, and result problems from human output or stable machine-readable codes.
---

# Troubleshooting index

Start with the command that observed the problem and rerun it in JSON mode when
possible:

For example, inspect a project status as structured output:

```text
trajecta --format json --project PROJECT project status
```

Application diagnostics appear in `diagnostics[]`. Each item has a stable
`code` and a contextual `message`; some operations include nested diagnostics
for several failed checks. CLI usage errors return exit code `2`. A parsed
command that encounters a product or runtime error returns `1`.

For a stream such as `job events --follow`, use JSONL:

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

Keep the complete JSON or JSONL response when comparing two attempts. Machine
mode writes structured application responses to stdout and normally leaves
stderr empty.

## Configuration and doctor

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| The CLI selects an unexpected configuration | `trajecta config path` with the same global options | `config.not_found`, `config.invalid_path` | Check `--config`, then `TRAJECTA_CONFIG`, then the platform default |
| A setting cannot be saved | `config get KEY`, then repeat `config set` in JSON mode | `config.invalid_key`, `config.invalid_value`, `config.invalid_schema`, `config.write_failed` | Correct the selector or value; the previous valid file remains in place after a rejected update |
| Doctor rejects the selected machine file | `config validate` | `doctor.config_invalid`, `config.invalid_schema` | Correct the machine configuration before checking the project |
| Deep doctor cannot write its probe | Project root and filesystem permissions | `doctor.filesystem_unwritable`, `doctor.cleanup_failed` | Restore create, sync, rename, and cleanup access under the selected project |
| Deep doctor fails its SQLite cycle | Free space and filesystem health | `doctor.sqlite_create_failed`, `doctor.sqlite_integrity_failed`, `doctor.sqlite_checkpoint_failed` | Correct the filesystem or SQLite runtime, then rerun deep doctor |
| Deep doctor cannot inspect meteorology | Data roots, reader backend, and supported files | `doctor.data_inspect_failed`, `doctor.data_scan_limit` | Confirm the root contents and the Profile reader choice |

Configuration path selection and resource sizing are described in
[Configuration and doctor](../getting-started/configuration.md).

## Project and data preparation

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| Project remains `draft` | `project status`, `project validate` | `project.document_invalid`, `project.invalid_index` | Complete missing Case or Profile fields and correct document types |
| Project remains `configured` | `project data-plan`, `project status` | `project.lock_missing`, `project.finalize_pending`, `project.output_root_pending` | Prepare listed files and create the output root before explicit finalize |
| Finalize exits without changing locks | Full JSON response from `project finalize` | `project.finalize_preflight_failed` with nested `data.*` or `project.*` codes | Correct every preflight item, then rerun finalize |
| A Profile selects the wrong Case | `project show`, Profile `case_path`, and indexed Cases | `project.profile_case_mismatch`, `run.profile_case_mismatch` | Point the Profile at one indexed resolved Case |
| A project path is rejected | The exact relative value in the project index | `project.path_escape`, `project.output_root_invalid` | Use a normalized path below the project root |
| Lock content no longer matches local data | `doctor --deep`, `data inspect`, DatasetLock file list | `project.lock_invalid`, `doctor.lock_invalid`, `data.lock_invalid` | Restore the locked bytes or finalize against the intended complete dataset |
| The Case needs frames outside the lock | Resolved Case coverage and lock coverage | `data.lock_requirements_mismatch` | Prepare the required temporal and spatial coverage, then finalize again |

`project finalize` completes all preflight checks before replacing lock
bindings. A failed preflight leaves existing lockfiles unchanged.

## Queue and daemon

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| Job stays queued while the machine is idle | `job status` resource request and configured pool | `scheduler.queued`, `job.invalid_resources`, `daemon.invalid_capacity` | Ensure the request can fit the total CPU and schedulable memory pool |
| Smaller jobs run ahead of an older large job | Queue positions and dispatch events | `scheduler.dispatched` | Safe backfill can bypass the head three times; the head is then reserved |
| Queue pauses while resources appear free | Host-wide available memory and event log | `daemon.external_memory_pressure` | Reduce unrelated memory pressure or revise the reserve before a later batch |
| Runtime command cannot connect | Selected configuration, endpoint path, and existing daemon process | `daemon.launch_failed`, `daemon.start_timeout`, `daemon.ipc_failed`, `job_backend.unavailable` | Confirm the configuration path and local IPC permissions, then retry the command |
| Daemon endpoint is already owned | Process identity and start token | `daemon.bind_failed`, `daemon.owner_probe_failed`, `daemon.identity_failed` | Check for an existing daemon using the same catalog and endpoint |
| Catalog operation fails | Free space, directory permissions, and SQLite sidecars | `job_backend.storage`, `job_backend.conflict` | Stop new submissions, restore catalog storage, and reconnect with the same configuration |

The [operations manual](index.md) explains admission, safe backfill, idle exit,
and restart reconciliation.

## Worker and interrupted run

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| Worker never reaches `running` | Events, worker stderr, output-directory creation | `worker.launch_failed`, `worker.start_failed`, `worker.lease_timeout` | Correct the launch or filesystem condition before rerunning |
| Worker disappears during a run | Process list, job events, and attempt directory | `worker.lost`, `run.interrupted.worker_lost` | Wait for terminal reconciliation, inspect the partial result, then rerun if needed |
| Daemon returns while a worker is still alive | Events and worker identity | `worker.reattached` | Continue monitoring the same attempt |
| Terminal manifest exists after daemon loss | Events, manifest run ID, and catalog run ID | `worker.terminal_reconciled` | Inspect and fully verify the result |
| Input changed after admission | Resolved input files and stored hashes | `worker.input_changed` | Restore the admitted bytes or submit a new run from finalized inputs |
| Safe cancellation takes time | Current macro-step progress | `job.safe_cancel_requested` | Allow the worker to reach the next safe boundary; use force only when immediate termination is required |

See [Recovery and interrupted attempts](recovery.md) for the full restart and
rerun sequence.

## Results, SQLite, and reports

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| Manifest cannot be parsed | Preserve the file and run `result inspect` in JSON mode | `result.manifest_invalid`, `result.encoding` | Locate the producing attempt and read worker closeout events |
| Manifest is readable but SQLite is unavailable | Main database, WAL, SHM, filesystem access | `result.inspect_sqlite_unavailable`, `result.sqlite_invalid` | Keep the files together and treat the inspection as partial |
| Terminal result lacks a required file | Artifact inventory from `result inspect` | `result.artifact_missing` | Inspect the terminal event and rerun after correcting the closeout failure |
| Manifest row counts differ from SQLite | Full verification output | `result.manifest_sqlite_count_mismatch` | Keep the directory unchanged and create a separate rerun |
| A particle cannot be read | Particle ID range and selected run | `result.particle_not_found`, `result.trajectory_particle_id_out_of_range` | Select an ID present in the result and check which attempt was resolved |
| Report creation fails | Result-directory permissions and free space | `report.write_failed`, `report.refresh_failed` | Restore write and atomic-rename access, then regenerate `run-report.md` |
| WAL grows during a run | Job state, output event cadence, and free space | `result.io` when a write fails | Monitor the active writer; terminal closeout checkpoints the WAL |

Detailed WAL and moving procedures are in
[Storage, SQLite, and WAL](storage-sqlite.md).

## Meteorological reader

| Symptom | Inspect | Common codes | Next action |
| --- | --- | --- | --- |
| Reader cannot open a data file | `data inspect`, file family, and selected reader | `data.inspect_failed`, `data.unknown_format`, `met.runtime` | Confirm that the file belongs to a supported family and is readable by the selected backend |
| Backward run lacks time support | Resolved time range and lock frames on both sides | `met.missing_symmetric_time_support` | Prepare the additional frame required around the query interval |
| Machine JSON is mixed with native output | Command output options | `met.machine_stdout_requires_file` | Direct native probe output to a file when using machine mode |

## Look up a code

The [generated diagnostic index](../reference/diagnostics.md) lists every
public code found in the CLI, daemon, worker, scheduler, project, data, and
result implementation. Search that page for the exact code, then use the
source link when debugging an installation or preparing a bug report.

A useful report includes:

- `trajecta --version` output;
- operating system and package architecture;
- the command with paths and credentials removed;
- the complete machine response;
- job-series ID, run ID, and attempt number when a job was accepted;
- the terminal `job status` and relevant event interval;
- result manifest status and artifact inventory.
