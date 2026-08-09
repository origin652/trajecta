---
title: Trajecta how-to guides
description: Task-oriented procedures for configuration, deferred data, finalization, queues, events, cancellation, reruns, pruning, results, and AI assistance.
---

# How-to guides

The how-to guides begin with a concrete task and assume that the reader already
knows the broad shape of a Trajecta project. For a first run, use the
[fifteen-minute quickstart](../getting-started/quickstart.md). For a complete
worked example, choose one of the [tutorials](../tutorials/index.md).

Commands on these pages use three placeholders:

| Placeholder | Meaning | Example |
| --- | --- | --- |
| `PROJECT` | Project directory or `trajecta-project.yml` path | `examples/domain-fill-cfsr` |
| `PROFILE` | A profile name from the project index | `quickstart` |
| `RESULT` | Job-series ID, run ID, or run directory | `runs/0198.../attempt-1` |

Paths in project documents remain relative to the project root. Shell examples
use forward slashes because PowerShell and Unix-style shells both accept them in the
locations shown.

## Choose a procedure

| Task | Guide | What it covers |
| --- | --- | --- |
| Change one machine or project value | [Build configuration incrementally](progressive-configuration.md) | Configuration selection, typed values, templates, project selectors, and validation |
| Prepare a project while meteorological data is still absent | [Configure before data arrives](deferred-data.md) | Project states, data plans, provider requests, local inspection, and finalization |
| Choose foreground or detached execution | [Run in front or through the queue](run-queue.md) | Submission, resource admission, job status, waiting, and daemon ownership |
| Feed status to a terminal, script, or monitoring process | [Follow job events](events.md) | Global cursors, per-job filters, JSONL framing, reconnects, and polling |
| Stop work or create another attempt | [Cancel, rerun, forget, and prune](cancel-rerun-prune.md) | Safe and forced cancellation, attempt history, list visibility, and dry-run cleanup plans |
| Examine a completed or partial run | [Inspect and read results](results.md) | Inspection, verification, trajectories, reports, and advanced SQLite access |
| Ask an AI assistant to help prepare configuration | [AI-assisted configuration](ai-companion.md) | Shared source files, bounded prompts, command review, and local validation |

## A useful order for a new project

Most projects move through the following sequence:

1. Create or select the machine configuration.
2. Initialize the project and add its Case and RunProfile documents.
3. Generate a data plan while the project is in `configured` state.
4. Acquire the requested files and inspect representative inputs.
5. Finalize the project, which creates immutable dataset locks.
6. Submit a foreground or detached run.
7. Follow durable events and inspect the terminal result.

Each command can also emit a machine-readable envelope through `--format json`.
Event and trajectory streams additionally support `--format jsonl`. The
[schema reference](../reference/schemas.md) describes those records, and the
[diagnostic index](../reference/diagnostics.md) maps stable codes to recovery
steps.

## Before changing a running setup

`config set`, `config unset`, `project set`, and `project unset` validate the
new document before replacing the previous file. Keep the command output when
an edit fails; it identifies the selector and document that need attention.

A finalized project binds resolved configuration to a dataset inventory.
Changing a Case, Profile, data mapping, or meteorological file calls for a new
`project finalize` before the next submission. Existing attempts retain their
own resolved documents and provenance inside their result directories.
