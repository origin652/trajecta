# B Prompt：M4-A4.6 post-sorted-SQLite 正式 WSL 复验

你是 B，仅负责机械执行、artifact 保全和证据汇总。A 已完成性能根因调查与生产实现。不要修改任何源码、合同、schema、caps、容差、异常分类、版本号或测试门槛；不要 commit/push；不要运行百万点；不要触碰 `E:\flexpart\origo-validation-v1.json`；不要调用子代理；无 C 参与。

先完整阅读：

```text
docs/engineering/TRAJECTA_M4_A4_6_A_SQLITE_SCALING_FIX_REPORT.md
docs/engineering/TRAJECTA_M4_A4_6_B_POST_QUERY_CACHE_EXECUTION_REPORT.md
tools/run_m4_a4_real_matrix.py
tools/monitor_m4_a4_wsl_cell.py
```

## 1. 冻结目标

验证 A 的 sorted SoA SQLite sink 是否让正式 50k→100k scaling 同时满足：

```text
forward ratio  <= 2.4
backward ratio <= 2.4
```

其余既有 A4.6 hard gates 一个都不能退化。

## 2. 先跑本地门禁

在 `E:\flexpart\trajecta`：

```powershell
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline -p trajecta-core --lib
cargo test --offline -p trajecta-met --lib
cargo test --offline -p trajecta-core --test m4_a1_engineering production_digest_matrix_two_runs_content_and_sql_match -- --exact
cargo test --offline -p trajecta-core --test m4_a1_engineering production_digest_matrix_workers_order_chunk -- --exact
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/monitor_m4_a4_wsl_cell.py tools/run_m4_a4_real_matrix.py
git diff --check
```

预期当前计数：

```text
core lib: 138 passed
met lib: 161 passed
workspace: 454 passed / 11 ignored
```

若失败，停止并原样报告；不得修绿。

## 3. 冻结 source identity

正式运行前，让编排器记录当前完整 source identity。所有正式 cell 必须匹配同一个 source identity；若源码在运行期间变化，停止，不得混合 aggregate。

## 4. WSL preflight

使用全新 root：

```powershell
python tools/run_m4_a4_real_matrix.py preflight `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-sorted-sqlite-preflight
```

除既有 hard gates 外，额外检查 `M4_A4_CONCURRENT_READER.json`：

```text
schema_version = trajecta.m4-a4-concurrent-reader/v1
snapshot_contract = indexed_high_water_marks/v1; exact terminal row counts are postflight-only
successful_active_snapshots >= required
recorded sample 包含 database_page_count / output_event_max_sequence /
particle_max_id / particle_state_high_water
```

不得恢复运行期 `COUNT(*)` 全表轮询。终态 exact row counts 仍必须由 postflight audit 全量验证。

preflight 失败即停止。

## 5. 正式六格

使用另一个全新 root；不得 resume 旧 post-query-cache artifact：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-sorted-sqlite-formal `
  --stop-on-fail
```

冻结顺序：

```text
S50-F  forward  50,000  w4
S100-F forward 100,000  w4
S50-B  backward 50,000  w4
S100-B backward 100,000 w4
D100-F forward 100,000  w1
D100-B backward100,000  w1
```

不要重试进同一 attempt；任何重试必须新 attempt 且保留失败 attempt。首个失败后自动停止，不运行后续格。

## 6. 每格 hard gates

每格必须同时满足：

- direct release binary 与 SHA 证据存在；
- `RunOutcome::Complete`、manifest `complete`；
- abnormal termination = 0；
- lifecycle coverage valid；无 duplicate/nonmonotonic/state mismatch；
- mass ledger 无违反；
- execute-phase I/O delta 五项均为 0；
- scale-aware particle-loop query gate valid，repeated exact executions = 0；
- SQLite integrity = ok，terminal WAL = 0；
- provenance bundle 与 manifest identity 全部可独立重算；
- dictionary records/field_sets 不超冻结 caps；
- 100k peak RSS <= 2 GiB；
- 100k SQLite size 不超既有冻结上限；
- S50-F runner <= 720,000 ms；
- backward high-boundary origin/direction/exact-birth termination audit 全通过。

不允许以 external_blocked、CompletedWithParticleErrors、调整异常分类或放宽门槛代替失败。

## 7. Aggregate hard gates

正式六格齐全后：

```text
forward  = S100-F.runner_ms / S50-F.runner_ms <= 2.4
backward = S100-B.runner_ms / S50-B.runner_ms <= 2.4
```

并验证：

- forward 100k w1/w4 normalized content/SQL/canonical triplet 完全一致；
- backward 100k w1/w4 triplet 完全一致；
- 新 formal 相同方向/规模的 normalized triplet 与旧
  `target/m4-a4.6/post-query-cache-formal/summary/formal-20260727T001031Z.json`
  对应 cell 完全一致；
- exact bundle/SQLite SHA 可因新 run UUID 或物理插入布局变化而不同，不能拿它代替 normalized digest 比较。

## 8. 性能证据

对 S50-F、S100-F、S50-B、S100-B 汇总：

```text
runner_run_milliseconds
GNU user/system/wall
filesystem inputs/outputs
voluntary/involuntary context switches
peak RSS
particle_state rows / output events
SQLite / bundle size
```

重点比较旧 post-query-cache 正式证据，确认 system time 与 filesystem inputs 不再超线性膨胀。不要用 10k 外推代替正式比例。

## 9. 报告

写：

```text
docs/engineering/TRAJECTA_M4_A4_6_B_POST_SORTED_SQLITE_EXECUTION_REPORT.md
```

报告必须明确：

- B 只机械执行，未改实现；A 独立完成数值/性能裁决；无 C、无子代理；
- source identity、binary SHA、artifact root 与每格 attempt；
- 六格逐项结果与两个 scaling ratio；
- normalized digest cross-generation/w1-w4 比较；
- preflight indexed-reader 证据；
- 所有失败、partial attempt 或 external blocker 原样保留；
- 未 commit/push，不宣称 M4-A4.6/M4-A4/M4 完成，最终裁决交 A。
