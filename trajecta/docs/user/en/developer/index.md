---
title: Developer architecture
description: Workspace structure, dependency direction, change routing, and contributor interfaces in Trajecta.
---

# Developer architecture

Trajecta is a Rust workspace with five product crates. The split follows the
life of a simulation: documents are resolved, meteorology is prepared, particles
are advanced, work is scheduled, and commands present the result. Keeping that
order visible makes a change easier to place and easier to review.

This guide is for contributors working on the Rust implementation, test
infrastructure, packaging tools, or documentation. The supported user surface is
covered in [Reference](../reference/index.md).

## Repository map

| Path | Responsibility | Typical changes |
| --- | --- | --- |
| `crates/trajecta-case` | Case and RunProfile documents, units, local references, diagnostics, DatasetLock types | A new validated field, reference rule, or document diagnostic |
| `crates/trajecta-met` | Dataset Profiles, source readers, native grids, vertical coordinates, field derivation, interpolation, query preparation | A reader, derived field, vertical-coordinate rule, or query optimization |
| `crates/trajecta-core` | Simulation clocks, particle state, RK2 integration, boundaries, populations, output products, manifests, result verification | A numerical rule, lifecycle rule, output row, or scientific verification check |
| `crates/trajecta-job` | Durable local catalog, scheduler, attempts, event history, IPC, daemon and worker control | A queue transition, recovery rule, resource decision, or job event |
| `crates/trajecta-cli` | Command parsing, project assembly, dispatch, machine envelopes, result views | A command, renderer, project operation, or exit-code mapping |
| `testdata` | Public schemas, frozen contracts, small fixtures, and validation inputs | A schema-compatible example or versioned contract fixture |
| `examples` | Complete user projects used by tutorials and automated quickstarts | A documented workflow or sample configuration |
| `tools` | Packaging, reference generation, data retrieval, and validation programs | A release gate or repeatable maintenance operation |
| `docs/user` | Published bilingual manual | User, operator, and contributor documentation |
| `docs/engineering` | Plans, prompts, execution reports, and historical design material | Internal project record; excluded from the public site |

The workspace dependency direction is:

```text
trajecta-case
    └── trajecta-met
            └── trajecta-core
                    └── trajecta-job

trajecta-cli depends on all four product libraries.
trajecta-core also uses document types directly from trajecta-case.
trajecta-job uses validated input types from trajecta-case.
```

Lower layers do not call the CLI. Scientific execution does not depend on the
daemon. This lets the same `trajecta-core` runner serve direct tests and a
daemon-launched worker without maintaining two simulation paths.

## From command to result

A project run crosses the workspace in a fixed sequence:

1. `trajecta-cli` discovers `trajecta-project.yaml`, selects a named profile,
   and resolves project-relative paths inside the project root.
2. `trajecta-case` parses the Case and RunProfile with unknown-field rejection,
   expands component references, normalizes quantities, and returns sorted
   diagnostics.
3. Project finalization binds the resolved documents to DatasetLocks. A lock
   records the files, hashes, coverage, capabilities, and dataset identity used
   by the run.
4. `trajecta-job` admits the request to the local catalog. The scheduler reserves
   CPU slots and memory, assigns an attempt identity, then launches the worker.
5. The worker asks `trajecta-met` to index the locked sources, build canonical
   fields, select the required frame window, and prepare query structures.
6. `trajecta-core` creates the population and advances it through signed macro
   steps. Boundary handling, births, terminations, and scheduled output all run
   in stable lifecycle order.
7. The output transaction closes SQLite, writes provenance, and replaces the
   running manifest with its terminal form. The job catalog records the matching
   terminal state and emits a final event.
8. Result commands read the public manifest and SQLite products. They do not
   reopen the numerical runner.

The [meteorology and numerical path](science-path.md) follows steps 3–6 in
detail. [Output and runtime](runtime.md) covers queue admission through terminal
recovery.

## Where a change belongs

| Change | Start here | Follow-up review |
| --- | --- | --- |
| Add a Case field | `trajecta-case` model and schema | Expansion, examples, generated schema reference |
| Add a dataset family | `trajecta-met` Profile, inventory, and reader | Lock capabilities, real fixture, package matrix |
| Change interpolation | `trajecta-met/query` or `trajecta-met/grid` | Scientific tolerance, determinism, performance |
| Change particle motion | `trajecta-core/integrator` | Forward/backward replay, boundaries, output identity |
| Change a population | `trajecta-core/population` with meteorological derivation in `trajecta-met` | Mass ledger, births, stable IDs, tutorial |
| Add a result field | `trajecta-core/output` and manifest | SQLite schema, verification, result inspect, reference |
| Change queue behavior | `trajecta-job` | Catalog transaction, IPC, CLI event stream, recovery tests |
| Add a command | `trajecta-cli` | CLI contract, JSON/JSONL envelopes, exit codes, generated reference |
| Change package contents | `tools/m5_a4_package.py` | Build manifest, SBOM, clean extraction, both platforms |
| Change published docs | `docs/user/en` and `docs/user/zh-CN` | Strict build, link checks, translation structure, dev/release SEO |

A change can span several rows. The table identifies the layer that owns the
new rule; adapters in later layers should call that rule rather than reproduce
it.

## Contracts that cross crates

Several objects carry meaning between layers and deserve extra care during
review:

| Contract | Producer | Main consumers | Compatibility concern |
| --- | --- | --- | --- |
| Resolved Case and RunProfile | `trajecta-case` | CLI, core runner | Field meaning, defaults, normalized units |
| DatasetLock | project finalization | met loader, manifest builder | File identity, time and space coverage, capabilities |
| Prepared query and typed sample status | `trajecta-met` | integrator, boundary sampler, outputs | No hidden execute-phase file I/O; explicit invalidity |
| Run manifest | `trajecta-core` | job recovery, verify, result commands | Lifecycle state, input identity, row counts, provenance |
| Job record and event | `trajecta-job` | CLI status, wait, events, history | Durable ordering, attempt identity, terminal semantics |
| CLI envelope | `trajecta-cli` | shell scripts, AI tools, CI | Stable command path, diagnostic shape, stdout discipline |

Public schemas under `testdata` describe disk and machine formats. Rust types are
compiled counterparts used within the workspace. During the alpha series, a
public Rust item may still change when the product contract changes; see
[Rust API](api.md).

## Contributor workflow

For most changes, the shortest reliable loop is:

1. Read the crate-level contract at the top of its `src/lib.rs` and the module
   comment nearest the change.
2. Run the narrow unit or integration test while editing.
3. Check every public format affected by the change: schema, example, diagnostic,
   CLI envelope, manifest, or SQLite row.
4. Run the workspace formatting, lint, test, and documentation gates described
   in [Tests and fixtures](testing.md).
5. Run a real-data or packaged-product gate when the claim crosses a reader,
   numerical, runtime, or native-library boundary.
6. Update both language trees when user-visible behavior changes.

Dynamic plugin loading is not part of `0.1.0-alpha.1`. The existing crate and
trait boundaries leave room for later design work without presenting an
extension API today. The current status is summarized in
[Extensibility](../concepts/extensibility.md).
