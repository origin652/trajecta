---
title: Machine configuration reference
description: Look up Trajecta machine-configuration path selection, defaults, resource fields, monitoring values, Profile templates, and update rules.
---

# Machine configuration

The machine configuration describes one local Trajecta runtime. It selects the
reader default, scheduler capacity, monitoring cadence, daemon idle behavior,
and reusable execution templates. Scientific time, domain, population, and
output choices stay in the project's Case. Dataset paths and the per-run
resource request stay in a RunProfile.

The file format is TOML with schema identity:

```toml
schema_version = "trajecta.config/v1"
```

Unknown fields and duplicate TOML keys are rejected.

## Path selection

Trajecta selects one file for each invocation in this order:

1. global `--config PATH`;
2. `TRAJECTA_CONFIG` environment variable;
3. the platform default.

| Platform | Default path |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| Ubuntu 24.04 with `XDG_CONFIG_HOME` | `$XDG_CONFIG_HOME/trajecta/config.toml` |
| Ubuntu 24.04 without `XDG_CONFIG_HOME` | `$HOME/.config/trajecta/config.toml` |

Display the resolved location:

```text
trajecta config path
trajecta --config workstation.toml config path
```

Files are not merged. A `run` submitted with one configuration belongs to that
configuration's daemon endpoint and job catalog. Later `job` and catalog-backed
`result` commands should select the same file.

## Initial values

`config init` creates a new file and leaves an existing file untouched:

```text
trajecta --config workstation.toml config init
```

The initial values are calculated as follows:

| Field | Initial value |
| --- | --- |
| `default_reader_backend` | `rust` |
| `daemon.idle_shutdown_seconds` | `300` |
| `daemon.local_ipc_only` | `true` |
| `resources.cpu_slots` | Logical parallelism reported by the host, with a minimum of 1 |
| `resources.memory_pool_mib` | Detected physical memory in MiB; falls back to 4,096 MiB when detection is unavailable |
| `resources.memory_reserve_mib` | One quarter of the pool, kept below the pool |
| `monitoring.sample_interval_ms` | `1000` |
| `monitoring.maximum_median_overhead_percent` | `1.0` percent |
| `profile_templates` | Empty mapping |

These values describe detected capacity. A workstation can expose fewer CPU
slots or increase its memory reserve before a large batch is submitted.

## Top-level fields

| Selector | Type | Constraint | Runtime meaning |
| --- | --- | --- | --- |
| `schema_version` | string | Exactly `trajecta.config/v1` | Selects the configuration contract |
| `default_reader_backend` | enum | `rust` or `native` | Reader used when a binding and Profile leave the choice open |
| `daemon` | table | Required | Local daemon lifecycle settings |
| `resources` | table | Required | Shared scheduler capacity |
| `monitoring` | table | Required | Resource-observation policy |
| `profile_templates` | table | Required; may be empty | Named execution templates used during Profile preparation |

## Daemon fields

| Selector | Type and range | Meaning |
| --- | --- | --- |
| `daemon.idle_shutdown_seconds` | Integer, `0` or greater | Seconds an empty local daemon waits before exiting; `0` keeps it available |
| `daemon.local_ipc_only` | Fixed `true` | Uses a named pipe on Windows or Unix socket on Linux; no network listener is configured |

Idle exit closes the process and endpoint while keeping the SQLite job catalog,
events, and result directories. A later runtime command starts a daemon and
reconciles the same catalog.

## Resource fields

| Selector | Type and range | Meaning |
| --- | --- | --- |
| `resources.cpu_slots` | Integer, at least `1` | Total worker-thread slots shared by active attempts |
| `resources.memory_pool_mib` | Integer, at least `1` | Memory pool considered by scheduler admission |
| `resources.memory_reserve_mib` | Integer, at least `0`, smaller than the pool | Portion kept outside worker admission and used as the external-pressure threshold |

Schedulable memory is:

```text
memory_pool_mib - memory_reserve_mib
```

One RunProfile requests `execution.worker_threads` CPU slots and
`execution.memory_budget_bytes` rounded upward to MiB. Several attempts may run
together while the sum fits both pools. Host memory used by unrelated programs
can pause new dispatch when available memory falls below the reserve.

## Monitoring fields

| Selector | Type and range | Meaning |
| --- | --- | --- |
| `monitoring.sample_interval_ms` | Integer, at least `100` | Nominal interval for runtime progress and resource observations |
| `monitoring.maximum_median_overhead_percent` | Finite number from `0.0` through `1.0` | Median overhead ceiling recorded by the monitoring contract |

Resource and progress observations become durable job events. They support
status displays and run reports; trajectory samples remain in the result
database.

## Profile templates

Each non-empty template name contains one `execution` table:

```toml
[profile_templates.local.execution]
worker_threads = 4
memory_budget_bytes = 2147483648
executor = "rayon"
meteorology_reader = "rust"
```

| Field | Constraint |
| --- | --- |
| `worker_threads` | Positive and no larger than `resources.cpu_slots` |
| `memory_budget_bytes` | Positive integer byte count |
| `executor` | Non-empty stable executor name |
| `meteorology_reader` | `rust` or `native` |

Templates are local conveniences. A project index can record a template name
and its SHA-256 so a prepared Profile can be related to the exact local
template. Runtime execution still receives a fully resolved RunProfile.

Create or remove a template with dotted selectors:

=== "Windows PowerShell"

    ```powershell
    $value = '{"execution":{"worker_threads":2,"memory_budget_bytes":1073741824,"executor":"rayon","meteorology_reader":"rust"}}'
    .\trajecta.exe --config workstation.toml config set profile_templates.two_workers $value
    .\trajecta.exe --config workstation.toml config unset profile_templates.two_workers
    ```

=== "Ubuntu 24.04"

    ```bash
    ./trajecta --config workstation.toml config set profile_templates.two_workers \
      '{"execution":{"worker_threads":2,"memory_budget_bytes":1073741824,"executor":"rayon","meteorology_reader":"rust"}}'
    ./trajecta --config workstation.toml config unset profile_templates.two_workers
    ```

## Read and update values

```text
trajecta --config workstation.toml config list
trajecta --config workstation.toml config get resources.memory_pool_mib
trajecta --config workstation.toml config set resources.memory_reserve_mib 2048
trajecta --config workstation.toml config validate
```

`config list` returns every leaf selector in deterministic order. `config get`
requires an existing selector. `config set` accepts plain strings for scalar
text and JSON syntax for an object or array value. `config unset` is limited to
optional template selectors; required fields remain present.

Every update follows the same sequence:

1. read and parse the current file;
2. replace or remove the selected value in memory;
3. deserialize and validate the complete configuration;
4. write a same-directory temporary file;
5. flush and synchronize it;
6. atomically replace the selected configuration.

A rejected value leaves the previous file bytes in place. Finish a group of
changes with `config validate`, then run project `doctor --deep` before a long
queue.

The machine-readable shape and example are linked from the
[schema reference](schemas.md).
