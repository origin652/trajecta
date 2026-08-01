---
title: Product reference
description: Normative reference for Trajecta 0.1.0-alpha.1 commands, configuration, documents, schemas, diagnostics, results, and supported platforms.
---

# Product reference

This section describes the stable product surface of `0.1.0-alpha.1`:

- the `trajecta` command-line interface;
- machine configuration and project documents;
- JSON, JSONL, manifest, provenance, and result artifacts;
- diagnostic and process exit codes;
- supported package platforms and meteorological readers.

The CLI, schemas, and disk artifacts are the integration boundary for users and
automation. Rust crate APIs remain contributor-facing interfaces and may change
within the alpha series. Use [Rust API](../developer/api.md) only when developing
Trajecta itself.

Generated pages state their source at the top. Their CI check fails when binary
help, a schema, or a production diagnostic changes without regenerating the
reference.
