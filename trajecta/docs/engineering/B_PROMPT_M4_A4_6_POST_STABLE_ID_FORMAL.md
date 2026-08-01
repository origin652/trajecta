# B Prompt：M4-A4.6 post-stable-ID 最终 WSL 正式复验

你是 B，只负责机械执行、artifact 保全和证据汇总。A 已独立完成根因调查、生产实现、数值裁决和 backward 50k/100k 定向验证。不要修改任何源码、合同、schema、caps、容差、异常分类、版本号、测试门槛或执行参数；不要 commit/push；不要运行百万点；不要触碰 `E:\flexpart\origo-validation-v1.json`；不要调用子代理；无 C 参与。

先完整阅读：

```text
docs/engineering/TRAJECTA_M4_A4_6_A_STABLE_ID_SCALING_FIX_REPORT.md
docs/engineering/TRAJECTA_M4_A4_6_B_POST_SORTED_SQLITE_EXECUTION_REPORT.md
tools/run_m4_a4_real_matrix.py
tools/monitor_m4_a4_wsl_cell.py
```

## 1. 冻结目标

在最终源码身份上证明：

```text
forward  = S100-F.runner_ms / S50-F.runner_ms <= 2.4
backward = S100-B.runner_ms / S50-B.runner_ms <= 2.4
```

其余 A4.6 individual/aggregate hard gates 一个都不能退化。A 的定向 backward pair `2.256167057756394` 只是实现证据，不能复制、resume 或代替本轮正式六格。

## 2. 本地门禁

在 `E:\flexpart\trajecta` 按顺序运行：

```powershell
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline -p trajecta-core --lib
cargo test --offline -p trajecta-met --lib
cargo test --offline -p trajecta-core --test m4_a1_engineering production_digest_matrix_two_runs_content_and_sql_match -- --exact
cargo test --offline -p trajecta-core --test m4_a1_engineering production_digest_matrix_workers_order_chunk -- --exact
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/validate_m4_a0_contracts.py tools/monitor_m4_a4_wsl_cell.py tools/run_m4_a4_real_matrix.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

预期计数：

```text
core lib: 139 passed
met lib: 161 passed
workspace: 455 passed / 11 ignored
```

任一门失败就停止，保留原始日志并交 A；不得自行修绿。

## 3. 冻结 source identity

本地门禁通过后，不再修改仓库文件。让编排器记录完整 source identity；preflight 和六个 formal cell 必须全部匹配同一 identity。若运行期间源码身份变化，立即停止，不得混合 aggregate。

本轮必须使用全新 roots，不能 resume 或复制以下旧证据：

```text
target/m4-a4.6/post-query-cache-formal/
target/m4-a4.6/post-sorted-sqlite-formal/
target/m4-a4.6/a-stable-id-fastpath-backward-pair/
```

## 4. WSL preflight

```powershell
python tools/run_m4_a4_real_matrix.py preflight `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-stable-id-preflight
```

除所有既有 hard gates 外，确认：

```text
M4_A4_CONCURRENT_READER.json:
  schema_version    = trajecta.m4-a4-concurrent-reader/v1
  snapshot_contract = indexed_high_water_marks/v1; exact terminal row counts are postflight-only
  successful_active_snapshots >= required
  sample 含 database_page_count / output_event_max_sequence /
             particle_max_id / particle_state_high_water
```

不得恢复运行期 `COUNT(*)` 全表轮询。preflight 失败即停止。

## 5. 正式六格

preflight 通过后运行：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-stable-id-formal `
  --stop-on-fail
```

冻结顺序：

```text
S50-F   forward   50,000  w4
S100-F  forward  100,000  w4
S50-B   backward  50,000  w4
S100-B  backward 100,000  w4
D100-F  forward  100,000  w1
D100-B  backward 100,000  w1
```

首个失败后停止，不运行后续格。不要覆盖 attempt；若外层进程异常中断，保留 partial attempt 并交 A 裁决，不得自行重试或以旧 cell 补齐。

## 6. 每格 hard gates

每格必须同时满足：

- direct WSL release binary、binary SHA 与 source identity 证据存在；
- `RunOutcome::Complete`、manifest `complete`、abnormal termination = 0；
- lifecycle coverage valid，无 duplicate/nonmonotonic/state mismatch；
- mass ledger 无违反；
- execute-phase I/O 五项 delta 均为 0；
- scale-aware particle-loop query gate valid，repeated exact executions = 0；
- SQLite integrity `ok`，terminal WAL=0；
- provenance bundle、manifest identity 与 normalized digests 可独立重算；
- dictionary record/field-set caps 不超冻结上限；
- 100k peak RSS <= 2 GiB；
- 100k SQLite size 不超既有冻结上限；
- S50-F runner <= 720,000 ms；
- backward high-boundary origin/direction/exact-birth termination audit 全通过；
- concurrent reader 满足 indexed high-water contract。

不允许以 `external_blocked`、`CompletedWithParticleErrors`、调整异常分类、改变 caps 或放宽门槛代替失败。

## 7. Aggregate hard gates

六格齐全后计算：

```text
forward ratio  <= 2.4
backward ratio <= 2.4
```

并验证：

- forward 100k w1/w4 normalized content/SQL/canonical triplet 完全一致；
- backward 100k w1/w4 triplet 完全一致；
- 每个新 formal cell 的 normalized triplet 与
  `target/m4-a4.6/post-sorted-sqlite-formal/summary/formal-20260727T063601Z.json`
  相同方向/规模 cell 完全一致；
- backward 50k/100k triplet 还应与 A 的
  `target/m4-a4.6/a-stable-id-fastpath-backward-pair/`
  对应 cell 完全一致；
- exact bundle/SQLite file SHA 可因 run UUID 与物理布局不同而不同，不能替代 normalized digest 比较。

## 8. 性能汇总

至少汇总六格的：

```text
runner_run_milliseconds
GNU user/system/wall
filesystem inputs/outputs
voluntary/involuntary context switches
peak RSS
seeded/inflow/final particle counts
particle_state rows / output events
query counts by origin
SQLite / bundle sizes
record_count / field_set_count
```

重点比较：

```text
旧 post-sorted backward:
  50k  = 222,240 ms
  100k = 544,803 ms
  ratio = 2.451417386609071

A stable-ID 定向证据:
  50k  = 202,852 ms
  100k = 457,668 ms
  ratio = 2.256167057756394
```

正式结果必须使用本轮六格自身计时计算，不得直接沿用 A 的数字。

## 9. 报告

写：

```text
docs/engineering/TRAJECTA_M4_A4_6_B_POST_STABLE_ID_EXECUTION_REPORT.md
```

报告必须明确：

- B 只做机械执行，未改实现；A 独立负责根因、实现与最终裁决；无 C、无子代理；
- 本轮 source identity、binary SHA、roots 与每格 attempt；
- 本地门禁、preflight、六格 individual hard gates；
- forward/backward 两个 scaling ratio；
- w1/w4 与 cross-generation normalized digest 比较；
- reader、RSS、SQLite/WAL、bundle、caps、query、lifecycle 和 backward audit 证据；
- 任何失败、partial attempt 或 external blocker 原样保留；
- 未 commit/push，不自行宣称 M4-A4.6、M4-A4 或 M4 完成，最终裁决交 A。
