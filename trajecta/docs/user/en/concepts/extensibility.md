---
title: Extensibility status
description: Current built-in extension boundaries in Trajecta and the reserved design space for a future plugin system.
---

# Extensibility

Plugin loading is **not implemented** in Trajecta `0.1.0-alpha.1`. The current
package has no plugin manifest, discovery directory, dynamic loader, plugin
configuration section, or plugin command. This page reserves a place in the
product model for later design work and describes the boundaries contributors
can use in a source build today.

## Current product surface

The public product surface is versioned through:

- the `trajecta` command line;
- machine and project configuration;
- Case, RunProfile, DatasetLock, manifest, and stream schemas;
- result-directory files and the read-only SQLite schema;
- stable diagnostic codes and process exit codes.

Configuration readers reject unknown fields. A key copied from a future design
will therefore produce a schema diagnostic in the current release rather than
remaining silently unused.

## Built-in registries and interfaces

The Rust workspace keeps several implementation boundaries behind the product
surface:

| Boundary | Current role |
| --- | --- |
| Meteorological reader backend | Opens GRIB or NetCDF input and supplies normalized frames |
| Dataset profile catalog | Maps provider variables and transforms into canonical fields |
| Population strategy | Owns initialization, births, terminations, and mass accounting |
| Ozone assignment rule registry | Selects a named built-in rule and its field requirements |
| Integrator and boundary models | Advance particle state and handle physical boundaries |
| Particle-state sink | Writes SQLite output and provenance assignments |
| Job backend and local IPC | Persist queue lifecycle and communicate with the local daemon |

These are contributor-facing Rust interfaces. Their existence does not make a
third-party binary compatible with a packaged Trajecta release. New built-in
implementations are added to the workspace, reviewed with their schemas and
scientific contracts, and distributed in a source or product build.

## Extending a source build today

A contributor who needs a new meteorological profile, population model, or
output sink can work in a branch of the Rust workspace:

1. Identify the owning crate and existing trait or registry.
2. Add the implementation under a new stable model ID.
3. Define its configuration and validation in the Case or Profile model.
4. Add required meteorological capabilities and provenance identity.
5. Add deterministic unit tests and a real-fixture integration path.
6. Update result verification when new persisted state is introduced.
7. Build and distribute the resulting Trajecta binary as a distinct source
   revision.

This path changes the binary and may change the scientific contract. The
[Developer Guide](../developer/index.md) describes crate ownership, the
meteorology path, numerical runtime, tests, and release checks.

Dataset interpretation profiles are also part of the source tree in this
release. A local profile source can describe supported provider layouts through
the existing RunProfile fields, while the reader and transform operations still
come from the compiled implementation.

## Reserved future plugin areas

A future plugin design may consider a subset of these areas:

| Area | Questions the contract would need to answer |
| --- | --- |
| Data readers | File access, field capabilities, topology identity, caching, and native dependencies |
| Field transforms | Unit/type checking, operation identity, deterministic execution, and provenance parameters |
| Population models | Birth/termination lifecycle, carried state, mass accounting, and verification |
| Output products | External destination, schema ownership, streaming, partial failure, and digest participation |
| Runtime integrations | Process isolation, resource accounting, cancellation, event delivery, and recovery |

The first plugin release would also need a package identity, compatibility
range, capability declaration, version negotiation, and an installation
workflow. Scientific extensions require a clear way to identify algorithms and
their validation status in each run result.

## Reproducibility and provenance

Built-in model IDs, dataset-profile hashes, software versions, and resolved
documents are recorded in run products. A plugin system would need equivalent
identity for plugin code and configuration. At minimum, a result would need to
retain:

- plugin name, version, and content identity;
- compatible Trajecta product/schema versions;
- declared capabilities and model IDs;
- normalized configuration supplied to the plugin;
- native or external dependencies that affect output;
- source/transform lineage for fields it produces;
- failure information if it stops mid-run.

These requirements are the reason the current page contains no provisional
manifest format. Publishing one early would create compatibility expectations
before loading, isolation, and result identity have been designed together.

## Isolation and credentials

Future data-provider or runtime integrations may need credentials. A plugin
contract would have to specify how secrets are supplied without entering
project files, command transcripts, events, manifests, or provenance bundles.
It would also need explicit file and network permissions.

The current data helper follows the provider's official credential store and
keeps those values outside Trajecta documents. That convention is a useful
starting point for later design, though it is not a plugin API.

## Configuration that remains portable

Projects can prepare for future growth by using stable logical names:

- refer to meteorology through dataset IDs rather than provider paths in a
  Case;
- keep machine paths and reader selection in RunProfiles;
- place derived analysis products outside immutable run directories;
- retain resolved documents and verification records with archived results.

These practices are already part of the current product model and do not depend
on a future plugin mechanism.

## Tracking future work

When a plugin proposal is ready, it can begin with an architecture decision
covering one narrowly defined extension type. The proposal can then add schema,
CLI, packaging, and documentation only for the implemented capability. Until
that contract is released, packaged Trajecta installations use the built-in
models listed in the [reference](../reference/index.md).
