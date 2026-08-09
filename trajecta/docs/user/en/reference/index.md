---
title: Trajecta product reference
description: Look up Trajecta 0.1.0-alpha.1 commands, configuration fields, project documents, schemas, diagnostics, results, and supported platforms.
---

# Product reference

The reference describes the public product surface of Trajecta
`0.1.0-alpha.1`. Use it when the question is about an exact command synopsis,
field name, schema, process status, diagnostic code, result table, or supported
platform.

For a complete workflow, begin with [Getting Started](../getting-started/index.md)
or choose a task from the [How-to Guides](../how-to/index.md). The pages here
favor lookup tables and exact contracts over step-by-step tutorials.

## Public interfaces

| Interface | Format | Reference |
| --- | --- | --- |
| Command-line interface | Human text, JSON envelope, or JSONL stream | [CLI command tree](cli.md) |
| Machine configuration | TOML | [Configuration fields](configuration.md) |
| Project index | YAML | [Project document model](documents.md) |
| Case and RunProfile | YAML or JSON document contracts | [Case and Profile fields](documents.md) |
| DatasetLock and data plan | JSON | [Project documents](documents.md) and [schemas](schemas.md) |
| Job and result machine output | Versioned JSON or JSONL | [Schemas](schemas.md) |
| Run product | JSON manifest, SQLite, provenance bundle, and optional report | [Results and SQLite](results-sqlite.md) |
| Diagnostics | Namespaced code plus contextual fields | [Diagnostic index](diagnostics.md) |
| Shell status | Process exit code | [Exit codes](exit-codes.md) |
| Distribution | Windows or Ubuntu release archive | [Platform matrix](platforms.md) |

## Stability model

Software releases and data formats have separate identities. The executable
uses semantic prerelease version `0.1.0-alpha.1`. Machine documents and streams
carry values such as `trajecta.config/v1` or
`trajecta.cli-stream-item/v1`. A schema version changes when that format's
contract changes; an ordinary documentation or implementation update does not
create another schema identifier.

The stable integration surface for users and automation is formed by:

- command paths and documented option meanings;
- machine output envelopes and stream records;
- configuration, Project, Case, RunProfile, and lock documents;
- run manifest, SQLite schema, provenance bundle, and result directory layout;
- diagnostic and process exit codes.

Rust crate APIs support contributors working inside the workspace. They may be
reorganized during the alpha series without changing the public CLI or disk
contract. Crate documentation is linked from the
[Rust API page](../developer/api.md).

## Generated reference

Three references are regenerated from the implementation:

| Page | Generation source |
| --- | --- |
| CLI command tree | Current binary `--help` and `M5_CLI_CONTRACT.v1.json` |
| JSON and JSONL schemas | Public schema and example files under `testdata/` |
| Diagnostic codes | Namespaced diagnostic strings in production CLI and job sources |

The generator checks these pages during documentation validation. A command,
schema, or diagnostic change therefore arrives with the matching reference
update.

## Alpha scope

Current package platforms, local-daemon behavior, dry-run pruning, result
interfaces, and reserved future extension points are summarized in
[Alpha limits](alpha.md). The software version remains
`0.1.0-alpha.1` throughout M5.1.
