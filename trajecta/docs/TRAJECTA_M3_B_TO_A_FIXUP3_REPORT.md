# 给 A：第三轮补丁报告（针对 fixup2 剩余工程验收缺口）

**立场：** 闭合你列出的剩余工程缺口；**仍不**宣称 M3/A2 完成、容差认证、git commit。

---

## 1. 阻断 → 本轮

| 你列的缺口 | 本轮 |
|---|---|
| `measure_rust_native_diff.py` L272 `SyntaxError` | **已修**；`ast.parse` 通过；主入口可运行 |
| `max_rel` 只跟 `max_abs` | **已修**：`max_rel` 独立扫全场；另报 `worst_rel_*`；`worst_index` 仍为 abs 别名 |
| 缺 hybrid / 近地完整 / CFSR ecCodes 结构化测量 | **已补**（见 §3） |
| CLI 依赖预存二进制、可能软跳过；hybrid 无 replay | **`ensure_trajecta_cli_binary`** 自动 `cargo build -p trajecta-cli`；fixture 在则硬跑；pressure+hybrid 均 **probe+replay** |
| `.part` 未校验即替换；manifest 缺失弱复用 | **`promote_part` 先验后替**；manifest 损坏直接 `SystemExit`；无 size/sha **拒绝弱复用**（除非 `TRAJECTA_ALLOW_UNVERIFIED_RAW=1`） |
| 百万点逆排未真正执行 | **已重跑长测** `real_million_point_perf`：**passed (726s)**；JSON `inverse_permutation_checked: true` |
| reader 计数 / Linux / FLEXPART | **仍未完成**（诚实保持） |

---

## 2. 门禁

```text
cargo fmt --all -- --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 214 passed, 2 filtered out
cargo test --offline -p trajecta-met --test real_era5_query_chain
  → 5 passed（含 CLI build+probe/replay pressure+hybrid）
cargo test --offline -p trajecta-met --test real_million_point_perf
  → 1 passed（含 inverse-permute 8 列对照，~12 min）
python -c "ast.parse(measure + both fetch scripts)" → OK
```

---

## 3. 结构化 native 测量文件

路径：`target/native-diff/`

| 文件 | 内容 | 本机结果摘要 |
|---|---|---|
| `native_netcdf_era5_pressure_measurements.json` | t,u,v,q,w,z | n=18，exact_mm=0，nonfinite=0 |
| `native_netcdf_era5_surface_measurements.json` | sp,z,u10,v10,t2m,**d2m,ishf,ie,blh,fsr,zust** | n=33，mm=0 |
| `native_netcdf_era5_hybrid_measurements.json` | t,u,v,q,w,sp（hybrid 3D/2D 可 decode 场） | n=18，mm=0 |
| `native_netcdf_era5_hybrid_surface_measurements.json` | 近地完整集 | n=30，mm=0 |
| `native_eccodes_cfsr_pgbl_measurements.json` | CFSR `discipline.category.number:level` | n=7；部分场 **非零 exact_mm（1e-18 量级）**，已如实记录，**不自签通过** |

每条 measurement 含：`exact_mismatch_count`, `nonfinite_count`, `max_abs`, **独立** `max_rel`, `worst_abs_*`, `worst_rel_*`（及 abs 别名 `worst_index/rust/native`）。

主入口：

```text
python tools/measure_rust_native_diff.py
```

（内部调用 example `measure_native_field_diff`；`--format netcdf|grib`）

**说明：** hybrid 的 `ap`/`b`/`lnsp` 是 PV/1D 坐标，不走 CF 3D field decode，故不纳入 dual-field 测量变量表（避免假 error 噪声）。

---

## 4. 下载器契约（raw）

`tools/fetch_era5_pressure_cds.py`、`tools/era5_hybrid_official_pipeline.py`：

1. 复用：必须 FETCH_MANIFEST **size+sha** 命中且 `file_usable`；否则拒绝弱复用。  
2. 下载：写入 `*.part` → **`promote_part` 校验**（size/sha 或至少非空+无 stamp+NetCDF 可开）→ 再 `replace`。  
3. FETCH_MANIFEST 损坏 / entry 缺 size|sha → **硬失败**，不静默降级。  
4. 无 manifest 时期望：仅当 `TRAJECTA_ALLOW_UNVERIFIED_RAW=1` 才允许未校验复用（会打印 UNVERIFIED）。

---

## 5. 百万点证据

`target/m3-million-point-perf.json`：

- `scaling.inverse_permutation_checked: true`（identity vs fisher_yates 逆排后 8 列逐项相等，**本轮实测通过**）
- `reader_zero_calls_claim: false`
- `execute_cache_note`: cache-miss=0 仅为 proxy
- `temp_memory_note`: PeakWS 进程级局限

---

## 6. 明确未宣称 / 未闭合

- ❌ M3 完成 / A2 签署  
- ❌ 容差 registry / 对 CFSR 1e-18 mismatch 的“通过”判定  
- ❌ 独立 reader/provider 调用计数器  
- ❌ FLEXPART 真实 oracle  
- ❌ Linux 门禁  
- ❌ git commit  

**B：** 请按本清单复核；上一轮语法/max_rel/覆盖/CLI/下载/逆排未跑 等问题本轮已对症处理。
