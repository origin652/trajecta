---
title: Rust API
description: Contributor-facing Rust modules, primary entry points, documentation build, and alpha compatibility policy.
---

# Rust API

Trajecta's Rust API connects the five workspace crates and supports direct
integration tests. Public items are documented and type checked across crate
boundaries. In `0.1.0-alpha.1`, they remain contributor interfaces; the stable
automation surface is the CLI, public document schemas, machine envelopes, and
result artifacts.

## Build the API documentation

Generate rustdoc for every workspace crate:

```text
cargo doc --offline --locked --no-deps --workspace
```

Open the index for the crate you are working on:

| Crate | Local rustdoc entry |
| --- | --- |
| `trajecta-case` | `target/doc/trajecta_case/index.html` |
| `trajecta-met` | `target/doc/trajecta_met/index.html` |
| `trajecta-core` | `target/doc/trajecta_core/index.html` |
| `trajecta-job` | `target/doc/trajecta_job/index.html` |
| `trajecta-cli` | `target/doc/trajecta_cli/index.html` |

On Windows, open the file through Explorer or a browser. On Ubuntu, a desktop
session can use `xdg-open target/doc/trajecta_core/index.html`.

The workspace denies missing documentation on public items and forbids unsafe
code. Rustdoc generation is therefore a source gate as well as a reading tool.

## Crate entry points

### `trajecta-case`

This crate can be used without a meteorological reader or runtime process. Its
main public areas are:

| Module | Use |
| --- | --- |
| `document` | Case, RunProfile, resolved documents, and metadata types |
| `schema` | Parse YAML/JSON and validate resolved shape |
| `expand` | Expand local component references into one resolved document |
| `intent` | Check presence requirements for a particular operation |
| `lockfile` | DatasetLock, locked files, roots, coverage, and capability identities |
| `diagnostic` | Typed paths, severity, code, and stable diagnostic ordering |
| `model` | Scientific component specifications used by Case documents |
| `quantity` | Unit registry and SI-normalized quantities |
| `reference` and `resolver` | Contained local references and source digests |

Parser entry points include `parse_case_yaml`, `parse_case_json`,
`parse_run_profile_yaml`, and `parse_run_profile_json`. Expansion functions
accept a path and resolver context so diagnostics can retain a document location.

The crate rejects unknown fields at schema boundaries. Downstream code should
consume a resolved type rather than repeat string-based YAML access.

### `trajecta-met`

`trajecta-met` turns a locked source inventory into typed query output:

| Module | Use |
| --- | --- |
| `profile` | Parse and compile Dataset Profiles and computation graphs |
| `io` | Inspect, index, and decode GRIB and NetCDF sources |
| `field` | Canonical field registry, capabilities, quality, and field keys |
| `frame` | Raw fields, computed frames, prepared windows, and frame cache |
| `grid` | Source-native grid placement and spherical vector interpolation |
| `vertical` | Pressure and hybrid geometry, columns, bounds, and stencils |
| `derive` | Named meteorological and domain-fill derivations |
| `query` | Query plans, prepared batches, output columns, execution contexts, and caches |
| `provenance` | Source records, transformations, and assignments |
| `validation` | Tolerance and comparison report types |

`MetEngine` owns preparation state. Callers obtain a `PreparedWindow`, prepare a
batch with a compiled plan, then execute it with an `ExecutionContext` and
caller-owned `BatchWorkspace`. This order is part of the no-hidden-I/O contract.

`RayonExecutionContext` is the production CPU policy. Tests can provide another
execution context without changing interpolation code.

### `trajecta-core`

The core crate owns scientific execution and result closeout:

| Module | Use |
| --- | --- |
| `clock` and `lifecycle_clock` | Signed simulation time and host lifecycle timestamps |
| `particle` | Structure-of-arrays particle batch, status, origin, and termination |
| `integrator` | `IntegratorModel`, `Rk2Spherical`, timed cohorts, and step results |
| `boundary` | Boundary path sampling, policies, decisions, and intersections |
| `population` and `release` | Release and domain-fill lifecycle models |
| `output` | Schedules, product traits, SQLite sink, and provenance bundle |
| `manifest` and `manifest_store` | Run identity, lifecycle record, and atomic persistence |
| `runner` | Production builder, `SimulationRunner`, runner control, and `RunOutcome` |
| `verification` | Quick/full result-directory verification |

`build_runner` constructs a direct run. `build_runner_for_attempt` binds output
to a durable job-series ID, run ID, and attempt number. The returned
`SimulationRunner` exposes `run` and `run_with_control`; the latter is used by
the worker for progress and cancellation.

`verify_run_directory` reads an existing product without starting a simulation.
Keep result readers separate from runner construction so inspection cannot
modify scientific state.

### `trajecta-job`

The job crate is the typed local control plane:

| Module | Use |
| --- | --- |
| `model` | Job states, identities, events, progress, resources, and submit requests |
| `backend` | Backend operations used by CLI clients and daemon implementations |
| `catalog` | SQLite-backed local queue, transitions, leases, and recovery |
| `scheduler` | Pure resource-accounted FIFO and bounded backfill plan |
| `daemon` | Dispatch cycle, worker process traits, and runner-control bridge |
| `ipc` | Bounded same-release local request/response transport |
| `history` | Attempts, full-verification records, rerun, forget, and prune plan |

`JobBackend` is the ordinary control interface. `JobHistoryBackend` adds attempt
operations. `LocalJobCatalog` implements durable behavior; `LocalJobClient`
speaks IPC to the daemon. `plan_dispatch` remains a pure function so scheduler
decisions can be tested without a process or database.

The backend traits reserve a future boundary for another scheduler integration.
They do not constitute a dynamic plugin interface in this release.

### `trajecta-cli`

The CLI crate is an adapter. `main_entry` accepts an iterator of `OsString` and
returns the process exit code, which makes complete invocations testable without
calling `std::process::exit`.

Public modules cover argument types, parsed commands, envelope rendering, and
output selection. Project, runtime, and result implementations use public
library contracts rather than exposing a second scientific API.

External Rust applications should not call CLI internals to bypass document or
job validation. For automation, execute the product binary with JSON or JSONL
output and validate the advertised schema.

## Error and diagnostic conventions

Library errors preserve enough structure for the adapter to select a public
diagnostic code. Configuration diagnostics also carry a sortable path and
severity so several document problems can be returned together.

At crate boundaries:

- return `Result` for recoverable input, I/O, resource, and lifecycle failures;
- validate a complete public type before persisting it;
- keep machine codes stable and human messages free of secrets;
- preserve the original source or path context when it helps locate a field;
- avoid converting typed meteorological sample status into a generic string
  before the numerical layer has classified it.

Production code follows workspace lints that deny `panic!`, `unwrap`, `expect`,
`todo!`, and `unimplemented!`. Tests may use local allowances for fixture setup
and assertions.

## Serialization boundaries

`serde` is used for Rust-to-document correspondence, but a derived serializer is
only one part of a public format. Public disk and stream types also have:

- a schema identifier inside the document;
- JSON Schema or a frozen SQL schema under `testdata`;
- valid examples;
- producer and consumer tests;
- deterministic ordering or canonicalization rules where identity is hashed.

Unknown-field policy is chosen at the schema boundary. Most configuration and
control-plane records use `deny_unknown_fields` to surface spelling mistakes.
Forward compatibility should be introduced through an explicit schema decision,
not by ignoring arbitrary fields in one reader.

## Trait boundaries

Several traits isolate policy or platform code:

| Trait | Boundary |
| --- | --- |
| `DataProvider` | Locate declared local dataset roots |
| `GridBackend` | Source-native horizontal placement |
| `ExecutionContext` | Worker-count policy for prepared query execution |
| `IntegratorModel` | Deterministic particle advance before boundary handling |
| `BoundaryPolicy` | Classify one ordered path against a physical boundary |
| `PopulationModel` | Population work around the shared advection step |
| `OutputProduct` and `ParticleStateSink` | Scheduled scientific output and storage |
| `RunnerControl` | Progress and cancellation at macro-step boundaries |
| `JobBackend` | Client-visible local job operations |
| `WorkerProcessManager` | Platform process launch and force-stop behavior |

These traits make focused tests possible and keep dependency direction clear.
Implementations still have to satisfy the product schemas and lifecycle rules;
implementing a public trait alone does not register a product extension.

## Compatibility during alpha

Compatibility is considered in two groups:

| Surface | `0.1.0-alpha.1` treatment |
| --- | --- |
| CLI syntax, machine envelopes, configuration, Project/Case/Profile documents, DatasetLocks, manifests, provenance, SQLite, package manifest | Product contracts; changes require schemas, examples, tests, docs, and a compatibility decision |
| Public Rust modules, traits, constructors, and error enums | Contributor contracts; may change as product implementation is refined during alpha |

A future crate intended for third-party embedding would need an explicit
stability policy, published crate packages, dependency-version guidance, and an
integration compatibility suite. Those commitments are outside the current
release.

## Adding or changing a public item

1. Place the rule in the crate that owns its meaning.
2. Add rustdoc that states inputs, output, failure cases, and lifecycle effects.
3. Prefer existing public types over a parallel string or map representation.
4. Add a focused unit test and a public-boundary integration test where useful.
5. Run `cargo doc` and clippy with warnings denied.
6. Update dependent crates in dependency order.
7. Update schemas and the user reference when the item changes a product format.
8. Run the real-data, runtime, or package matrix associated with the boundary.

Crate-level comments in each `src/lib.rs` summarize ownership. Start there before
following individual type links in rustdoc.
