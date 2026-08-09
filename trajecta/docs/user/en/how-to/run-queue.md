---
title: Run Trajecta in front or through the local queue
description: Submit foreground and detached runs, understand resource admission, inspect queue state, and wait for completion.
---

# Run in front or through the queue

Every product run is admitted to the same durable local queue. Foreground mode
keeps the submitting command attached until the attempt reaches a terminal
state. Detached mode returns a receipt after the queue has accepted the work.
The numerical worker and result layout are the same in both modes.

## Prepare the submission

For project mode, confirm that the selected Profile belongs to a finalized
project:

```text
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

The run command accepts either a project/Profile pair or explicit Case and
RunProfile paths.

=== "Project mode"

    ```text
    trajecta --project PROJECT run --profile PROFILE
    ```

=== "Direct document mode"

    ```text
    trajecta run --case cases/study.yml --run-profile profiles/local.yml
    ```

Project mode is convenient for repeated work because the project index fixes
the Case, dataset mapping, locks, and output location selected by the Profile.
Direct mode provides the same queue lifecycle with an explicit document pair.

## Run in the foreground

Foreground waiting is the default:

```text
trajecta --project PROJECT run --profile PROFILE
```

After durable acceptance, the command prints the receipt and follows the job
until completion. Its terminal exit code is `0` only for `complete`. A failed,
cancelled, interrupted, or particle-error terminal state returns `1`.

Closing the terminal disconnects the foreground client. The accepted attempt
continues under daemon ownership. Use the receipt's job-series ID to reconnect:

```text
trajecta job status JOB_ID
trajecta job wait JOB_ID
```

## Submit and detach

Add `--detach` when the terminal should return immediately after admission:

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

The receipt contains three useful identity fields:

| Field | Meaning |
| --- | --- |
| `job_series_id` | Stable identity for the logical task and later reruns |
| `run_id` | Identity of this exact attempt |
| `attempt` | One-based attempt number within the series |

Store the complete receipt in automation. Human commands generally use
`job_series_id`; result commands can resolve a job-series ID, a run ID, or a
run-directory path.

!!! tip "Keep both job identities"

    Use the series ID to follow or rerun the task. Keep the run ID when a script
    must refer to one exact attempt.

## How the local daemon is selected

The machine configuration determines the daemon endpoint and its SQLite job
catalog. The first runtime command starts the local daemon automatically when
the endpoint is not reachable. On Windows it uses a local named pipe; on Linux
it uses a local Unix socket. The endpoint does not listen on a network
interface.

An idle daemon exits after `daemon.idle_shutdown_seconds`. A later command can
start it again and read the same durable catalog. Set the value to `0` when the
daemon should remain available until the operating system stops it.

All run, job, and result commands that refer to the catalog need to select the
same machine configuration:

```text
trajecta --config configs/workstation.toml job list
trajecta --config configs/workstation.toml job status JOB_ID
```

## Understand resource admission

The selected RunProfile supplies `worker_threads` and
`memory_budget_bytes`. At submission, Trajecta converts them into a scheduler
request:

- CPU slots equal the worker-thread count;
- the memory request is the byte budget rounded up to MiB;
- all active attempts share the configured CPU and schedulable memory pools.

The schedulable memory pool equals `resources.memory_pool_mib` minus
`resources.memory_reserve_mib`. A task remains `queued` until both CPU and
memory are available. Queue order is first in, first out among attempts that can
be admitted.

When currently available system memory falls below the reserve, dispatch pauses
and the queue records an external-memory-pressure warning. Running workers keep
their existing reservations.

## Read queue state

List the latest visible attempt from each routine job series:

```text
trajecta job list
```

The current command returns at most 1,000 entries. Use `job status` for a
specific series:

```text
trajecta --format json job status JOB_ID
```

Important fields include:

| Field | When it appears |
| --- | --- |
| `state` | Always |
| `queue_position` | While the attempt is queued |
| `started_at` | After worker startup begins |
| `finished_at` | In a terminal state |
| `output_directory` | After the attempt directory has been allocated |
| `resources` | Always; records the admitted request |

The normal state sequence is:

```text
queued → starting → running → complete
```

Cancellation and failures introduce other terminal paths. The
[operations manual](../operations/index.md) explains each state and the
recovery behavior after a daemon or worker interruption.

## Wait from another shell

`job wait` attaches to an existing series and exits when its current attempt
becomes terminal:

```text
trajecta job wait JOB_ID
```

It can be used after a detached submission or after reconnecting to a machine.
For continuous progress rather than a final snapshot, use
[`job events`](events.md).

## Run several Profiles

Submit each Profile separately with `--detach`:

```text
trajecta --project PROJECT run --profile forward-wet-season --detach
trajecta --project PROJECT run --profile backward-wet-season --detach
trajecta --project PROJECT run --profile sensitivity-low-mixing --detach
trajecta job list
```

The daemon admits as many workers as fit the shared pools. Remaining attempts
stay durable in `queued` state. Completed attempts are terminal catalog rows;
daemon restart does not submit or dispatch them again.

## Capture submission output in a script

Use JSON for one request and one response:

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

The envelope contains `ok`, the complete command path, the receipt in `data`,
and diagnostics when present. Keep stderr separate; machine-mode application
responses are written as one stdout envelope. Usage errors retain exit code
`2`, while runtime or product errors use exit code `1`.
