---
title: Build Trajecta configuration incrementally
description: Select, inspect, edit, and validate Trajecta machine and project configuration one value at a time.
---

# Build configuration incrementally

Trajecta separates settings for the local machine from documents that describe
a scientific run. Machine configuration controls the local daemon, the shared
CPU and memory pools, monitoring cadence, and optional execution templates.
Project configuration connects named Cases, RunProfiles, datasets, and locks.

Both layers support small, validated edits. This is useful when a project is
assembled over several sessions or reviewed through machine-readable command
output.

## Select the machine configuration file

Run `config path` before editing when several installations share a user
account:

```text
trajecta config path
```

The path is selected in the following order:

1. `--config FILE` on the current command;
2. the `TRAJECTA_CONFIG` environment variable;
3. the platform default.

| Platform | Default location |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| Ubuntu 24.04 and other supported Linux systems | `${XDG_CONFIG_HOME:-$HOME/.config}/trajecta/config.toml` |

An explicit path is convenient for an isolated benchmark or a second local
resource pool:

```text
trajecta --config configs/workstation.toml config path
trajecta --config configs/workstation.toml config validate
```

The selected configuration also determines the local daemon endpoint and job
catalog. Commands that point at different configuration files therefore see
different local queues.

## Create and inspect a configuration

`config init` creates a platform-aware starting file. It leaves an existing
file unchanged.

```text
trajecta config init
trajecta config list
```

`config list` prints leaf values in stable dotted-key order. Read a single
value with `config get`:

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get default_reader_backend
```

For a script, add the global output option before the command name:

```text
trajecta --format json config get resources.memory_pool_mib
```

## Change typed values

`config set` accepts a dotted key and a JSON value. Plain text is treated as a
string when it is not valid JSON.

```text
trajecta config set resources.cpu_slots 8
trajecta config set resources.memory_pool_mib 16384
trajecta config set resources.memory_reserve_mib 2048
trajecta config set default_reader_backend '"rust"'
trajecta config validate
```

The complete updated document is parsed and checked before it replaces the old
file. A rejected edit leaves the previous file bytes in place.

Resource values have two distinct roles:

| Key | Meaning |
| --- | --- |
| `resources.cpu_slots` | Total scheduler CPU capacity shared by admitted jobs |
| `resources.memory_pool_mib` | Memory considered by the local scheduler |
| `resources.memory_reserve_mib` | Portion kept outside the schedulable pool for the operating system and other processes |

The reserve stays below the pool. If several values need to change together,
set the pool first when increasing capacity and set the reserve first when
reducing it. Validate after the sequence.

## Add an execution template

A named profile template can be created as one structured value:

```text
trajecta config set profile_templates.workstation '{"execution":{"worker_threads":4,"memory_budget_bytes":2147483648,"executor":"local","meteorology_reader":"rust"}}'
trajecta config get profile_templates.workstation
trajecta config validate
```

Template worker threads fit within `resources.cpu_slots`, and the memory budget
is expressed in bytes. A template is removed by name:

```text
trajecta config unset profile_templates.workstation
```

Required configuration fields cannot be removed with `config unset`.

## Edit a project through selectors

Start by viewing the derived project state and the complete index:

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
```

`project get`, `project set`, and `project unset` operate on selectors from the
project index. List the current document before choosing a selector:

```text
trajecta --project PROJECT project get index
trajecta --project PROJECT project get profile.quickstart
```

Values may be JSON or YAML scalars and structures. The command resolves every
project-relative path inside the project root, validates the resulting index,
and then performs an atomic replacement.

```text
trajecta --project PROJECT project set profile.quickstart.template workstation
trajecta --project PROJECT project validate
```

Absolute paths, parent traversal such as `../data`, duplicate names, and paths
that resolve outside the project root are rejected. Keeping data roots beneath
the project directory makes a project portable between Windows and Linux.

## Work with drafts

A Profile can remain a draft while required fields are being entered. Draft
status means that recognizable content is incomplete. Unknown fields, invalid
types, and malformed Case or RunProfile documents are reported as errors.

Use this loop while assembling the project:

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
trajecta --project PROJECT project validate
```

The state progresses from `draft` to `configured` after the documents are
complete. It reaches `finalized` after the selected meteorological files and
DatasetLocks have been checked by `project finalize`.

## Check the whole local setup

Configuration validation checks the selected `.toml` file. Project validation
checks the index and the documents currently present. Doctor joins those views
with local runtime checks:

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta --project PROJECT doctor
trajecta --project PROJECT doctor --deep
```

The deep check opens configured data roots and existing locks when present. It
also creates a temporary SQLite database, exercises WAL mode, runs an integrity
check, checkpoints the WAL, and removes the temporary files. Run it after
moving a project, changing storage, or changing the machine configuration.

## Review a failed edit

For a stable diagnostic record, repeat the read-only validation in JSON mode:

```text
trajecta --format json config validate
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

The command path and diagnostic code can be stored in an issue or run log.
Correct the selected file, then rerun the same validation command before
finalizing or submitting work.
