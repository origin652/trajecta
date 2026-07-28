# Trajecta M5-A1 B delivery report — CLI and project control plane

**Status: B-owned corrections accepted by A; A-owned `data lock` and
`project finalize` closeout is implemented and locally accepted.**

M5-A1 is locally complete against the frozen command surface and gates in this
report. M5 as a whole is not complete. No commit or push was made. The
implementation is confined to the M5 CLI/project boundary and the public
meteorology lock-requirements verifier; it does not alter numerical algorithms,
runner lifecycle semantics, meteorological profiles, SQLite output schema, or
M4 evidence artifacts.

## Delivered surface

### Typed command tree and output protocol

`trajecta-cli` now parses the complete frozen M5 command tree and global
options (`--format`, `--config`, and `--project`) without Unicode path
assumptions:

* `config init|get|set|unset|list|validate`
* `project init|status|show|validate|get|set|unset|data-plan|finalize`
* `case validate|resolve`
* `data inspect|lock`
* `doctor [--deep]`
* existing `met` command compatibility is retained
* `run`, `job`, and `result` parse but return the stable runtime-stage
  diagnostic `command.stage_not_available`, rather than a usage error.

`--format json` emits the frozen `trajecta.cli-output/v1` envelope for success,
application error, and argument-parse error.  The envelope's `command` is the
complete command path (for example `project status`, `project unknown`, or
`met probe`).  `--format jsonl` emits ordered
`trajecta.cli-stream-item/v1` data/diagnostic/summary records without a blank
trailing record.  Human output remains separate.  In machine mode a met
command that would write native records to stdout is rejected with an envelope
unless its native output is redirected to a file, preventing mixed stdout.

### Config and project behavior

* Config uses typed TOML (`trajecta.config/v1`), unknown-field rejection, the
  strict cross-field invariant `memory_reserve_mib < memory_pool_mib`, and
  file-permission diagnostics on Unix. Server/credential arrays are deliberately
  rejected by the frozen schema rather than accepted or reordered. `config set`
  supports creation of a named `profile_templates.<name>` object.
  It flushes and syncs a same-directory temporary file before atomic replace;
  semantic failures leave the previous file bytes unchanged.
* Config resolution has the required three-layer order: explicit `--config`,
  injected `TRAJECTA_CONFIG`, then the platform default.  `doctor` validates
  the selected configuration and returns `doctor.config_invalid` when it is
  absent or malformed rather than silently ignoring `--config`.
* Project discovery accepts a root or index, parses the frozen
  `trajecta.project-index/v1` index, handles missing Profile fields as
  `draft`, and reports actual parse/unknown-field/type errors as `error`.
  No broad `Err(_)` fallback fabricates a resolved RunProfile.
* Every project selector mutation is revalidated before atomic write.  Index
  paths strictly reject absolute paths, `.`/`..`, repeated separators, and any
  lexical or canonical escape from the project root; rejected operations do
  not modify the index or an outside file.
* Configured projects may deliberately have missing local locks: profile shape
  and Case resolution remain valid, while lock status is `missing` with no
  roots, `partial` when an existing configured root has local content, and
  `ready` only after public lock verification.

### Case, planning, data, and doctor

* `case validate` performs shape and intent validation; `case resolve` uses the
  public Case resolver and returns normalized resolved data.
* `project data-plan` uses public Case/profile expansion and
  `required_capabilities_for_population`; its output is canonicalized,
  runtime-checked against the frozen data-plan shape (including optional
  `cache_root` omission rather than JSON `null`), and atomically written when
  `--output` is supplied.  All emitted project-derived paths are relative.
* The table-driven full Case/RunProfile fixture produces byte-identical
  **complete CLI JSON stdout** on two invocations. Its integration test freezes
  the full stdout SHA-256 as:

  ```text
  e4aa921372b5412a7c45e4d3b2423c98688170dc6ccab579c89d464f64177714
  ```

* `data inspect` delegates format recognition and metadata inspection to the
  public meteorology reader API.
* `doctor --deep` exercises project-root write/rename/remove, creates a SQLite
  probe, enables WAL, checks `integrity_check`, checkpoints with `TRUNCATE`,
  and cleans up the database/WAL/SHM files.  It also invokes the public lock
  verifier for existing locks and public meteorology metadata inspection for
  existing configured roots.  Daemon/IPC checking remains explicitly deferred
  to M5-A2.

## A review-closeout: immutable data and finalize

`data lock` now parses the A-corrected frozen input surface:

```text
data lock --root DIR --profile NAME --case FILE --output PATH [--replace]
```

The A closeout now derives physical coverage as
`min(time.start,time.end)..max(time.start,time.end)`, obtains capabilities from
the public population helper, requires exactly one logical dataset for the
standalone command, and delegates file discovery, Profile matching, frame
selection, hashing, and lock construction to `DatasetLockBuilder`. The output
is parsed back through the authoritative lock parser before a synced
same-directory temporary file is atomically installed. Existing output without
`--replace` returns `data.lock_exists`; replacement occurs only when the user
passes `--replace`.

`project finalize` resolves every Case and RunProfile into lock requirements.
Uses of one lockfile are grouped; coverage and capabilities are unioned, while
dataset, Profile, roots, Profile sources, or reader conflicts hard fail. Every
existing lock is validated before any missing lock is built. Missing locks are
built fully in memory, output roots are created and write-probed, then locks are
installed without replacement. A failed multi-lock install rolls back only
newly installed, byte-identical files. Postflight must dynamically derive
`finalized`; repeated finalize reuses valid locks byte-for-byte.

Existing-lock validation checks logical dataset identity, exact active Profile
name and content SHA, capability availability, physical coverage including
interpolation/warm-up frames, local payload SHA, and inventory assembly. A Case
or Profile edit therefore downgrades the project dynamically and cannot leave a
stale lock masquerading as finalized.

Daemon-backed `run`, `job`, and `result` remain M5-A2/M5-A3 boundaries. No
daemon, scheduler, run executor, job persistence, result serving, or pruning
implementation was introduced in A1.

## Changed files owned by this delivery

```text
Cargo.lock
crates/trajecta-cli/Cargo.toml
crates/trajecta-cli/src/app.rs
crates/trajecta-cli/src/case_data.rs
crates/trajecta-cli/src/cli.rs
crates/trajecta-cli/src/configuration.rs
crates/trajecta-cli/src/data_lock.rs
crates/trajecta-cli/src/lib.rs
crates/trajecta-cli/src/project.rs
crates/trajecta-cli/src/command/config.rs
crates/trajecta-cli/src/command/data.rs
crates/trajecta-cli/src/command/doctor.rs
crates/trajecta-cli/src/command/mod.rs
crates/trajecta-cli/src/command/project.rs
crates/trajecta-cli/src/command/staged.rs
crates/trajecta-cli/tests/m5_a1_cli_contracts.rs
crates/trajecta-cli/tests/public_module_contracts.rs
crates/trajecta-met/src/io/lock_builder.rs
docs/TRAJECTA_M5_A1_B_DELIVERY_REPORT.md
```

The repository also contains A0/new-baseline files that predate this B work;
they are intentionally not represented as B implementation changes.

## Verification

Executed after implementation:

```text
cargo test --offline -p trajecta-cli
cargo test --offline --workspace
cargo clippy --offline -p trajecta-cli --all-targets -- -D warnings
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

Results:

```text
trajecta-cli: 45 passed
workspace: 487 passed, 11 ignored
clippy/doc/fmt/diff-check: passed
M5 A0 validator: passed
M4 A0 validator: passed
M5 CLI commands: 35
known CLI placeholders: 0 (baseline was 11)
```

The added `m5_a1_cli_contracts` suite covers 35 command parse paths, duplicate
and conflicting global flags, injected config priority, config type/template/
strict reserve and old-SHA preservation, project init/status, path escape
rejection, draft-versus-real-error classification, Case-derived deterministic
planning, three public population capability-helper paths, deep doctor
config/SQLite behavior, JSON/JSONL and met/parse error envelopes, staged-command
exit behavior, real CFSR `data lock` create/no-replace/replace, idempotent
project finalize, stale-lock rejection, and invalid index
name/dataset/template/SHA cases. The private data-plan guard unit suite rejects
unsorted or duplicate requirements/capabilities, inverted coverage, null cache
roots, and escaping lock/data-root paths.

### Review-closeout Fixup2

Project discovery now first deserializes the raw YAML index into
`serde_yml::Value`, then converts that value to `ProjectIndex`. This makes
mapping-level duplicate YAML keys fail before typed map conversion can discard
the evidence. The CLI black-box regression covers duplicate keys in `cases`,
`profiles`, and nested `profiles.*.dataset_profiles`; for each it asserts exit
1, `project.invalid_index`, empty machine-mode stderr, and unchanged input
index bytes. The deterministic full JSON stdout data-plan fixture is frozen at
`e4aa921372b5412a7c45e4d3b2423c98688170dc6ccab579c89d464f64177714`.

### A closeout evidence

The real CFSR test uses the frozen 2009-01-01 00/06/12 pgbl fixtures and the
built-in `cfsr-pgbl-pressure-v0` Profile. It proves:

- standalone lock creation and explicit replacement produce identical bytes;
- no-replace preserves the existing file;
- first project finalize creates the missing lock and output root;
- second finalize reuses the same lock without rewriting it;
- changing the Case to require unavailable following coverage makes finalize
  fail while preserving the old lock bytes;
- backward Case requirements normalize to increasing physical coverage;
- shared lockfile requirements union coverage/capabilities and reject reader
  or source conflicts.

## Handoff

A has reviewed the B closeout and completed the remaining immutable-data and
finalize path. M5-A1 is locally accepted. M5-A2 must not start until requested
by the user.

**No commit. No push. M5 is not complete.**
