---
title: Everyday command roadmap
description: Find the Trajecta commands most often used for machine setup, project finalization, queue operation, and result reading.
---

# Everyday command roadmap

This roadmap follows the order of a typical study. Work from top to bottom for
a first run, or jump directly to the stage needed by an established project.
`PROJECT` is a project directory, `PROFILE` is a RunProfile name registered in
the project index, `JOB_ID` comes from run admission, and `RESULT` may be a job
series ID, run ID, or result directory.

## Find a command by task

| Task | Commands used most often | Continue with |
| --- | --- | --- |
| Create and check machine configuration | `config init`, `config validate`, `doctor` | [Configuration and doctor](../getting-started/configuration.md) |
| Check whether a project can proceed | `project status`, `project show`, `project validate` | [Incremental configuration](../how-to/progressive-configuration.md) |
| Plan and inspect meteorological data | `project data-plan`, `data inspect`, `project finalize` | [Configure before data arrives](../how-to/deferred-data.md) |
| Submit foreground or background work | `run`, `job events`, `job wait` | [Foreground runs and the queue](../how-to/run-queue.md) |
| Read or change queued work | `job list`, `job status`, `job cancel`, `job rerun` | [Cancel and rerun](../how-to/cancel-rerun-prune.md) |
| Inspect completed results | `result inspect`, `result verify`, `result trajectory`, `run report` | [Read results](../how-to/results.md) |

## Prepare the machine

Create the machine configuration once, then check its fields and the runtime
environment:

```text
trajecta config init
trajecta config validate
trajecta doctor
```

The machine configuration stores the job catalog location, data roots, and
resource-pool settings. `config validate` checks the document. `doctor` also
checks the selected configuration and referenced paths. Add the deep checks
after the project and DatasetLock are ready:

```text
trajecta --project PROJECT doctor --deep
```

!!! tip "Keep machine configurations separate"

    Add `--config PATH` when one computer has distinct test and production
    environments. The selected job catalog, data roots, and resource pool move
    together.

## Prepare the project and meteorological data

An existing project usually begins with a status check. Project documents can
be validated and a data plan can be written before the meteorological files
have been downloaded:

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project validate
trajecta --project PROJECT project data-plan --output data-plan.json
```

The data plan lists the dataset family, time coverage, local target root, and
DatasetLock path. The download helper previews its request by default. Add
`--execute` after reviewing the target and interval:

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json --execute
```

After the files arrive, inspect an individual file when useful and finalize the
project:

```text
trajecta data inspect FILE
trajecta --project PROJECT project finalize
trajecta --project PROJECT doctor --deep
```

`project finalize` checks local data against Case coverage and the RunProfile,
then binds the matching DatasetLock. Regenerate the data plan and finalize
again after changing the scientific Case.

## Choose foreground or background execution

A foreground run waits in the current terminal until the attempt finishes. It
fits tutorials and short jobs:

```text
trajecta --project PROJECT run --profile PROFILE
```

A detached run returns after durable queue admission. Use it for long jobs and
multi-job queues:

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

The JSON response contains the `job_id` used by later queue commands. Closing
the submitting terminal does not remove an admitted job.

!!! tip "Keep the job ID"

    Save the JSON admission response with the study log. Status, cancellation,
    and reruns all begin from the same job-series ID.

## Follow and manage jobs

These commands cover routine queue work:

```text
trajecta job list
trajecta job status JOB_ID
trajecta --format jsonl job events JOB_ID --since 0 --follow
trajecta job wait JOB_ID
```

`job status` returns a snapshot, `job events` reads state changes by durable
sequence, and `job wait` waits for the current attempt to become terminal. To
stop or repeat work, use:

```text
trajecta job cancel JOB_ID
trajecta job cancel JOB_ID --force
trajecta job rerun JOB_ID
```

Regular cancellation gives the worker time to close out. Force cancellation is
available when a worker no longer responds; the attempt directory remains for
inspection. `job rerun` creates a new attempt in the same series and preserves
completed attempts.

## Read and verify results

After a job reaches a successful terminal state, read its summary and run the
full verification:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
```

Read a trajectory by particle ID or create the result report:

```text
trajecta --format jsonl result trajectory RESULT --particle-id ID
trajecta run report --result RESULT
```

`result trajectory` can stream one or several particle time series. Use `--all`
for the complete population; JSONL is convenient when a large stream will be
processed one record at a time.

## Find the remaining commands

The [complete command index](command-index.md) lists every command in the
current release under six task groups. Open the [CLI command tree](../reference/cli.md)
for global options, exit behavior, machine-output formats, and exact binary
help.
