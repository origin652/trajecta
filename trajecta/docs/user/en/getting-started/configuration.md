---
title: Configuration initialization and doctor
description: Create a Trajecta machine configuration, size its local resource pool, and inspect a project with doctor.
---

# Configuration initialization and doctor

Every Trajecta command selects one machine configuration. This file describes
the resources available to the local scheduler, the default meteorology reader,
monitoring cadence, and daemon idle time. Scientific settings remain in the
project's Case, while dataset paths and per-run resource requests remain in its
RunProfile.

Keeping these concerns separate makes a project portable. The same project can
use a four-core laptop Profile on one machine and a larger Profile on another
without changing its simulation period, population, or numerical settings.

## What the configuration controls

| Area | Main fields | Effect |
|---|---|---|
| Reader default | `default_reader_backend` | Reader used when a Profile leaves the choice open |
| Local daemon | `daemon.idle_shutdown_seconds` | How long an idle local daemon remains available |
| CPU pool | `resources.cpu_slots` | Total CPU slots admitted across active jobs |
| Memory pool | `resources.memory_pool_mib` and `resources.memory_reserve_mib` | Total memory considered by admission and the portion kept outside worker allocation |
| Monitoring | `monitoring.sample_interval_ms` | Interval used for runtime resource observations |
| Templates | `profile_templates.<name>` | Reusable execution values for project Profiles |

`config init` detects the host's logical CPU count and physical memory. It sets
the initial memory reserve to one quarter of physical memory and chooses the
Rust reader. These values are a starting point; `config get` and `config set`
make the final resource policy explicit.

## Select the configuration file

Trajecta resolves the path in this order:

1. the global `--config PATH` option;
2. the `TRAJECTA_CONFIG` environment variable;
3. the platform default.

The default path is `%APPDATA%\Trajecta\config.toml` on Windows. On Ubuntu it
is `$XDG_CONFIG_HOME/trajecta/config.toml`, or
`$HOME/.config/trajecta/config.toml` when `XDG_CONFIG_HOME` is empty.

Display the selected location with:

```text
trajecta config path
```

An explicit path is convenient for tutorials and isolated projects:

```text
trajecta --config quickstart.toml config path
```

For a setting shared by several shells, define the environment variable once
in the shell profile. The global option still takes precedence for an
individual command.

## Initialize and inspect it

Create a new file and display its leaf values:

```text
trajecta --config local.toml config init
trajecta --config local.toml config list
trajecta --config local.toml config get resources.cpu_slots
trajecta --config local.toml config get resources.memory_pool_mib
```

`config list` is useful when comparing two machines because it emits every
setting by its dotted selector. Machine-readable output uses the same selectors:

```text
trajecta --format json --config local.toml config list
```

If `local.toml` already exists, `config init` keeps the existing file. Continue
with `get`, `set`, or `validate` for that configuration.

## Size the resource pool

The scheduler admits a job only when its Profile request fits inside the free
CPU and memory pool. Several small jobs may run together; a larger request
waits in the queue until enough capacity is available.

### CPU slots

For a workstation where four logical cores may be used by Trajecta:

```text
trajecta --config local.toml config set resources.cpu_slots 4
```

A RunProfile's `execution.worker_threads` fits within this value. Leaving some
cores outside the pool keeps an interactive workstation responsive during a
long run. A dedicated machine can expose more of its logical cores.

### Memory pool and reserve

The scheduler's usable memory is:

```text
memory_pool_mib - memory_reserve_mib
```

For a machine where Trajecta may schedule within a 4 GiB pool while 512 MiB is
held aside:

```text
trajecta --config local.toml config set resources.memory_pool_mib 4096
trajecta --config local.toml config set resources.memory_reserve_mib 512
```

The reserve remains smaller than the pool. Each RunProfile declares
`execution.memory_budget_bytes`; the queue compares that request with currently
available memory. Memory used by unrelated applications can still change while
a job runs, so a workstation usually benefits from a reserve larger than the
minimum needed to pass validation.

## Reader and daemon settings

The default reader can be selected directly:

```text
trajecta --config local.toml config set default_reader_backend rust
```

A RunProfile may choose `rust` or `native` for a particular dataset. The
Profile value is the clearest choice for a project intended to use an explicit
reader on several machines; the machine default is convenient for exploratory
work.

The local daemon starts when a job command needs it. After the queue and active
worker set become empty, `daemon.idle_shutdown_seconds` controls how long it
waits before exiting:

```text
trajecta --config local.toml config set daemon.idle_shutdown_seconds 600
```

Queue and worker communication stays on the local machine. The current schema
keeps `daemon.local_ipc_only` set to `true`.

## Add an execution template

Named templates collect execution values that can be reused while preparing
RunProfiles. This example defines a one-thread Rust-reader template with a
1 GiB memory budget:

=== "Windows PowerShell"

    ```powershell
    $value = '{"execution":{"worker_threads":1,"memory_budget_bytes":1073741824,"executor":"cpu","meteorology_reader":"rust"}}'
    .\trajecta.exe --config local.toml config set profile_templates.one_worker $value
    ```

=== "Ubuntu 24.04"

    ```bash
    ./trajecta --config local.toml config set profile_templates.one_worker \
      '{"execution":{"worker_threads":1,"memory_budget_bytes":1073741824,"executor":"cpu","meteorology_reader":"rust"}}'
    ```

Template names are local configuration keys. Remove a template with:

```text
trajecta --config local.toml config unset profile_templates.one_worker
```

## Run doctor

Doctor reads the same configuration selected by the later run command. With an
explicit project, it also resolves and validates that project.

### Normal check

```text
trajecta --config local.toml doctor
trajecta --config local.toml --project examples/domain-fill-cfsr doctor
```

The first form checks the configuration and reports whether a project was
discovered from the current directory. The second form selects the project
directly and includes its state in the response.

### Deep check

```text
trajecta --config local.toml --project examples/domain-fill-cfsr doctor --deep
```

Deep mode creates a temporary directory under the project and exercises file
creation, synchronization, rename, and cleanup. It verifies existing
DatasetLocks against local files, opens a supported meteorological source from
each populated data root, and performs a SQLite create, write-ahead log,
integrity, checkpoint, and cleanup cycle. The temporary probe directory is
removed when the check completes.

Run deep doctor after placing data and finalizing the project. It is also useful
after moving a project to another disk or changing filesystem permissions.

## Change settings safely

Finish an editing session with validation:

```text
trajecta --config local.toml config validate
```

`config set` parses the supplied value, validates the complete document, and
then replaces the file atomically. A rejected value leaves the previous
configuration in place. Values such as reader names may be passed as plain
text; arrays and objects use JSON syntax.

For AI-assisted configuration, work through the same small sequence used by a
person: read with `get`, change one selector with `set`, run `config validate`,
and finish with `doctor --deep` against the chosen project. This keeps the file
itself as the shared configuration source.

## Common first-run issues

| Symptom | Likely point to inspect |
|---|---|
| `config.not_found` | The selected `--config` path or `TRAJECTA_CONFIG` value |
| `config.invalid_schema` | A field name, value type, reserve/pool relationship, or template request |
| Job remains queued | The Profile's CPU or memory request compared with free pool capacity |
| `doctor.filesystem_unwritable` | Write and rename permissions in the project root |
| `doctor.lock_invalid` | DatasetLock paths, file content, or a project moved without its data |
| `doctor.data_inspect_failed` | Reader choice and the first supported file under the configured data root |

The [configuration reference](../reference/configuration.md) lists every field
and constraint. Diagnostic details are indexed in the
[troubleshooting guide](../operations/troubleshooting.md).
