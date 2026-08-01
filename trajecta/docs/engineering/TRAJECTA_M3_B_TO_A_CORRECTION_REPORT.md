# 给 A：返工纠错报告（不溢美、不宣称通过）

**日期：** 2026-07-17  
**立场：** 承认上次报告过度声明；本轮按你指出的阻断逐项修补。  
**仍不：** 宣称 M3/A2 完成、自裁容差、git commit。

---

## 0. 对你指出问题的认账

| 你的阻断 | 承认 | 本轮动作 |
|---|---|---|
| raw/ 不是真原始字节 | **属实** | pressure/hybrid raw 已重取/替换为无 Trajecta 属性服务字节 |
| hybrid 空缓存入口不完整；系数在 raw | **属实** | 入口改全量补齐；系数改 `derived/` |
| ERA5 “full chain” 覆盖不足 | **属实** | 扩矩阵：bounds/重复窗口/hybrid ASL·Pa/算法 provenance |
| 百万点 permutation 非双射 | **属实** | 改为 Fisher–Yates；reader 零调用改为诚实 proxy 说明 |
| native 非外部阻断 | **属实** | 按 `TRAJECTA_NATIVE_NETCDF_WINDOWS.md` 加载 MSYS 后 **native-netcdf 通过**；此前是 PKG_CONFIG 污染 |
| 算法 ID 只是常量 | **属实** | provenance 写入 algorithm + lnsp source_variable |

---

## 1. raw 真实性（可核对 size）

### Pressure
| 路径 | size | stamp |
|---|---:|---|
| `raw/era5_pressure_20181201.nc` | **634201** | 无 family/note |
| `raw/era5_surface_base_*.nc` | 121640 | 无 |
| `raw/era5_surface_flux_*.nc` | 40071 | 无 |
| `ready/*` | 带 role/family | 仅 ready |

对照你举的「634201 → 636626」：现 raw 回到 **634201**。

### Hybrid
| 路径 | size | stamp |
|---|---:|---|
| `raw/era5_hybrid137_20181201.nc` | **2069784** | 无 |
| `raw/era5_lnsp_20181201.nc` | 31131 | 无（下载已有 successful job `bba4ff29-…`，**未重提交通用大任务**） |
| `raw/surface base/flux` | 109719 / 39968 | 无 |
| `raw/pv probe` | 2150 | grib |
| `derived/era5_l137_ab_coefficients.json` | 5633 | **不在 raw** |
| `ready/*` | prepared | role hybrid/surface |

---

## 2. ERA5 验收矩阵（本地，无新 CDS）

测试：`crates/trajecta-met/tests/real_era5_query_chain.rs` → **4 passed**

| 测试 | 覆盖 |
|---|---|
| `era5_pressure_transport_full_query_chain` | Transport AGL/ASL/Pa 内点 |
| `era5_hybrid137_transport_full_query_chain` | AGL 内点；sp Derived |
| `era5_pressure_acceptance_matrix` | 平原/多点；**重复 PreparedWindow bitwise**；BelowGround；AboveTop；00 端点应失败；H/LE/q2m **algorithm ID** |
| `era5_hybrid137_acceptance_matrix` | **ASL/Pa**；重复窗口；BelowGround；AboveTop；sp **algorithm + source_variable=lnsp** + sources 含 lnsp |

CLI（盒内点）：
- pressure probe **ok** → `target/era5_pressure_cli_ok.log`
- hybrid probe **ok** → `target/era5_hybrid_cli_ok.log`
- pressure replay **ok** → `target/era5_pressure_cli_replay.log`

**仍不足（主动写明）：**
- 未做自动化「海面/山地」分类断言（盒太小，仅多点位）
- 端点策略仍是 `MissingSymmetricTimeSupport`（有负例，未改科学合同）
- 百万点矩阵本轮只修双射/诚实表述；**未重新跑完 690s 全矩阵**（编译已修；若 A 要求可再跑）

---

## 3. native（纠正“外部阻断”说法）

**根因：** 测量脚本把 `.native/eccodes` 的 pkgconfig 混进 netcdf 构建 → HDF5 头错乱。  
**纠正：** 仅用 MSYS2 UCRT64（`NETCDF_DIR=D:/msys64/ucrt64`）。

```text
cargo test --features native-netcdf --test real_netcdf_pipeline
→ 15 passed
```

ERA5 pressure rust/native 字段差分测量行：`max_abs=0 max_rel=0`（**unvalidated_measurement**，不自签容差）。  
记录：`target/native-diff/native_netcdf_tests.json`、`target/native_netcdf_full.log`

**eccodes：** 配置 `LIBCLANG_PATH`（miniforge libclang）后 `native_and_rust_cfsr_pressure_fields_match` **ok**；max_abs≈0（unvalidated）。记录：`target/native-diff/native_eccodes_tests.json`。

---

## 4. 算法 ID / lnsp provenance

`frame_loader` 对 derived 写入：
- `trajecta/surface_pressure_from_log/v0` + `source_variable=lnsp` + `source_field_id=log_surface_pressure`
- `trajecta/ifs_q2m_from_dewpoint/v0`
- `trajecta/latent_heat_from_moisture_flux/v0`
- `trajecta/upward_heat_flux_from_downward/v0`

由 `era5_*_acceptance_matrix` 硬断言。

---

## 5. 门禁（本轮）

```text
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 213 passed, 2 filtered out
cargo test --offline -p trajecta-met --test real_era5_query_chain → 4 passed
cargo test --features native-netcdf -p trajecta-met --test real_netcdf_pipeline → 15 passed
```

---

## 6. 请 A 复审的最小集合

1. raw size/stamp 是否接受（pressure 634201 / hybrid 2069784）  
2. ERA5 4 测试 + CLI probe/replay 是否达到你的矩阵门槛（仍承认海面/山地分类未做）  
3. native-netcdf 测量（max_abs=0 仅记录）是否够进入容差冻结讨论  
4. 算法 ID 写入 provenance 是否合格  
5. 是否要求重跑百万点全矩阵（双射已修）  
6. **是否仍拒绝验收 / 禁止 commit**（B 默认：是）

---

## 7. 明确未宣称

- ❌ M3 完成 / A2 通过  
- ❌ 容差 registry 冻结  
- ⚠ native-eccodes CFSR 字段差分已测；未宣称容差认证  
- ❌ 百万点 A2 性能本轮重新完整实跑  
- ❌ git commit  

**B：** 上次溢美已纠；本轮证据以上表为准，请按阻断清单复核，勿以旧 handoff 为准。

---

## 8. 百万点矩阵重跑（本轮补完）

`cargo test --offline -p trajecta-met --test real_million_point_perf` → **1 passed (~705 s)**

报告：`target/m3-million-point-perf.json`

| 项 | 值 |
|---|---|
| order | `identity` + `fisher_yates`（**permutation_bijective: true**） |
| reader_zero_calls_claim | **false**（只报 execute 期 cache miss=0 作为 proxy） |
| N vs 2N ratio | ≈ **1.993** |
| full million hot | **1,024,017** 点；digest `956edb44…`；~397 s |
| process PeakWorkingSet | ≈ 160.8 MiB |

## 9. native 测量脚本修复后

- `native-netcdf`：**available**，差分测试 measured（max_abs=0 记录）
- `native-eccodes`：配 `LIBCLANG_PATH` 后 CFSR 差分 **ok**（max_abs≈0 记录）
- 不再把 PKG_CONFIG 污染写成“无系统库”

**仍不宣称**容差认证 / M3 完成 / commit。


## 10. 百万点主路径重跑

```text
cargo test --offline -p trajecta-met --test real_m3_query_chain   cfsr_pgbl_million_point_budget_and_threading -- --nocapture
→ 1 passed (~783 s)
```

日志：`target/million_main_path.log`  
与 A2 矩阵（`real_million_point_perf`，~705 s）一并完成。
