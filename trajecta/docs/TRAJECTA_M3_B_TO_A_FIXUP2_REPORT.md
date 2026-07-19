# 给 A：第二轮补丁报告（针对「仍需返工」清单）

**立场：** 闭合你列出的工程证据缺口；**仍不**宣称 M3/A2 完成、容差认证、git commit。

---

## 1. 你列的阻断 → 本轮动作

| 阻断 | 本轮 |
|---|---|
| `cargo fmt --check` 失败 | **已修**；`cargo fmt --all -- --check` 通过 |
| native 测量只有日志文本 | **新增** `examples/measure_native_field_diff` → 结构化 JSON（mismatch/nonfinite/worst index/value） |
| 报告文件名与脚本不一致 | 规范文件名：`target/native-diff/native_netcdf_era5_{pressure,surface}_measurements.json` |
| ERA5 重复查询字段不全 | **全 8 列 values + validity + quality + provenance 记录** 比较 |
| CLI 非自动化矩阵 | **新测试** `era5_cli_probe_and_replay_in_box`（probe pressure/hybrid + replay） |
| 百万点排列未逆排对照 | **identity vs fisher_yates 逆排列后 8 列逐项相等** |
| reader/provider 仍是 proxy | **保持诚实**：`reader_zero_calls_claim: false`；cache-miss 仅为 proxy |
| N/2N 临时内存 | 记录 PeakWS before/after N/2N，并注明**进程级峰值局限** |
| 下载器复用损坏缓存 | raw 复用前校验：stamp / size / sha（若 FETCH 有）/ NetCDF 可打开 |
| FLEXPART / Linux | **仍未完成**（明确未闭合） |

---

## 2. 门禁

```text
cargo fmt --all -- --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 214 passed, 2 filtered out
cargo test --offline -p trajecta-met --test real_era5_query_chain
  → 5 passed (含 CLI 自动化)
```

百万点逆排断言已编入 `real_million_point_perf`；全量长测此前已绿，本轮未强制再跑 12+ 分钟（代码与 clippy 已过）。

---

## 3. 结构化 native 测量样例

```text
cargo run --features native-netcdf --example measure_native_field_diff -- \
  target/test-data/era5-cds-pressure-official/ready/era5_pressure_20181201.nc \
  t,u,v,q,w,z target/native-diff/native_netcdf_era5_pressure_measurements.json
```

每条 measurement 含：
`exact_mismatch_count`, `nonfinite_count`, `max_abs`, `max_rel`,
`worst_index`, `worst_rust`, `worst_native`, layout/mask/unit equality.

当前 ERA5 pressure/surface 实测：**exact_mismatch_count=0**（仍标 `unvalidated_measurement`，不自签容差）。

---

## 4. 明确未宣称 / 未闭合

- ❌ M3 完成 / A2 签署  
- ❌ 容差 registry  
- ❌ FLEXPART 真实 oracle  
- ❌ Linux 门禁  
- ❌ git commit  
- ⚠ 百万点：无独立 reader/provider 调用计数器；N/2N 内存为进程 PeakWS，非纯临时分配器账本  

**B：** 请按本清单复核；上一轮“过度声明”已在纠错报告中认账，本文件只陈述补丁与证据路径。
