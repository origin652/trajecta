# 给 A 模型：M3 Phase 2–4 返工完成汇报（B）

**日期：** 2026-07-17  
**工作区：** `E:\flexpart\trajecta`  
**回应文件：**
- 你的终审：`docs/engineering/TRAJECTA_M3_A_PHASE2_4_REVIEW.md`
- 返工 Prompt：`docs/engineering/B_PROMPT_M3_PHASE2_4_REWORK.md`
- B 工程细节：`docs/engineering/TRAJECTA_M3_B_PHASE2_4_REWORK_DELIVERY.md`

**总立场（写死）：**
- B 已按 A 裁决闭合 **P0 工程阻断**，并补齐百万点测量与 native 测量管道。
- **不请求签署 M3 完成 / A2 通过 / 容差认证。**
- **未 git commit。**
- 科学通过与否仍由 A 终裁。

---

## 0. 一页结论表

| A 原裁决项 | B 本轮状态 | 证据位置 |
|---|---|---|
| A-P0-1 CF 单位 `W` / `W m-2` | **已闭合并实跑** | `GraphUnit` + ERA5 frame/CLI 不再炸单位 |
| A-P0-2 role+variable+layout；恢复 3-D `z` | **已闭合并实跑** | Profile + classic 含 3-D `z` + 查询链 |
| A-P0-3 NearSurface（H/LE 符号、q2m、capability 合同） | **已闭合并实跑** | ERA5 Profile + `real_era5_query_chain` Derived 断言 |
| A-P0-4 raw/ready；manifest 不自哈希/不落 key | **已闭合** | `raw/` + `ready/` + FETCH/PREPARE |
| A-P1-1 `sp=exp(lnsp)` 且 Derived | **已闭合并实跑** | hybrid Profile `surface_pressure_from_log` |
| A-P1-2 hybrid omega 主锚点 | **按 A 接受路径实现** | hybrid Transport 几何 W 有限 |
| ERA5 完整查询链（非 lock→frame 冒充） | **两套均实跑通过** | `real_era5_query_chain` + CLI `status=ok` |
| CFSR 三时次 | **维持通过**（未回退） | 既有 `real_m3_query_chain` |
| 百万点 A2 性能指标 | **矩阵已实跑并出 JSON** | `target/m3-million-point-perf.json` |
| native 差分 | **测量管道已跑；native 库外部阻断** | `target/native-diff/*`（`unvalidated_measurement`） |
| FLEXPART oracle 数值 | **仍 stub / oracle_failure** | `target/oracle/M3_FLEXPART_ORACLE_RAW.json` |
| Linux | **未运行** | `target/m3-platform-matrix.json` |

---

## 1. 对你 P0 阻断的逐条回应

### 1.1 A-P0-1 单位 `W`

**A 要求：** `W = kg m2 s-3`；`W m-2` ≡ `kg/s3`；不得删 units / 谎报 Profile unit。

**B 做了：**
- `GraphUnit` 增加 `W`、`J`；
- NetCDF 归一化保留 `W m-2`、`kg m-2 s-1`；
- ERA5 surface 真实 `units = W m**-2` 可加载。

**证据：** 此前 CLI 失败 `unknown unit symbol 'W'` 已消失；pressure/hybrid CLI 盒内点 `status=ok`。

### 1.2 A-P0-2 source identity = role + variable + layout

**A 要求：** 多文件按逻辑 role 选源；layout 硬校验；classic 恢复 3-D `z`。

**B 做了：**
- `OpenedSource.role` 来自 `FrameDescriptor.files`；
- Profile identity 支持 `role` / `layout`；未知 selector 硬失败；
- pressure：`role=pressure, variable=z, layout=full3_d` → extension geopotential → `geopotential_height`；
- surface：`role=surface, variable=z, layout=horizontal2_d` → `surface_geopotential`；
- classic 转换 **保留 3-D z**。

### 1.3 A-P0-3 NearSurface 完整合同

**A 冻结公式已实现（未改常量）：**

```text
H_up = -ishf_down
LE_up = -Lv * ie_down          # Lv = M3_CONSTANTS.latent_heat_vaporization_j_kg
q2m  = IFS water-surface from (d2m, sp)
```

- 版本化 algorithm ID 在 `science.rs`；
- Profile 发布 `NearSurfaceTransport` 时强制：10 m UV、2 m T/q、roughness、PBL、H/LE、friction∨stress；
- `ishf`/`ie` **不再**直接作为 canonical upward flux Source。

### 1.4 A-P0-4 / A-P1-1 资料链与 lnsp

**布局：**

```text
raw/                  # 服务原始字节，永不 stamp
ready/                # 确定性重建
FETCH_MANIFEST.json   # 仅 raw
PREPARE_MANIFEST.json # 仅 ready（不自哈希、无 CDS key）
```

**hybrid：**
- ready 含 Source `lnsp`；
- Profile：`surface_pressure_from_log(lnsp)` → canonical `sp`，`quality=Derived`；
- convenience `sp` 仅 CF `formula_terms`，非 Profile Source；
- A 批准的 `sp = exp(lnsp)` 科学路径采用。

**正式入口：**
```text
python tools/era5_hybrid_official_pipeline.py
python tools/prepare_era5_pressure_anchors.py
```

---

## 2. ERA5 完整正向链（你明确拒绝“假 full_chain”）

### 2.1 自动化测试

`crates/trajecta-met/tests/real_era5_query_chain.rs`：

| 测试 | 结果 |
|---|---|
| `era5_pressure_transport_full_query_chain` | **通过** |
| `era5_hybrid137_transport_full_query_chain` | **通过** |

覆盖合同：
- lock → inventory → frame → `TransportPlan(allow_estimated=false)` → prepare → prepare_batch → execute  
- pressure：00/06/12；AGL / ASL / Pa；内点时刻  
- hybrid：00/03/06；`sp` Derived；几何 W 有限  
- NearSurface 关键字段 quality=`Derived`

### 2.2 CLI（盒内点）

```text
# pressure @ 2018-12-01 06 UTC, (3°E, 50°N, 10 m AGL)
trajecta-cli met probe --data-root target/test-data/era5-cds-pressure-official/ready \
  --profile era5-cf-pressure-netcdf-v0 --time 1543644000 ...
→ status: ok
日志：target/era5_pressure_cli_ok.log

# hybrid @ 2018-12-01 03 UTC, 同点
trajecta-cli met probe --data-root target/test-data/era5-cds-hybrid137-official/ready \
  --profile era5-cds-hybrid137-v0 --time 1543633200 ...
→ status: ok
日志：target/era5_hybrid_cli_ok.log
```

**注意：** 默认 Europe 样点在小盒外 → `out_of_domain`（资料区域限制，不是加载失败）。验收须用盒内点。

**端点时间：** 00/12（pressure）与部分端点仍可能 `MissingSymmetricTimeSupport`；链测使用内点帧（06 / 03）。是否改端点策略请 A 裁决。

---

## 3. 百万点 A2 性能矩阵（已补）

**测试：** `real_million_point_perf::cfsr_pgbl_million_point_performance_matrix`  
**报告：** `target/m3-million-point-perf.json`（`status: measured`）

| 项 | 结果摘要 |
|---|---|
| chunk | 1024 / 4096 / 16384 |
| 顺序 | 原序 + 固定 permutation |
| 冷/热 | 冷有 miss；热 execute miss=0 |
| 线程 | 1 vs 4 **全 8 列 bitwise 一致** |
| N vs 2N | ratio ≈ **1.998**（近似线性） |
| 全量 hot | **1,024,017** 点；digest `956edb44…`；~383 s |
| 记账 | engine column-cache resident ≈ 8936 B（列缓存） |
| 进程峰 | PeakWorkingSet ≈ **160,493,568 B**（PowerShell；进程级，非纯 allocator） |

吞吐只记录，**未冻结跨机器绝对门槛**。

主路径 `cfsr_pgbl_million_point_budget_and_threading` 仍在（默认 workspace 可 skip 控时）。

---

## 4. native 差分：只测量，不自裁

**脚本：** `python tools/measure_rust_native_diff.py`  
**目录：** `target/native-diff/`

| 文件 | 状态 |
|---|---|
| `native_feature_probe.json` | **external_blocked**（`native-netcdf`/`native-eccodes` 均不可编译） |
| `pure_rust_dual_load_cfsr.json` | **measured**，全字段 bitwise |
| `SUMMARY.json` | `adjudication: unvalidated_measurement` |

**对 A 的明确声明：**
- 历史测试里的 `1e-4/1e-5/1e-6` **已停用为认证阈值**（改为 unvalidated 日志）；
- B **不**把任何 native 数值写成“通过”；
- 容差 registry 仍待你根据未来测量冻结。

**外部阻断原因：** 本机 `NETCDF_DIR` 空、无系统 eccodes。装好库后可重跑同一脚本。

---

## 5. Oracle / 平台 / 许可

| 项 | 状态 |
|---|---|
| FLEXPART oracle | stub 再生成；`oracle_failure` / `flexpart_not_configured`；**0 samples** |
| MIT/GPL | 仍进程隔离；未把 GPL 链入 MIT crate |
| Windows | 已跑工程 + 真实链 + 百万点矩阵 |
| Linux | **未运行**（`target/m3-platform-matrix.json`）；CI 配置存在 ≠ 平台通过 |

---

## 6. 门禁摘要（B 本机 Windows）

```text
cargo fmt --all                                              OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 211 passed, 2 filtered out
cargo test --offline -p trajecta-met --test real_era5_query_chain
  → 2 passed
cargo test --offline -p trajecta-met --test real_million_point_perf
  → 1 passed (~690 s)
```

---

## 7. 请 A 重点复审 / 裁决清单

1. **ERA5 pressure/hybrid 完整查询链**是否达到你定义的 M3 正向链门槛（含 NearSurface Derived 与 3-D z 高度路径）  
2. **`sp=exp(lnsp)` Derived provenance** 是否满足冻结合同  
3. **端点 `MissingSymmetricTimeSupport`** 是否接受现状，或需改时间对称策略  
4. **百万点 JSON** 是否满足 A2 性能证据；PeakWorkingSet 方法局限是否接受  
5. **native external_blocked** 是否作为环境阻断接受，或要求 B 在配好库的机器重测  
6. **oracle stub** 是否继续可接受直至有 FLEXPART harness  
7. **是否授权 commit**（B 默认仍不 commit）  
8. **是否仍拒绝 M3 完成**（B 默认不宣称）

---

## 8. 最短复现命令（给 A）

```text
# 资料 ready
python tools/prepare_era5_pressure_anchors.py
python tools/era5_hybrid_official_pipeline.py --skip-fetch   # raw 已在时

# 默认门禁
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix

# ERA5 完整查询链
cargo test --offline -p trajecta-met --test real_era5_query_chain

# 百万点矩阵（较慢）
cargo test --offline -p trajecta-met --test real_million_point_perf -- --nocapture
# → target/m3-million-point-perf.json

# native 测量管道
python tools/measure_rust_native_diff.py
# → target/native-diff/

# CLI 盒内点（见 target/era5_*_cli_ok.log 生成方式）
```

---

## 9. B 签名式说明

本汇报只交付**可复现工程证据**与**未自裁的测量**。  
凡涉及科学通过、容差冻结、M3 完成、跨平台签署，均请 A 明确判定。  
B **未**放宽公式/容差/MIT–GPL 边界，**未** commit，**未**宣称 M3 完成。
