# Trajecta M5-A3 B Delivery Report — Result Products

**Status: submitted to A for review.** This document records B's implementation and
local verification evidence. It does **not** claim M5-A3, M5, or release completion.
No commit or push was made.

## Scope and boundaries

Implemented only the M5-A3 result-product surface in `trajecta-cli` plus frozen
Draft 2020-12 schemas/examples and the M5 validator registrations:

- `result inspect RESULT`
- `result trajectory RESULT`
- `run report --result RESULT`

No numerical core, meteorology, Case model, job scheduler semantics, data-lock
production, project-finalize logic, M4 contracts, version, or
`../origo-validation-v1.json` was modified.

## Products

### `result inspect`

`result inspect` resolves and parses the public `run-manifest.json` before opening
optional artifacts.  It returns identity, lifecycle, software/input/execution/
numerical summaries, particle/termination/quality/mass-ledger summaries, catalog
placeholder, and deterministic artifacts (`manifest`, SQLite/WAL, provenance,
run-report when present, and sorted forensic entries).

- Terminal success states require `particles.sqlite` and provenance identity/file.
- Failed/interrupted-style manifests can remain inspectable without SQLite; when
  SQLite exists but is unreadable the command succeeds with exactly one
  `result.inspect_sqlite_unavailable` warning diagnostic and SQL-derived fields
  are `null`.
- It validates inspected SQLite table counts against non-empty manifest row counts;
  disagreement is `result.manifest_sqlite_count_mismatch`.
- Malformed/unknown manifest fields are `result.manifest_invalid`; artifacts outside
  the resolved run directory are rejected.

### `result trajectory`

Trajectory selection parses `--particle-id` plus optional UTC `--start`/`--end` and
`--max-records` before emitting stdout.  Requested IDs are verified first; any
missing ID returns `result.trajectory_missing_particle` with no stdout.

- `--format json` streams SQLite rows to a create-new temporary spool first, then
  emits one `trajecta.cli-output/v1` envelope with the immutable identity and one
  sorted record array; it never materializes all records in a `Vec`.
- `--format jsonl` emits a header, one `data` item per state, then exactly one
  summary item.
- Human output is deterministic text rows rather than pretty-printed JSON.
- SQLite access errors map to stable `result.trajectory_*` diagnostics.

### `run report`

Reports can only be written to the fixed in-run path `run-report.md`; arbitrary
`--output` is rejected as a structured usage error.  The renderer is deterministic
UTF-8/LF Markdown with a final LF and the fixed sections:

1. Run identity
2. Lifecycle
3. Inputs
4. Execution
5. Numerical configuration
6. Particle outcomes
7. Quality
8. Mass ledger
9. Artifacts
10. Forensics

The writer uses a same-directory create-new temporary file, `write_all`, `flush`,
`sync_all`, and rename; write failure removes the temporary path and leaves existing
`run-report.md` bytes unchanged.  The report renderer excludes the derived report
artifact from its own artifact table and reports are explicitly not scientific
content, SQLite, or canonical-output digest inputs.

## Schemas, examples, and validation

Added and registered Draft 2020-12 assets:

```text
testdata/M5_RESULT_INSPECTION.schema.json
testdata/M5_RESULT_INSPECTION.example.json
testdata/M5_TRAJECTORY_RECORD.schema.json
testdata/M5_TRAJECTORY_RECORD.example.json
testdata/M5_TRAJECTORY_STREAM.schema.json
testdata/M5_TRAJECTORY_STREAM.example.json
```

`tools/validate_m5_a0_contracts.py` validates each new schema/example pair with
`Draft202012Validator`.

## Test evidence

The A3 tests use the existing M5-A2 production daemon/worker CFSR contract to
produce a real M5 run directory with `run-manifest.json` and `particles.sqlite`;
they do not hand-write a substitute SQLite database or reuse a legacy M4 artifact.
They cover real `result inspect`, JSON/JSONL/human trajectory product paths,
missing-particle zero-stdout behavior, fixed report location, final LF, report
idempotence, and corrupt-SQLite partial inspection diagnostics.

Additional unit coverage locks strict public manifest parsing, formal successful
artifact requirements, schema shape, result-root containment, deterministic artifact
sorting, terminal ordering, and report atomic-write behavior.

## Final local gates

All commands below passed after the final implementation change:

```text
cargo test --offline -p trajecta-cli
55 passed

cargo test --offline -p trajecta-job
37 passed

cargo test --offline -p trajecta-core verification::tests
9 passed, 179 filtered out

cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts
2 passed (real CFSR production runtime contract)

cargo test --offline --workspace
537 passed, 11 ignored

cargo clippy --offline --workspace --all-targets -- -D warnings
passed

cargo doc --offline --no-deps
passed

cargo fmt --all -- --check
passed

python tools/validate_m5_a0_contracts.py
passed

python tools/validate_m4_a0_contracts.py
passed

git diff --check
passed
```

## Review request

A should review the public result shape, JSON/JSONL stream contracts, the real
M5 runtime artifact test path, and the fixed-name atomic report contract.  No M5-A3
or M5 completion is asserted by B.
