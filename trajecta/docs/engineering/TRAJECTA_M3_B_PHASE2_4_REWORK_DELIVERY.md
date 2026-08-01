# B 返工交付报告（A Phase 2–4 终审后）

**日期：** 2026-07-17  
**工作区：** `E:\flexpart\trajecta`  
**依据：** `docs/engineering/TRAJECTA_M3_A_PHASE2_4_REVIEW.md` + `docs/engineering/B_PROMPT_M3_PHASE2_4_REWORK.md`
**结论立场：** 阻断项已工程闭合并有真实链证据；**不宣称 M3 完成 / A2 通过 / 容差认证**；**未 git commit**。

---

## 1. 已实现且实际运行

### 1.1 P0-1 CF 单位 `W`

- `GraphUnit` 增加 SI 派生单位 **`W = kg m2 s-3`**、`J`。
- `W m-2` / `W/m2` / 归一化后的 `W m**-2` 与 canonical `kg/s3` 同维度（`ENERGY_FLUX`）。
- 单测：`compound_units_reduce_to_fundamental_dimensions` 覆盖正/等价/非法。
- NetCDF `normalize_cf_unit_string` 保留 `W m-2` 与 `kg m-2 s-1`。

### 1.2 P0-2 role + variable + layout 身份

- `OpenedSource` 保留 `FrameDescriptor.files` 的逻辑 role。
- Profile identity：`role` 过滤成员；`layout` decode 后硬校验；未知 selector 硬失败。
- ERA5 pressure：`pressure` 角色 3-D `z` → extension geopotential → `geopotential_height`；`surface` 角色 2-D `z` → `surface_geopotential`。
- classic 转换**恢复 3-D `z`**（删除“为消歧删 z”）。

### 1.3 P0-3 NearSurface 完整化

A 冻结公式实现：

| 项 | 实现 |
|---|---|
| `H_up = -ishf` | derived `-downward_sensible_heat_flux` |
| `LE_up = -Lv * ie` | `latent_heat_from_moisture_flux`（Lv=`M3_CONSTANTS`） |
| `q2m` from `d2m+sp` | IFS 水面饱和 + `two_metre_specific_humidity_from_dewpoint` |
| 符号/算法 ID | `science.rs` 版本化常量 |
| Capability 合同 | Profile 发布 `NearSurfaceTransport` 时强制 10m UV、2m T/q、roughness、PBL、H/LE、friction 或 stress |

### 1.4 hybrid `lnsp` provenance（A 批准 `sp=exp(lnsp)`）

- ready hybrid 含 **Source `lnsp`**；Profile 经 `surface_pressure_from_log` 生成 canonical `sp`，`quality=Derived`。
- convenience `sp` 仅服务 CF `formula_terms`，**不是** Profile Source。
- prepare 校验 time/lat/lon **逐值**一致。

### 1.5 raw / ready 资料链

两套 ERA5：

```text
raw/                  # 服务原始字节，只读
ready/                # 确定性重建
FETCH_MANIFEST.json   # 仅 raw（无自哈希、无 CDS key）
PREPARE_MANIFEST.json # 仅 ready
```

- pressure：`python tools/prepare_era5_pressure_anchors.py`
- hybrid：`python tools/prepare_era5_hybrid_anchors.py`

### 1.6 真实 ERA5 完整查询链（非仅 lock→frame）

新测试：`crates/trajecta-met/tests/real_era5_query_chain.rs`

| 测试 | 结果 |
|---|---|
| `era5_pressure_transport_full_query_chain` | **通过**（00/06/12 lock→frame→TransportPlan→prepare→execute；AGL/ASL/Pa；NearSurface Derived 字段） |
| `era5_hybrid137_transport_full_query_chain` | **通过**（00/03/06；sp Derived from lnsp；omega 几何 W 有限） |

### 1.7 CLI 正向（in-box 点）

```text
trajecta-cli met probe --data-root …/era5-cds-pressure-official/ready \
  --profile era5-cf-pressure-netcdf-v0 --time 1543644000 … --points … 
→ status: ok  (log: target/era5_pressure_cli_ok.log)

trajecta-cli met probe --data-root …/era5-cds-hybrid137-official/ready \
  --profile era5-cds-hybrid137-v0 --time 1543633200 …
→ status: ok  (log: target/era5_hybrid_cli_ok.log)
```

注：默认 Europe 样点落在小盒外 → `out_of_domain`（预期）；正式 probe 使用盒内点 (3°E, 50°N)。

### 1.8 门禁（本轮）

```text
cargo fmt --all                                              OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point
  → 211 passed, 1 filtered out
cargo test --offline -p trajecta-met --test real_era5_query_chain
  → 2 passed
```

---

## 2. 已实现但未运行 / 仅部分

| 项 | 状态 |
|---|---|
| 百万点性能终审矩阵（多 chunk / 排列 / RSS / N vs 2N） | **未在本轮补齐**（主路径此前通过） |
| native-netcdf / native-eccodes 全字段差分测量 JSON | **未在本轮系统跑**（环境 `NETCDF_DIR` 等） |
| Linux 矩阵 | **未运行** |
| 真实 FLEXPART 数值 oracle | 仍 stub `oracle_failure` |
| 从完全空缓存 clean fetch 的 hybrid 一键入口 | prepare 已统一；fetch 仍依赖既有脚本/CDS |

---

## 3. 外部阻断

- Windows 本机 native-netcdf 构建需系统 NetCDF 开发库。
- 无 FLEXPART 二进制/case harness → oracle 不能产出数值 samples。
- CDS 长任务历史阻塞已用 raw 落盘绕过；新账号 job 已成功。

---

## 4. A 仍需裁决 / 知晓

1. **端点时间** `MissingSymmetricTimeSupport`：压力 00/12 与 hybrid 00/06 端点仍按对称时间策略拒绝；完整链测使用内点帧（06 或 03）。是否需要端点策略变更由 A 定。
2. **native 容差 registry**：B 未放宽、未自裁；待差分测量后 A 冻结。
3. **百万点 A2 性能指标**：主路径在；完整性能矩阵待补。
4. **CLI 默认样点** 不在小盒内——工程如此；验收须用盒内点或扩大区域资料。

---

## 5. 明确未宣称

- ❌ M3 完成  
- ❌ A2 签署 / 容差认证  
- ❌ native 数值通过  
- ❌ FLEXPART oracle 数值通过  
- ❌ git commit  

---

## 6. 关键路径

| 路径 | 说明 |
|---|---|
| `crates/trajecta-met/src/profile/graph.rs` | W/J 单位；新热力学 op 类型 |
| `crates/trajecta-met/src/science.rs` | q2m / Lv 算法 ID 与公式 |
| `crates/trajecta-met/src/frame/mod.rs` | 热力学执行 |
| `crates/trajecta-met/src/io/frame_loader.rs` | role/layout 选择 |
| `crates/trajecta-met/src/profile/document.rs` | NearSurface 合同校验 |
| `crates/trajecta-met/profiles/era5_cf_pressure_netcdf_v0.yaml` | 完整 pressure Profile |
| `crates/trajecta-met/profiles/era5_cds_hybrid137_v0.yaml` | 完整 hybrid Profile |
| `tools/prepare_era5_pressure_anchors.py` | raw/ready |
| `tools/prepare_era5_hybrid_anchors.py` | raw/ready + lnsp |
| `crates/trajecta-met/tests/real_era5_query_chain.rs` | 真实完整查询链 |
| `testdata/REAL_MET_MANIFEST.json` | ready 哈希更新 |

---

## 7. 给 A 的最短复现

```text
python tools/prepare_era5_pressure_anchors.py
python tools/prepare_era5_hybrid_anchors.py
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point
cargo test --offline -p trajecta-met --test real_era5_query_chain
# CLI（盒内点）见 target/era5_*_cli_ok.log 生成命令
```

**B：** 工程阻断已按 A 裁决闭合并附真实证据；科学终审与 M3 完成标记仍归 A。


---

## 8. 补完：百万点 / native / oracle / 平台（同日后续）

### 8.1 百万点 A2 性能矩阵 — **已实跑**

- 测试：`crates/trajecta-met/tests/real_million_point_perf.rs`
- 报告：`target/m3-million-point-perf.json`
- 覆盖：
  - chunk ∈ {1024, 4096, 16384}
  - 原序 + 固定 permutation
  - 冷/热 column-cache（execute 期 miss=0）
  - 1 vs 4 线程 **全列 bitwise**
  - N vs 2N 耗时（ratio ≈ 2.0）
  - 全量 hot 1,024,017 点 digest（非抽样）
  - engine column-cache resident_bytes
  - 进程 PeakWorkingSet（PowerShell；局限：进程级非纯 allocator）
- 吞吐只记录，**不冻结跨机器绝对门槛**

### 8.2 native 差分 — **测量管道已跑；native 库外部阻断**

- 脚本：`tools/measure_rust_native_diff.py`
- 输出：`target/native-diff/`
  - `native_feature_probe.json` → **external_blocked**（`native-netcdf` / `native-eccodes` 均不可用：`NETCDF_DIR` 空、无 eccodes）
  - `pure_rust_dual_load_cfsr.json` → **measured** bitwise 全字段
  - `SUMMARY.json` → `adjudication: unvalidated_measurement`
- 历史 `1e-4/1e-5/1e-6` 硬编码断言已停用为正式认证（改为 `unvalidated_measurement` 日志）

### 8.3 Oracle / 平台

- Oracle stub 再生成：`target/oracle/M3_FLEXPART_ORACLE_RAW.json` 仍为 `oracle_failure` / 无 samples（无 FLEXPART 二进制）
- 平台矩阵：`target/m3-platform-matrix.json` — Windows 已跑；**Linux 未运行**（CI 配置 ≠ 平台通过）

### 8.4 hybrid 正式入口

- `python tools/era5_hybrid_official_pipeline.py` — raw 缺则 fetch，再 prepare ready
- `tools/fetch_era5_hybrid_cds.py` 标 DEPRECATED

### 8.5 门禁（补完后）

```text
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 211 passed, 2 filtered out
cargo test --offline -p trajecta-met --test real_million_point_perf
  → 1 passed (~690 s)
```

### 8.6 仍不宣称

- M3 完成 / A2 签署 / native 容差认证 / Linux 通过 / FLEXPART 数值通过 / git commit
