# Trajecta M4-A4 execution report

**Status: A tooling/preflight passed; B formal matrix not started; M4-A4 is not complete.**

This report replaces the obsolete pre-O1/O2 P0 failure record. A completed the
medium-hard execution tooling and ran exactly one final-source WSL preflight.
No 50k/100k formal cell was launched. No commit or push was performed.

## Responsibility boundary

- A owns numerical/core changes, performance diagnosis, scientific/platform
  adjudication, baseline acceptance, and final M4-A4 sign-off.
- B is now available only for the frozen mechanical six-cell execution and
  evidence/report assembly described by `docs/B_PROMPT_M4_A4_EXECUTION.md`.
- There is no C model. A does not use subagents; B is invoked separately by the
  user through the written Prompt and must not delegate again.
- `E:\flexpart\origo-validation-v1.json` was not read, modified, moved, or
  added to Git.

## A-frozen tooling

| Item | Path / behavior |
|---|---|
| real-data harness | `crates/trajecta-core/tests/m4_a4_real_perf.rs` |
| matrix runner | `tools/run_m4_a4_real_matrix.py` |
| WSL concurrent reader | `tools/monitor_m4_a4_wsl_cell.py` |
| cell evidence | `M4_A4_CELL_SUMMARY.json`, command/binary identity, stdout/stderr, GNU time, monitor JSON |
| resume | only an audited passed cell with the same frozen source-tree identity is reused |
| aggregate formal gates | direction scaling ratio and 1/4-worker normalized digest equality |

The runner executes the selected release test binary directly after a separate
`--no-run` build. GNU `/usr/bin/time -v` supplies peak RSS. An independent
read-only/query-only process samples SQLite only while both the measured
process and `manifest.status=running` remain active. Terminal validation checks
integrity, lifecycle-derived row coverage, all REAL output columns for
nonfinite values, WAL truncation, artifact sizes, mass ledger, I/O and query
counts.

The Rust harness independently recomputes from disk:

- exact provenance-bundle and SQLite SHA-256;
- streaming normalized provenance content SHA-256;
- canonical ordered SQLite SQL SHA-256;
- canonical output SHA-256 and bundle counts.

## Final WSL preflight

Command:

```text
python tools/run_m4_a4_real_matrix.py preflight --platform wsl \
  --wsl-distro Ubuntu-24.04 --artifact-root target/m4-a4
```

Canonical evidence:

```text
target/m4-a4/cells/wsl__era5-hybrid__forward__p1000__w4/attempt-20/
target/m4-a4/summary/preflight-20260725T075051Z.json
```

`M4_A4_CELL_SUMMARY.json` SHA-256:

```text
263a04f41494196e85fec975fa4c9d7ea865088d111c710e08e8abd527379998
```

| Gate | Result |
|---|---:|
| outcome / manifest | Complete / complete |
| particles / steps / output events | 1,000 / 6 / 7 |
| abnormal terminations | 0 |
| normal population outflow | 109 |
| lifecycle state rows | 6,701 actual = 6,701 derived; maximum 7,000 |
| runner-only wall | 10,504 ms |
| lock/build/preload | 2,673 ms |
| measured test process | 13,177 ms; GNU time 14.35 s |
| peak RSS | 122,011,648 bytes |
| concurrent SQLite | 79 writer-active successes / 110 attempts; required 1 |
| SQLite main / WAL / SHM | 2,011,136 / 0 / 32,768 bytes |
| provenance bundle / run dir | 1,455,142 / 3,507,873 bytes |
| SQLite integrity / nonfinite values | `ok` / 0 |
| execute I/O delta | all five counters 0 |
| particle-loop query | 19 logical / 13 executed / 6 exact reuse |
| exact keys | 13 unique / 0 repeated executions |
| mass ledger | 0 tolerance violations |
| independent output identity | valid; no diagnostic |

The monitor recorded one early `no such table: output_event` observation while
the writer was still creating the schema. It subsequently obtained 79 valid
writer-active transactions, so this expected startup race is retained as raw
evidence and is not a gate failure.

Normalized digests:

```text
content_sha256
  6a8acea3a935ab69b4aa25cb6c72c92b22bcf55949295defb09e986d00aa27c2
sqlite_sql_sha256
  1779b14f7d6ab28f151b3992321e4f02be7fd2129ada5f003667ca28688ed2a9
canonical_output_sha256
  a7c88412980a7f68a2fe1d6d13f09cd1ebe138509e8b86509ba7d48f4459e705
```

Release binary:

```text
/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-1132c2b9322338df
size    12,086,352 bytes
sha256  c3bd42f3004f0bf25aa93be822a6ecb57ac53aac687dd42ff52d3c861fbd84c9
```

## Formal B execution still required

The frozen order remains:

1. S50-F: forward, 50k, w4
2. S100-F: forward, 100k, w4
3. S50-B: backward, 50k, w4
4. S100-B: backward, 100k, w4
5. D100-F: forward, 100k, w1
6. D100-B: backward, 100k, w1

B should run only:

```text
python tools/run_m4_a4_real_matrix.py formal --platform wsl \
  --wsl-distro Ubuntu-24.04 --artifact-root target/m4-a4 --resume
```

The runner enforces per-cell hard gates, 100k RSS <= 2 GiB, 100k SQLite <=
512 MiB, 50k->100k runner-only ratio <= 2.4, and equal normalized
content/SQL/canonical-output digests between w1 and w4 for each direction.

## Gates run before handoff

```text
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --workspace --no-deps
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

All passed. B must still rerun and save the Windows/WSL ordinary-gate logs in
its final execution package, because this A preparation run is not a substitute
for the later platform evidence. This report does not claim M4-A4 or M4
completion.
