# Trajecta M4-A4.6：invalid-meteorology 修复后 WSL 正式执行报告（B）

日期：2026-07-26
状态：**执行已停止：S50-F passed；S100-F failed；其余四格未运行。**
本报告不宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 责任与冻结身份

本轮仅执行 A 已冻结的门禁、WSL preflight、正式矩阵与 artifact 审计；B 未修改数值核心、输出、provenance 字典、合同、容差、SQLite schema、版本或验收规则。

- Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
- **执行开始 source tree SHA-256**：`a8ae2e480fb0e201dd2aa813112126c2cc2887335d1509137aa8e5f91bdd48a0`
- 执行 release binary：`/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae`
- binary SHA-256：`78f4854c067f2a2b11a11d5a6959d601e762631f28a47fbbcf4d2d09d0aeeaec`
- 平台：WSL Ubuntu-24.04；Linux `6.18.33.2-microsoft-standard-WSL2`
- Rust：`rustc 1.94.0` / `cargo 1.94.0`
- artifact root：`target/m4-a4.6/post-invalid-met-formal/`

未 reset、clean、checkout、stash 或覆盖既有改动；未读取、修改或报告外层 `origo-validation-v1.json` 内容；未 commit / push。

## 2. 起始核对与本地门禁

A 的短 invalid-met 预检 manifest 已按 Prompt 复算：

```text
target/m4-a4.6/invalid-met-fix-preflight-5/
  m4-a2-real-era5-hybrid-forward/
  019f9c29-d314-7350-9171-a3a971392cd7/run-manifest.json
SHA-256: ce64a52c8ef2422841bb0a2f20f3ac790f0dcf824ba951ff09353dda1e3e4784
status: complete
abnormal_count: 0
normal: population_outflow = 1
```

本地门禁全部通过；完整日志：
`target/m4-a4.6/post-invalid-met-formal/gates/local-gates.log`
（SHA-256 `2bebd9b5ade3da342393007c99808a5d2917117b65dec72918bab5dfc88683cd`）。

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| `cargo test --offline -p trajecta-core --lib` | 128 passed |
| `cargo test --offline -p trajecta-met derive::domain_fill` | 7 passed |
| `cargo clippy --offline -p trajecta-met -p trajecta-core --all-targets -- -D warnings` | passed |
| `cargo test --offline --release -p trajecta-core --test m4_a2_real_data real_hybrid_model_top_midpoint_replay -- --ignored --exact` | 1 passed |
| `git diff --check` | passed |

## 3. 新源码 WSL preflight

旧 A4.6 root 的 artifact 没有复用。新 root preflight 实际运行：

```text
python tools/run_m4_a4_real_matrix.py preflight --platform wsl \
  --wsl-distro Ubuntu-24.04 \
  --artifact-root target/m4-a4.6/post-invalid-met-formal --resume
```

结果：`passed`，1000 particles / forward / w4，exit 0，130.1 s。

| Evidence | Value |
|---|---|
| cell summary | `cells/wsl__era5-hybrid__forward__p1000__w4/attempt-1/M4_A4_CELL_SUMMARY.json` |
| summary SHA-256 | `c9598b10b0c2df02249c5386c8b7e7e49aa052bcd2be38393ae95e14d06b0ef8` |
| manifest status / abnormal | `complete` / 0 |
| normal termination | `population_outflow = 109` |
| scheduled lifecycle | 7 events; 6,701 expected / 6,701 actual rows; valid |
| execute I/O delta | inspect/build_index/decode/provider_frame_load/format_detection_open_attempt = all 0 |
| exact particle-loop query gate | `19 logical / 13 executed / 6 reuse`; unique=13; repeated=0 |
| SQLite | integrity `ok`; terminal WAL 0 |
| provenance | present; streaming semantic validation passed |
| normalized digests | content `c9fe4cc2…`; SQL `93e0458c…`; canonical output `bf731e9d…` |

Preflight phase summary：`summary/preflight-20260726T052258Z.json`，SHA-256 `cf819d62f01db3968d657de718802d4777e230f6424dba883aa58e8e7197eb23`。

## 4. 正式 6-cell 矩阵

执行命令：

```text
python tools/run_m4_a4_real_matrix.py formal --platform wsl \
  --wsl-distro Ubuntu-24.04 \
  --artifact-root target/m4-a4.6/post-invalid-met-formal \
  --resume --stop-on-fail
```

| Cell | 参数 | Status | Evidence |
|---|---|---|---|
| S50-F | forward / 50k / w4 | **passed** | `cells/wsl__era5-hybrid__forward__p50000__w4/attempt-1/` |
| S100-F | forward / 100k / w4 | **failed** | `cells/wsl__era5-hybrid__forward__p100000__w4/attempt-1/` |
| S50-B | backward / 50k / w4 | not run | stop-on-fail after S100-F |
| S100-B | backward / 100k / w4 | not run | stop-on-fail after S100-F |
| D100-F | forward / 100k / w1 | not run | stop-on-fail after S100-F |
| D100-B | backward / 100k / w1 | not run | stop-on-fail after S100-F |

### 4.1 S50-F passed

- cell summary SHA-256：`75d203aa5488e95921e32815d98dd26f24480b41b49e0437cef0de1dc0411985`
- manifest SHA-256：`e000ff4e06707d22cf9b35a5e151c588d076b7f3c88f193a816e4804fd04f36e`
- `RunOutcome::Complete` / manifest `complete`; abnormal=0
- normal `population_outflow=5,690`
- particles：initial=50,000; inflow=3,297; SQLite total=53,297
- lifecycle：7 scheduled events; expected/actual state rows=347,754/347,754; valid
- process wall=12:07.08; formal runner wall=647,039 ms
- peak RSS=690,327,552 bytes
- SQLite=102,203,392 bytes; WAL=0; integrity=`ok`
- bundle=151,912,094 bytes; records=45,005; field sets=9,001; samples=347,754
- execute I/O five-counter delta=0; repeated exact executions=0

### 4.2 S100-F failed — first failure

**Return code：101.** Harness received `CompletedWithParticleErrors` instead of `Complete`; this is a real formal failure, not `external_blocked`.

- cell summary SHA-256：`f7f53f52a32903c5394d5e7347549973ad00b5c55ecccef58730859da55cbd81`
- manifest SHA-256：`bea60de9bc940563db2f9f2c3aab1201cf4cb36538d5cb8b2f4a9ecff8cc7efd`
- direct process wall=28:51.49; wrapper elapsed=1,745.172 s
- peak RSS=1,415,696,384 bytes (`<=2 GiB`, but cell still failed)
- SQLite=208,044,032 bytes; WAL=0; integrity=`ok`
- provenance bundle exists, size=329,250,397 bytes, SHA-256 `82cef6afc4fef0fdf80039cfe74c1f3ca8c52389c5ac7c0dcfb94ff877c9a00e`
- provenance dictionaries did **not** exceed frozen caps: records=101,755 (`<128,000`); field sets=20,351 (`<64,000`); samples=706,178
- normalized identities were written by the failed manifest: content `be70d84a…`; SQL `4ea8f877…`; canonical output `924055fe…`; B does not treat their presence as a passing audit after particle errors.

Termination measurement:

```text
manifest status: completed_with_particle_errors
normal_count:   11,461
  population_outflow: 11,460
  model_top:              1
abnormal_count: 3
  invalid_meteorology: 1
  reflection_limit:    2
```

Lifecycle audit is also invalid: expected 7 scheduled times, 7 present, but one duplicate scheduled event; expected lifecycle rows=706,176 and actual=706,178; one nonmonotonic sample-event particle and two scheduled state mismatches.

The test stderr’s first terminal assertion is:

```text
left: Ok(CompletedWithParticleErrors)
right: Ok(Complete)
```

Concurrent reader still met its writer-active count requirement: 6,126 successful snapshots, required=3; busy/locked=0; other_errors=1. The available failure summary omits final execute-I/O/query snapshots because the harness assertion occurred after the runner returned an unacceptable outcome; they are **unmeasured**, not assumed passing.

## 5. Formal aggregate and stop point

Formal phase summary:

```text
target/m4-a4.6/post-invalid-met-formal/summary/formal-20260726T060658Z.json
SHA-256: 9960eee4ecb350df2fab92278184724ef8d901b12a9881b8e1b8ba375401d020
```

Its aggregate reports `observed_cell_count=2` of 6 and `passed=false`. No 50k→100k scaling ratio, reverse scaling, or 1-worker/4-worker determinism comparison can pass because the S100-F cell failed and the remaining formal cells were not run.

Full first-failure evidence remains in the original, non-overwritten attempt directory:

```text
target/m4-a4.6/post-invalid-met-formal/cells/
  wsl__era5-hybrid__forward__p100000__w4/attempt-1/
```

It includes `command.json`, `binary-identity.json`, host/source identity, `stdout.log`, `stderr.log`, GNU time, concurrent-reader JSON, cell summary, manifest, SQLite, WAL/SHM and provenance bundle.

## 6. Handoff to A

B stopped on the first failed formal cell, did not retry, did not alter parameters or failure classifications, and did not run the dependent remaining cells. A must adjudicate the remaining `invalid_meteorology=1`, `reflection_limit=2`, and the failed lifecycle evidence before any new formal run.

**未 commit / 未 push；不宣称 M4-A4.6、M4-A4 或 M4 完成。**
