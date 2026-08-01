# Trajecta M5-A2 B Mechanical Execution Report

**Status:** B mechanical re-verification passed.  Final acceptance remains with A.

This report records a frozen-source execution only.  B did not modify Rust, Python,
TOML, schema, contract, test, daemon scheduling, recovery, state-transition, runner,
or M4 evidence implementation; did not run A2E reduction; did not delete artifacts;
and did not commit, push, tag, or publish.

## 1. Execution identity

| Item | Value |
|---|---|
| Start HEAD | `246169a1dd6e073fca2b793cd32280ee883d09f0` |
| Branch | `main` |
| Start tree | Dirty; 63 pre-existing status entries (M5/M4 implementation and contracts), no B source edit during this execution |
| Start `git diff --check` | passed |
| A delivery report SHA-256 | `e6cdc76c01a07bc73293e85b8db328da0a319d68c9661818e06977b578e3a4f8` |
| Windows | Windows 10 10.0.19045, 64-bit, `DESKTOP-NO3FH8I` |
| Rust | `rustc 1.95.0 (59807616e 2026-04-14)`, host `x86_64-pc-windows-gnu` |
| Cargo | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Crate versions | checked: `0.0.0` |
| Start origo SHA-256 | `d6e79bb88bbedf224f8f55745caf3995b6528a9b63cb679ed3daca99ac1253b0` |

### CFSR fixture identity

All three required files exist below
`E:\flexpart\tools\flexctl\target\test-data\cfsr\20090101\raw`:

| File | SHA-256 |
|---|---|
| `pgbl00.gdas.2009010100.grb2` | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| `pgbl00.gdas.2009010106.grb2` | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| `pgbl00.gdas.2009010112.grb2` | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |

## 2. Windows mandatory gates

All commands below completed with exit code 0, in the prescribed order.  The execution
harness did not expose independently timestamped wall-clock start/end pairs for every
short command; where Cargo reports a suite duration, it is stated explicitly.  The
runtime-contract command was executed (not `--no-run`).

| Command | Result | Count / duration |
|---|---|---|
| `cargo fmt --all -- --check` | passed | exit 0 |
| `cargo test --offline -p trajecta-job` | passed | 31 passed |
| `cargo test --offline -p trajecta-cli --lib` | passed | 17 passed |
| `cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture` | passed | 2 passed; test suite reported 82.85 s |
| `cargo test --offline --manifest-path vendor\trajecta-local-ipc\Cargo.toml` | passed | 2 passed |
| `cargo test --offline --workspace` | passed | 521 passed / 11 ignored |
| `cargo clippy --offline --workspace --all-targets -- -D warnings` | passed | exit 0 |
| `cargo clippy --offline --manifest-path vendor\trajecta-local-ipc\Cargo.toml --all-targets -- -D warnings` | passed | exit 0 |
| `cargo doc --offline --no-deps` | passed | exit 0 |
| `python tools\validate_m5_a0_contracts.py` | passed | 35 CLI commands / 9 job states / 0 placeholders / `pruning_mode=dry_run` |
| `python tools\validate_m4_a0_contracts.py` | passed | exit 0 |
| `git diff --check` | passed | exit 0 |

## 3. Real-process contract audit

The two `m5_a2_runtime_contracts` tests ran against real CFSR data and passed.  Their
assertions cover and passed the following ten required contracts:

1. daemon on-demand startup, single-instance lease, and idle release;
2. real CFSR foreground `Complete`, with matching catalog/manifest identity;
3. external-memory pressure pauses only new dispatch and persists one de-duplicated warning;
4. two event consumers from the same cursor receive the same events;
5. queued cancellation produces no run artifact;
6. 10,000-particle safe cancellation ends `Cancelled`, retains provenance/SQLite, has
   absent-or-zero terminal WAL, and reports `run_success=false`;
7. 10,000-particle force cancellation ends `Interrupted`, retains forensic data, and
   omits provenance;
8. killing the default foreground client leaves the independent worker and job `Running`;
9. killing the daemon allows a new daemon to reattach the same worker PID/start token and
   emits `worker.reattached`; and
10. the reattached worker can be force-stopped and finalized by the new daemon.

The test harness intentionally keeps runtime evidence under `target/m5-a2-runtime-*`.
Eight directories were present at end audit.  No directory was deleted or overwritten.
The newest was:

```text
target/m5-a2-runtime-yTOVQw/
```

It contains retained cancellation/force-cancellation evidence, including the catalog,
worker logs, SQLite files, and two CFSR run directories.  This is test-owned forensic
material, not a mandatory-gate failure.  The runtime test itself passed; no first-failure
forensic directory was produced by this B execution.

## 4. WSL/Linux smoke

WSL distro: `Ubuntu-24.04`; repository access verified at
`/mnt/e/flexpart/trajecta`; offline Cargo metadata and the three required `pgbl00`
fixtures were available.  The smoke used only:

```text
CARGO_TARGET_DIR=/tmp/trajecta-m5-a2-b-target
```

| Command | Result |
|---|---|
| `cargo test --offline --manifest-path vendor/trajecta-local-ipc/Cargo.toml` | passed, 2 passed |
| `cargo test --offline -p trajecta-job` | passed, 31 passed |
| `cargo test --offline -p trajecta-cli --lib` | passed, 16 passed |
| `cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture` | passed, 2 passed; test suite reported 89.55 s |

The WSL CLI lib count is 16 rather than the Windows platform count of 17 because of a
platform-conditional test; it is not a Windows source-drift signal.  No `external_blocked`
condition occurred and no WSL formal product matrix was run.

## 5. End audit

| Check | Result |
|---|---|
| End `git diff --check` before this report | passed |
| End origo SHA-256 | `d6e79bb88bbedf224f8f55745caf3995b6528a9b63cb679ed3daca99ac1253b0` (matches start) |
| Windows `trajecta*` processes | none |
| WSL Trajecta/M5-A2 processes | none |
| Version change | none; inspected crate version remains `0.0.0` |

After writing this report, B will only run the prompt-authorized `cargo fmt --all -- --check`
and `git diff --check` checks.  B makes no M5-A2/M5/release completion claim; final
acceptance is delegated to A.
