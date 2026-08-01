---
title: Machine configuration
description: Selection precedence, fields, constraints, templates, and mutation rules for the Trajecta machine configuration.
---

# Machine configuration

Machine configuration describes resources and local runtime policy. It does not
contain Case science, project identity, data locks, or provider credentials.

## File selection

Trajecta selects exactly one configuration using this precedence:

1. `--config PATH`;
2. `TRAJECTA_CONFIG`;
3. `%APPDATA%/Trajecta/config.toml` on Windows, or
   `$XDG_CONFIG_HOME/trajecta/config.toml` on Linux;
4. the documented home-directory fallback when the platform variable is absent.

Use `trajecta config path` to show the selected path. Commands never merge two
configuration files.

## Fields

| Selector | Type and constraint | Meaning |
| --- | --- | --- |
| `schema_version` | `trajecta.config/v1` | Disk contract identity |
| `default_reader_backend` | `rust` or `native` | Reader used when a profile does not override it |
| `daemon.idle_shutdown_seconds` | integer, at least 0 | Idle lifetime of the local daemon |
| `daemon.local_ipc_only` | fixed `true` | Prevents a network control endpoint |
| `resources.cpu_slots` | integer, at least 1 | Shared scheduler CPU capacity |
| `resources.memory_pool_mib` | integer, at least 1 | Shared scheduler memory pool |
| `resources.memory_reserve_mib` | integer, at least 0 and smaller than the pool | Memory withheld from admission |
| `monitoring.sample_interval_ms` | integer, at least 100 | Resource observation interval |
| `monitoring.maximum_median_overhead_percent` | number from 0 through 1 | Monitoring overhead limit |
| `profile_templates.<name>.execution` | object | Named execution defaults |

Each template supplies positive `worker_threads` and `memory_budget_bytes`, a
non-empty executor name, and a `rust` or `native` meteorology reader. A template
cannot request more worker threads than `resources.cpu_slots`.

## Safe mutation

```text
trajecta config get resources.memory_pool_mib
trajecta config set resources.memory_reserve_mib 1024
trajecta config set resources.memory_pool_mib 8192
trajecta config unset profile_templates.experimental
trajecta config validate
```

`set` parses the value in the context of its field. The whole document is
validated before an atomic replacement. A rejected change leaves the original
file bytes intact. `config init` refuses to overwrite an existing file.

The normative field shape is generated in [JSON and JSONL schemas](schemas.md).
