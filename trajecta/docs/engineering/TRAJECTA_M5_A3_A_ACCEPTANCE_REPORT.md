# Trajecta M5-A3 A Acceptance Report

**Status: M5-A3 exit conditions passed in the uncommitted working tree.**

This report is A's review and closeout record for M5-A3. It does not claim M5,
packaging, prerelease, or release completion. No commit, push, tag, version change,
artifact deletion, or M5-A4 work was performed in this closeout.

## 1. Source and scope

- Base committed source: `fc4d737` (`docs(m5): close A2E WSL verification`).
- B delivery reviewed:
  `docs/engineering/TRAJECTA_M5_A3_B_DELIVERY_REPORT.md`, received SHA-256
  `fdda25034723c79a22484887632222b1df5b58a07d758b4b6015edb78613f4f7`.
- A retained the existing M5-A3 verifier, attempt-history, rerun, forget,
  supersession, and dry-run pruning implementation.
- A reviewed and corrected B's result-product implementation, then connected the
  derived report to the A-owned terminal lifecycle.
- No numerical core, meteorology, Case, tolerance, particle classification, M4
  contract, crate version, or outer `../origo-validation-v1.json` change was made.
- No subagent was used. B work was coordinated only through the frozen prompt file.

## 2. Accepted M5-A3 behavior

### Job history and control

- `job rerun` creates a new run ID and incremented attempt in the same series.
- Active attempts must reach a safe terminal state before rerun.
- `job forget` changes routine-list visibility only; it does not delete catalog rows
  or result files.
- `job prune` remains permanently `mode=dry_run` with `delete_enabled=false`.
- A fully verified later Complete attempt records supersession of older attempts.

### Result verification and products

- `result verify` supports run directory, manifest path, series ID, and exact run ID.
- Quick/full verification keeps process success separate from scientific
  `run_success`; Failed/Interrupted results cannot masquerade as formal products.
- `result inspect` returns the strict `trajecta.result-inspection/v1` shape, validates
  catalog identity when selected by ID, and exposes routine visibility, full
  verification, and supersession metadata.
- Inspection no longer computes a fresh full-file SQLite SHA. It performs read-only
  integrity, schema, row-count, quality, and termination summaries.
- A readable manifest with an unreadable SQLite artifact remains a partial inspection
  with exactly one `result.inspect_sqlite_unavailable` warning.
- `result trajectory` supports the frozen explicit selection syntax only:
  repeated `--particle-id ID` or `--all`. The B report's reference to
  `--start`, `--end`, and `--max-records` was inaccurate and was not turned into
  unplanned feature expansion.
- JSON trajectory output uses one standard `trajecta.cli-output/v1` envelope and an
  OS-temporary create-new spool; JSONL uses contiguous standard stream items and one
  success summary. Mid-stream failures do not append a second, restarted envelope.
- Missing requested particles fail before the trajectory stream begins with
  `result.particle_not_found` and sorted IDs.

### Derived `run-report.md`

- The fixed report is deterministic UTF-8/LF Markdown and explicitly excluded from
  scientific content, SQLite, provenance, and canonical-output digest identity.
- Report writes use a unique same-directory create-new temporary file, flush,
  `sync_all`, and atomic rename; failure removes the temporary file and preserves an
  existing report.
- The report is refreshed after normal worker terminalization, safe cancellation,
  force-cancel/recovery terminalization, full verification/supersession, and forget.
- Report refresh failure is a warning/log only. It cannot roll back or corrupt an
  already legal manifest or catalog terminal state.

## 3. A review corrections to the B delivery

A did not accept the B report solely from its passing test count. Review found and
closed these contract gaps:

1. Removed non-schema top-level inspection fields and supplied the missing catalog
   identity/visibility/verification/supersession view.
2. Removed large-file SHA work from ordinary inspection.
3. Corrected JSON trajectory output to the existing CLI envelope (no extra
   `exit_code`, required diagnostics present).
4. Corrected JSONL summary shape and prevented a second sequence-1 error envelope
   after an already-started stream.
5. Replaced broad forensic filename matching with the frozen provenance forensic
   prefix.
6. Corrected release/domain-boundary origin typing and required termination-table
   data for terminal state records.
7. Tightened all three new schemas from permissive placeholders to explicit,
   additional-property-rejecting product contracts.
8. Added automatic report lifecycle hooks and a real report-write-failure regression.

## 4. Windows evidence

Final current-source gates:

```text
cargo test --offline --workspace
  540 passed, 11 ignored

cargo clippy --offline --workspace --all-targets -- -D warnings
  passed

cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
  passed
```

The exact real CFSR runtime contract was also run independently before the full
workspace gate:

```text
real_zero_duration_cfsr_run_uses_daemon_worker_and_terminal_identity
  passed, 98.54 s
```

It covered production daemon/worker output, inspect/trajectory/report, full verify,
rerun, supersession, dry-run prune with zero deletion, forget persistence, safe
cancel, force cancel, daemon restart/worker reattachment, Interrupted recovery, and
report-refresh failure that preserved the verified catalog state.

An independent schema audit against the retained production result also passed:

```text
runtime root: target/m5-a2-runtime-LvdN4N
run id:       019fad0d-5043-71d3-b96b-9387ba03aa61
particle id:  7175256382879213655
JSON records: 2
JSONL items:  4
inspection / envelope / stream header / every record / every stream item: passed
```

## 5. WSL/Linux evidence

WSL distro: `Ubuntu-24.04`; repository path `/mnt/e/flexpart/trajecta`; independent
target `CARGO_TARGET_DIR=/tmp/trajecta-m5-a3-target`; offline only.

```text
trajecta-local-ipc: 2 passed
trajecta-job:       30 lib + 7 contracts passed
trajecta-cli lib:   20 passed
M5 runtime:         2 passed, including real CFSR (suite 109.00 s)
```

No package installation, network access, retry, or external-blocked substitution was
used.

## 6. Closeout boundary

M5-A3 is accepted in the current working tree. M5 is not complete: M5-A4 packaging
and cross-platform clean-extraction product matrix, followed by M5-A5 FLEXPART
comparison and the single planned prerelease version transition, remain future work.

The current changes remain uncommitted and unpushed pending user direction.
