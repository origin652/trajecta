# Trajecta M4-A4.5 A 实现与最小归因报告

日期：2026-07-25
状态：**O1 + O2 implemented / repeated WSL evidence passed / M4-A4.5 not complete**
责任：A 独立完成观测设计、数值解释、O1/O2 实现和复验；A 未调用子代理。此前 B 仅按书面 Prompt 机械执行冻结矩阵与 profiler 探测，不参与本轮 O2 代码修改或裁决。

本报告不宣称 M4-A4.5、M4-A4 或 M4 完成；未 commit、未 push，未修改 `E:\flexpart\origo-validation-v1.json`。

## 1. 本轮落地

### 1.1 默认关闭的低扰动观测核心

新增 `crates/trajecta-met/src/performance.rs`：

- 只有显式安装 `PerformanceCounters` 后才计时；正常生产路径默认关闭；
- 固定阶段 ID，不接受运行时字符串标签；
- 原子累计 observations、total ns、maximum ns；
- 固定 64 桶 log2 histogram，不逐调用写日志；
- boundary 路径额外记录 samples、corner samples、segments、certification passes、residual nodes 和 maximum depth。

观测已接入：

- runner：population、integrator、boundary、output、output finish；
- output：气象 prepare/execute、sink、SQLite finish；
- SQLite finish：audit、canonical SQL digest、SQLite file SHA、provenance bundle；
- provenance：external sort、stream write、semantic validation、final hash、cleanup；
- met/query：window preparation、boundary query、grid locate、stencil、cache lock wait/hold、column build/sample；
- boundary certified path：path build、certificate/root isolation 和路径分布。

### 1.2 Harness、schema 与工具

- `crates/trajecta-core/tests/m4_a4_real_perf.rs`
  - `TRAJECTA_M4_A4_MODE=attribution`；
  - `TRAJECTA_M4_A4_5_OBSERVE=0|1`；
  - 写出 `A4_5_STAGE_TIMINGS.json`。
- `testdata/M4_A4_5_PERFORMANCE_ATTRIBUTION.schema.json`
- `tools/validate_m4_a4_5_performance.py`
- `tools/summarize_m4_a4_5_attribution.py`
  - 聚合 1/4 worker median；
  - 可选读取 instrumentation-off baseline 并计算观测开销。
- `tools/run_m4_a4_5_perf_profile.py`
- `tools/run_m4_a4_real_matrix.py`
  - 新增 `attribution` phase、重复次数和可选 worker。

## 2. O1 前最小 WSL 真资料结果

冻结场景：ERA5 hybrid、forward、1,000 粒子、1 小时、600 s step/output，artifact 位于 `target/m4-a4.5/`。

| worker | attempt | runner wall time | 结果 |
|---:|---:|---:|---|
| 1 | 1 | 49.991246884 s | Complete |
| 4 | 1 | 49.785906532 s | Complete |
| 4 | 2 | 51.355409322 s | Complete |
| 4 | 3 | 49.658088624 s | Complete |

四次均满足：

- manifest `complete`；
- abnormal termination = 0；
- particle_state rows = 6,701；
- execute-phase reader/provider I/O delta 五项为 0；
- SQLite integrity、mass ledger 和 artifact identity 通过 harness 门禁。

当前 1-worker 只有一个新 schema attempt，不能作为最终缩放统计；它与 4-worker 约 1.00× 的初步结果只说明应优先审计串行段。

## 3. O1 前 primary bottleneck

最完整证据：

`target/m4-a4.5/cells/wsl__era5-hybrid__forward__p1000__w4/attempt-3/.../A4_5_STAGE_TIMINGS.json`

该 artifact 已通过 Draft 2020-12 schema validator。

| 阶段 | 时间 | runner 占比 |
|---|---:|---:|
| runner total | 49.658088624 s | 100% |
| output finish | 37.750285858 s | 76.02% |
| provenance bundle total | 37.238103530 s | 74.99% |
| provenance stream write | 37.067613675 s | 74.65% |
| external sort | 0.089859200 s | 0.18% |
| semantic validation | 0.048081041 s | 0.10% |
| final hash | 0.021153224 s | 0.04% |
| cleanup | 0.009941519 s | 0.02% |

这排除了“排序、完整文件校验或最终 SHA 是 37 秒主因”的解释。boundary、插值和 cache 仍有优化空间，但不是当前 50 秒运行的 primary bottleneck。

## 4. O1 前 37 秒的具体机制

静态调用链：

```text
ProvenanceBundleBuilder::finalize_after_sqlite
  -> stream_write_bundle
    -> lockstep_write_samples
      -> write_json_pretty_value
        -> DigestingFile::write_all
          -> std::fs::File::write_all
```

审计结论：

1. `DigestingFile` 直接持有未缓冲的 `File`，每次 token/缩进/字段都直接调用底层 `write_all`；
2. `lockstep_write_samples` 对每条样本同步读取 SQLite 和排序 spool，校验 field-set/quality，再 `serde_json::to_value`；
3. 三字段 sample assignment 的 pretty JSON 路径每条至少约 13 次小写调用，6,701 条样本仅 sample 区就约 87,112 次，records/field_sets 还会继续增加；
4. 最终 bundle 只有 1,455,142 bytes，却用 37.068 s 写出；
5. WSL runner 从 `/mnt/e/flexpart/trajecta` 运行，artifact 同样落在 `/mnt/e`，大量小写会跨 WSL/Windows 挂载文件系统边界。

因此当前 primary hypothesis 是：**未缓冲 pretty-JSON 小写调用被 drvfs/NTFS 边界放大**。BTreeMap lookup、slot validation 和逐条 `serde_json::to_value` 是可能的次级 CPU 成本，但无法单独解释 1.46 MB 文件的 37 秒墙钟。

## 5. O1：provenance 有界缓冲写

`DigestingFile` 已由裸 `File` 改为 128 KiB `BufWriter<File>`；`sync_all()` 先 flush，再对底层文件执行 `sync_all()`。JSON 字节、字段顺序、SHA、原子替换和失败语义保持不变，并新增跨 buffer 边界的字节/SHA 单测。

B 的冻结执行证据确认：

- instrumentation-on：3× w1 + 3× w4 全部 `Complete`；
- instrumentation-off w4 baseline：3/3 `Complete`；
- 9/9 abnormal=0、execute I/O delta 五项为 0；
- 三类 normalized digest 在九次运行中完全一致；
- profiler 因 WSL 无 `perf` executable 诚实标为 `external_blocked`；
- 观测开销约 0.37%，满足不超过 3% 的门槛。

O1 结果：

| 指标 | O1 前 | O1 后 | 变化 |
|---|---:|---:|---:|
| w4 runner median | 48.976 s | 12.749 s | 3.84× faster |
| provenance stream write | 36.758 s | 0.183 s | 约 201× faster |

O1b“继续改序列化”被否决：O1 后 stream write 只占 runner 约 1.43%，继续重写 JSON 路径的风险高于潜在收益。

证据：

- `target/m4-a4.5/o1-matrix/`
- `target/m4-a4.5/o1-baseline/`
- `target/m4-a4.5/o1-matrix/summary/M4_A4_5_PERFORMANCE_ATTRIBUTION.json`

## 6. O2：跨 boundary path 的 exact stencil 复用

### 6.1 归因

O1 后 w4 中位数中：

- `runner_boundary`：3.652 s；
- `boundary_stencil_prepare`：2.553 s；
- `column_stencil_build`：2.660 s；
- `column_cache_lock_hold`：2.380 s；
- `column_cache_lock_wait`：仅约 0.002 s。

因此 secondary bottleneck 不是线程锁等待或 root recursion，而是同一 exact stencil 在相邻粒子路径间被反复重建。旧 `finalize_boundary_stencil_session()` 把全局 cache 裁到当前路径所用 stencil 大小，主动淘汰了仍在硬预算内、可由下一路径复用的 unpinned entry；同时 `ColumnCache::insert()` 在容量充足时仍遍历并排序全部 eviction candidates。

### 6.2 实现

- `ColumnCache::insert()` 仅在 `resident + new_entry > hard_budget` 时构造和排序 eviction candidates；
- boundary finalize 的 trim target 改为 `execution_budget - current_non_cache_scratch`；
- 当前路径 pin、确定性 LRU、frame/cache identity 和硬字节预算保持不变；
- 预算收紧时仍只淘汰 unpinned entry，无法满足时保持原 hard fail；
- 新增容量内无 eviction、预算内保留旧 stencil、预算收紧后淘汰旧 stencil 的单测。

### 6.3 3× WSL 真资料结果

冻结场景仍为 ERA5 hybrid / forward / 1,000 粒子 / w4 / 1 小时。

| attempt | runner | boundary | stencil build | hits / misses / evictions |
|---:|---:|---:|---:|---:|
| 1 | 11.314 s | 1.876 s | 0.616 s | 76,469 / 8,656 / 7,400 |
| 2 | 10.719 s | 1.684 s | 0.572 s | 76,469 / 8,656 / 7,400 |
| 3 | 10.424 s | 1.701 s | 0.556 s | 76,469 / 8,656 / 7,400 |

中位数对 O1：

| 指标 | O1 | O2 | 变化 |
|---|---:|---:|---:|
| runner | 12.749 s | 10.719 s | −15.92% |
| runner boundary | 3.652 s | 1.701 s | −53.41% |
| column stencil build | 2.660 s | 0.572 s | −78.49% |
| cache misses | 56,296 | 8,656 | −84.62% |
| cache evictions | 55,040 | 7,400 | −86.55% |

O2 冻结验收目标“boundary 中位至少下降 30%”已满足。3/3 均为 `Complete`、abnormal=0、execute I/O delta=0；content、normalized SQL 和 canonical-output digest 均与 O1 bit-identical。最终 cache resident bytes 仍为 23,994,624；尚无 peak RSS 证据，因此不能据此宣称正式大规模内存门禁已通过。

证据：

- `target/m4-a4.5/o2-smoke/`
- `target/m4-a4.5/o2-matrix/`

O1+O2 合计把同一 w4 场景的中位 runner 从 48.976 s 降至 10.719 s，约 4.57×；这不外推为 50k/100k 的线性承诺。

## 7. 已运行门禁

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo test --offline -p trajecta-cli --test public_module_contracts
git diff --check
  -> passed
```

公开模块 inventory 已由 93 更新为 95，并为 `performance` 与 `query::metrics` 补齐强制 `//! # Contract:` 首行。

## 8. 当前热点与尚未闭合

- profiler 仍为 `external_blocked`，没有 inclusive/self CPU top-20 或 flamegraph；
- O2 后 top-level primary 为 `runner_output` 中位约 6.038 s（56.33%），其中 output sink 与 meteorology query execute 是主要子项；
- `runner_integrator` 中位约 2.164 s（20.19%），`runner_boundary` 已降至约 1.701 s（15.87%）；
- general transport `prepare_stencils()` 仍按当前 batch 裁剪 cache，是剩余 eviction 的已知来源之一，但本轮不扩大 O2 范围；
- M4-A4 正式 50k/100k、RSS、SQLite size、writer concurrency、scaling 和最终签署。

本报告不宣称 M4-A4.5、M4-A4 或 M4 完成；未 commit、未 push，未修改 `E:\flexpart\origo-validation-v1.json`。
