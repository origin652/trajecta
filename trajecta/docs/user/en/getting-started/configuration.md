---
title: Configuration initialization and doctor
description: Create a Trajecta machine configuration incrementally and check it with validate and deep doctor.
---

# Configuration initialization and doctor

A machine configuration describes local resource limits and daemon behavior.
It does not contain a Case or scientific intent. Project documents remain
portable because machine-local paths are supplied through RunProfile bindings.

## Path selection

Trajecta selects the configuration in this order:

1. global `--config PATH`;
2. `TRAJECTA_CONFIG` environment variable;
3. the platform default path.

Use `config path` to display the selected path without changing it.

## Incremental setup

```text
trajecta --config local.toml config init
trajecta --config local.toml config set resources.cpu_slots 4
trajecta --config local.toml config set resources.memory_reserve_mib 512
trajecta --config local.toml config set resources.memory_pool_mib 4096
trajecta --config local.toml config validate
```

`memory_reserve_mib` must be smaller than `memory_pool_mib`. A failed update
leaves the previous configuration bytes unchanged. `config get`, `set`, and
`unset` support an iterative human or AI-assisted setup; no intermediate JSON
file is required.

## Doctor levels

```text
trajecta --config local.toml doctor
trajecta --config local.toml --project PROJECT doctor --deep
```

The normal doctor checks configuration and local runtime prerequisites. Deep
mode also performs a real SQLite create/WAL/integrity/checkpoint/cleanup cycle.
When a project is selected, it checks existing locks and data roots through
public verification APIs.

Treat a doctor error as a preflight failure. Record its diagnostic code and
correct the selected configuration or project before submitting work.
