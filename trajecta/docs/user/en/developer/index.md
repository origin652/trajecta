---
title: Developer architecture
description: Workspace boundaries, dependency direction, and contributor-facing interfaces in Trajecta.
---

# Developer architecture

The workspace is divided by ownership. Dependencies flow from document models
toward meteorology, execution, job control, and the CLI adapter.

| Crate | Owns | Must not own |
| --- | --- | --- |
| `trajecta-case` | Case, RunProfile, units, references, diagnostics, DatasetLock shapes | Meteorological decoding or execution |
| `trajecta-met` | Profiles, file indexing, native grids, derivation, interpolation, prepared queries, meteorological provenance | Particle lifecycle or output scheduling |
| `trajecta-core` | Clocks, particles, integration, boundaries, populations, outputs, manifests, verification | Provider downloads or CLI rendering |
| `trajecta-job` | Durable local queue, scheduler, attempts, IPC, daemon and worker contracts | Scientific numerics |
| `trajecta-cli` | Arguments, document loading, dispatch, machine envelopes, product result views | Duplicate scientific or scheduler logic |

The product boundary is the CLI, public document schemas, machine streams, and
disk artifacts. Public Rust modules exist so workspace crates can enforce typed
contracts. They are contributor interfaces during the alpha series.

## End-to-end ownership

1. `trajecta-case` expands and validates immutable input documents.
2. `trajecta-met` indexes locked meteorology and prepares in-memory query data.
3. `trajecta-core` executes the population and commits result artifacts.
4. `trajecta-job` owns queue state and attempt identity around the worker.
5. `trajecta-cli` exposes the lifecycle without changing its semantics.

Future extensions may use these boundaries. Dynamic plugin loading has not been
implemented; see [Extensibility](../concepts/extensibility.md).
