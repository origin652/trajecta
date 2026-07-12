# trajecta-case public API (B implementation, boundary-hardened)

**Contract version:** `0` (`trajecta_case::CONTRACT_VERSION`)
**Schema version:** `0` (`schema::CURRENT_SCHEMA_VERSION`)
**YAML crate:** `serde_yml` (maintained replacement for deprecated `serde_yaml` 0.9)

## Module map

| Module | Path | Responsibility |
|---|---|---|
| diagnostic | `src/diagnostic.rs` | Typed diagnostics |
| document | `src/document.rs` | Case / RunProfile documents |
| expand | `src/expand.rs` | Single-level component expand + RunProfile machine paths |
| intent | `src/intent.rs` | Operation-specific presence checks |
| lockfile | `src/lockfile.rs` | Dataset lock identity + streaming verify |
| model | `src/model/*` | Scientific component specs |
| quantity | `src/quantity.rs` | Units + SI quantities |
| reference | `src/reference.rs` | Safe local component refs |
| resolver | `src/resolver.rs` | Local file resolve + digests |
| schema | `src/schema.rs` | Parse + shape validation |

## Hard invariants

1. MIT case crate depends on **no** other Trajecta crate.
2. Diagnostics are data; no panics for config errors.
3. Units are never implicit; bare numbers are invalid.
4. `Timestamp` / `Unit` deserialization always go through constructors.
5. Configuration objects use `deny_unknown_fields`.
6. `ComponentRef` ref-objects may contain **only** `ref`.
7. Case component `RefPath` rejects absolute paths and `..`, and stays under `LocalRefResolver` root.
8. RunProfile machine paths (`case_path`, `lockfile`, `cache_root`) may live outside the Case root.
9. `cache_root` may not exist yet; it is still absolutized when possible.
10. Lock verification streams SHA-256 with a fixed buffer and canonicalizes payload paths.
11. SHA-256 digests are **lowercase** 64-char hex.
12. v0 expands **one level** of Case component refs only (no nested ref re-expansion).

## Expand (M1 resolved document)

| API | Role |
|---|---|
| `expand_case_file(path, resolver)` | Read Case, expand single-level component refs |
| `expand_case_document(...)` | Expand already-parsed Case |
| `expand_run_profile_file(...)` | Read profile, normalize machine paths |
| `expand_run_profile_document(...)` | Expand already-parsed profile |
| `validate_resolved_case` | Shared shape validation for expanded Cases |

Behavior:

- single-level local component resolution (YAML/JSON by extension)
- self-reference edges rejected via `ResolutionGraph`
- deterministic `sources` ordered by canonical path
- records size + SHA-256 for every source document
- missing Case/lockfile identity I/O is propagated (not swallowed)
- expanded Cases run the same semantic shape checks as inline values

## Quantity / Unit / Timestamp

- `Unit::new` rejects empty symbol, zero/non-finite scale, non-finite offset
- `Timestamp::new` rejects `nanosecond >= 1_000_000_000`
- serde cannot bypass these constructors
- `QuantityInput` object form rejects unknown fields

## Schema shape (selected rules)

- domain-fill: exactly one of mass/count; count > 0; mass finite positive
- ozone_rule / model ids / output product ids non-empty
- domain / substance / output / run-profile dataset bindings unique
- domain parent exists, not self, no cycle
- `horizontal_halo_cells >= 1`
- output interval / averaging interval finite positive

## Lock contract fields

| Field | Notes |
|---|---|
| `identity.id/source/source_url?/attribution?` | Logical provenance + optional URL |
| `profile.name/sha256` | Interpretation profile identity |
| `generator.tool/version` | Tool that produced the lock |
| `files[]` | Path-unique; roles may repeat across `valid_time` |
| `grid` / `vertical` | Topology signatures |

## Lock verify

- `sha256_file_streaming` with 64 KiB buffer (no full-file `read_to_end` for payloads)
- canonicalize lock root and each file; reject escapes (symlink/junction/`..`)
- reject duplicate paths; require deterministic path order
- same `role` with different `valid_time` is legal

## Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```
