# Trajecta M4-A4.6 A：SQLite 输出缩放修复报告

日期：2026-07-27
状态：**A implementation + WSL 10k attribution passed / formal 50k·100k matrix pending**

本轮由 A 独立调查、实现与验证，未使用子代理。后续 B 仅按书面 Prompt 做机械长测；无 C 参与。

本报告不宣称 M4-A4.6、M4-A4 或 M4 完成；未 commit、未 push；未修改数值算法、科学容差、异常分类、SQLite schema、dictionary caps 或任何版本号；未运行百万点测试；未触碰 `E:\flexpart\origo-validation-v1.json`。

## 1. 输入失败

`post-query-cache` 正式六格 individual cell 全部通过，仅 aggregate scaling 失败：

```text
forward:  1,666,472 / 654,036 = 2.5479820682653553 > 2.4
backward: 1,700,220 / 666,402 = 2.5513428831246006 > 2.4
```

六格的数值、生命周期、query、I/O、SQLite/WAL、bundle、digest、RSS 与异常终止门均已通过，因此本轮只处理真实性能缩放。

## 2. 被否决的首个假设

最初发现 provenance lockstep 与 canonical digest 的 SQL 缺少 `run_id` 条件，查询计划会出现全表扫描和临时排序。A 补齐 run-scoped 查询后，在真实 100k SQLite 上做 WSL 只读微基准，发现 canonical `particle_state_by_time` 是非覆盖索引：

```text
非覆盖 time-index 热扫描：约 14.271 s
原顺序扫描 + temp sort：  约  3.957 s
```

因此“消灭所有 TEMP B-TREE 就会更快”被实测否决。最终实现保留 run scope，但通过等价的 `ORDER BY +physical_seconds` 让 SQLite 使用 run 的主键范围顺序扫描后排序：

```text
SEARCH particle_state USING PRIMARY KEY (run_id=?)
USE TEMP B-TREE FOR ORDER BY
```

两次 WSL 实测为 4.015 s / 4.046 s。unary `+` 不改变 INTEGER 顺序或 digest 字节，只阻止 mounted output 上代价更高的随机回表。对应查询计划与 foreign-run 隔离均有单测。

provenance lockstep 查询保留：

```sql
WHERE run_id = ?1
ORDER BY particle_id, sample_sequence
```

并命中复合主键，无临时排序。

## 3. 真正主因

当前源码的 WSL attribution 对比显示，1k→10k 时：

| 指标 | 1k | 10k | 倍率 |
|---|---:|---:|---:|
| particle_state rows | 6,701 | 67,127 | 10.02× |
| lifecycle/output events | 116 | 1,128 | 9.72× |
| runner total | 4.91 s | 77.18 s | 15.72× |
| runner output sink | 0.35 s | 47.92 s | 约 139× |

生产 `ParticleStateSqliteSink::write_event` 有两个叠加问题：

1. event 分类扫描通过 `ParticleBatch::state()` 把每个 SoA 粒子完整 materialize；正式写入循环又 materialize 第二遍；
2. 稳定 particle ID 是哈希 ID，旧路径按粒子存储顺序插入 `(run_id, particle_id, sample_sequence)` WITHOUT ROWID 主键，导致大库在 `/mnt/e` 上发生随机 B-tree 页访问。

GNU time 证据与该机制一致：10k 旧路径的 system time 为 7.73 s，filesystem inputs 为 8,388,340，sink 墙钟远高于 CPU 上应有的线性成本。

并发 reader 原先每 0.1 s 做三个 `COUNT(*)`，虽不是最终主瓶颈，但不符合“只证明 writer-active read snapshot”的最小职责。它已改为主键/covering-index 高水位探针；终态精确行数仍只由既有 postflight 全量审计计算一次。

## 4. 实现

### 4.1 SQLite sink

`crates/trajecta-core/src/output/sqlite.rs`：

- event 分类直接读取 `ParticleBatch` SoA 列，不再构造完整 `ParticleState`；
- 正式写入同样直接读取 SoA；
- `insert_particle` 直接按列借用 population/origin/mass；
- `insert_termination` 只借用 typed termination metadata；
- 每个 event 的写入 indices 按 `ParticleId` 升序排序；测试用 reverse knob 仍可反向压力验证；
- row 内容、sample sequence、event sequence、termination time/fraction、bundle sample mapping 与 normalized digest 公式不变。

### 4.2 终态审计查询

- provenance lockstep 与 canonical digest 均显式绑定唯一 `run_id`；
- inspect 对非单-run SQLite hard fail；
- wide `particle_state` canonical export 避免非覆盖 time-index 随机回表；
- 新增 query-plan 与 foreign-run 隔离回归。

### 4.3 并发 reader

`tools/monitor_m4_a4_wsl_cell.py`：

- 删除运行期间反复 `COUNT(*)`；
- 改为 output event、particle、particle_state 的 indexed high-water probes；
- 保留同一只读 URI、`query_only`、短事务、writer-active 前后状态检查；
- terminal exact counts 仍由 postflight 审计，不减少验收覆盖。

## 5. WSL 10k A/B 证据

两次运行使用相同真实 ERA5 hybrid、forward、10,000 粒子、4 workers、1 小时、600 s step/output。比较基线已包含 indexed reader，因此下表隔离的是 sorted SoA sink 修复。

| 指标 | 修复前 | 修复后 | 变化 |
|---|---:|---:|---:|
| runner total | 77.183 s | 34.154 s | -55.75% |
| runner output | 57.55 s | 14.55 s | -74.72% |
| runner output sink | 47.92 s | 4.39 s | -90.84% |
| system time | 7.73 s | 1.71 s | -77.88% |
| filesystem inputs | 8,388,340 | 1,537,897 | -81.67% |
| peak RSS | 194,523,136 B | 194,142,208 B | essentially unchanged |

修复后仍满足：

- `RunOutcome::Complete`、manifest `complete`；
- abnormal termination = 0；
- lifecycle、mass ledger、query gate、execute I/O delta、SQLite integrity、terminal WAL、bundle semantic validation 全通过；
- rows = 67,127、events = 1,128，与修复前完全相同；
- normalized digest triplet 完全相同：

```text
content:  3c5af25acac17cf0af2891fed2cdfa47719b590bccaf420f32edc885cde7a272
SQL:      2adfadfd58cefd7117deed0eb73dddc77b8ba6d2c88be24dd47c9ac69afbe73d
canonical:9329886b743be86c0d978f36cc6fb721a0b746beb810af809997197d313988f0
```

修复后 artifact：

```text
target/m4-a4.6/a-scaling-diagnostic-10k-sorted-sink/
```

执行 source identity：

```text
206af5edc7c0ddaa05fa71a68c86dfc8027c2d89450edaa62ba45489196d007c
```

## 6. 本地门禁

```text
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
cargo fmt --all -- --check
python -m py_compile tools/monitor_m4_a4_wsl_cell.py tools/run_m4_a4_real_matrix.py
git diff --check

=> passed
=> workspace: 454 passed / 11 ignored
=> core lib: 138 passed
=> met lib: 161 passed
```

定向通过：

- canonical digest run-scoped/query-plan/foreign-run tests；
- provenance lockstep run-scoped primary-key test；
- production digest matrix across workers/order/chunk；
- WSL 10k before/after exact normalized-digest comparison。

## 7. 未闭合

这次 10k 结果强烈支持 50k→100k scaling 回到 `<=2.4`，但不能替代正式矩阵。下一步必须由全新 source identity、全新 artifact root 的 WSL 50k/100k forward/backward 正式运行给出最终比例；随后才运行 100k w1 determinism cells 并裁决 A4.6。
