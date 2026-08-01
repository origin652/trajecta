# 给 A 模型的 M3 Phase 2–4 交付说明（B）

**日期：** 2026-07-17  
**工作区：** `E:\flexpart\trajecta`  
**角色：** B 工程实现与资料链；**不宣称 M3 完成**；**未 git commit**。  
**范围：** Phase 2–4 真实资料 / 差分门禁 / oracle 边界；**未重做**已验收的 CLI 流式、preferred-profile 候选 glob、外排 fan-in。

---

## 1. 一句话结论

B 已完成：

1. **CFSR 官方 00/06/12** 冻结 + 三时次 Transport 链（含 03/09 中间时刻）+ pure-Rust 双载差分 + 百万点门禁（已实跑）；  
2. **ERA5 pressure 官方 00/06/12** 下载/准备 + classic 派生 + lock→frame；  
3. **ERA5 hybrid 1–137 官方 00/03/06** 下载/准备 + lock→frame；  
4. Oracle **仅 stub**（MIT/GPL 边界未破）；  
5. 默认 workspace 门禁（跳过百万点以控时）通过。

**A 需要裁决/接手的科学点**见第 5 节；**未完成项**见第 6 节。

---

## 2. 已实现且实跑（证据）

### 2.1 CFSR pressure（NCEI pgbl GRIB2）

| 时次 (UTC) | 文件 | size | sha256 (完整) |
|---|---|---:|---|
| 2009-01-01 00 | `pgbl00.gdas.2009010100.grb2` | 5465079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| 2009-01-01 06 | `pgbl00.gdas.2009010106.grb2` | 5436947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| 2009-01-01 12 | `pgbl00.gdas.2009010112.grb2` | 5452202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |

- **目录：** `target/test-data/cfsr-ncei-pgbl-official/`  
- **来源：** NCEI low-resolution  
  `…/6-hourly-low-resolution/2009/200901/20090101/pgbl00.gdas.{stamp}.grb2`  
- **下载命令：**
  ```text
  python tools/fetch_cfsr_pgbl.py --stamp 2009010112 --out-dir target/test-data/cfsr-ncei-pgbl-official
  python tools/fetch_cfsr_pgbl.py --stamp 2009010100 --out-dir target/test-data/cfsr-ncei-pgbl-official \
    --expected-size 5465079 --expected-sha256 fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c
  python tools/fetch_cfsr_pgbl.py --stamp 2009010106 --out-dir target/test-data/cfsr-ncei-pgbl-official \
    --expected-size 5436947 --expected-sha256 00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b
  ```
- **实跑测试：**
  - `cfsr_pgbl_transport_three_frames_00_06_12_and_mid_03_09`  
    lock→inventory→frame→`allow_estimated=false` TransportPlan→prepare→execute；  
    中间时刻 **03 / 09**；AGL / ASL / Pa；12 UTC 端点仍为 `MissingSymmetricTimeSupport`
  - `cfsr_pgbl_pure_rust_dual_load_full_field_diff`（全字段 layout/mask/unit/values 一致）
  - `cfsr_pgbl_million_point_budget_and_threading`（1,024,017 点分块；预算 &lt;1 GiB；1 vs 4 线程全 8 列一致；热路径无新增 column-cache miss）— **此前会话实跑通过**

### 2.2 ERA5 pressure（CDS 官方 NetCDF4 + classic 派生）

| 文件 | 角色 | size | sha256 (前缀…) |
|---|---|---:|---|
| `era5_pressure_20181201.nc` | 官方 3D | 636626 | `93abdfae0943628a…` |
| `era5_surface_base_20181201.nc` | 官方 surface base | 121640 | `69d4eb5bbb42afc8…` |
| `era5_surface_flux_20181201.nc` | 官方 flux | 40071 | `109069654ff48d3c…` |
| `era5_surface_20181201.nc` | 合并 surface | 141692 | `457c9a162faabd89…` |
| classic 派生 | 双后端夹具 | 见 `PREPARE_MANIFEST` / `REAL_MET_MANIFEST` | |

- **日期/时次：** 2018-12-01 **00 / 06 / 12**  
- **区域 NSWE：** `[52, 0, 48, 6]`  
- **3D：** t, u, v, q, w, **z**（37 层）  
- **近地：** sp, z, u10, v10, t2m, d2m, fsr, blh, zust, ishf, ie  
- **命令：**
  ```text
  python tools/fetch_era5_pressure_cds.py --out-dir target/test-data/era5-cds-pressure-official \
    --date 2018-12-01 --times 00:00 06:00 12:00
  python tools/prepare_era5_pressure_anchors.py
  ```
- **实跑：** `real_era5_cds_pressure_classic_rust_full_chain`（3 frames lock→frame）通过  
- **工程备注（请 A 知悉）：**
  - classic pressure 夹具 **故意省略 3D `z`**，避免当前 decoder 对 surface `z` 与 3D `z` 的 `AmbiguousSource`；**3D z 仍在官方 NetCDF4**  
  - 角色判定已修：pressure 轴上的 3D `z` 不再误标 surface  

### 2.3 ERA5 hybrid 1–137（CDS 官方 + prepared）

| 项 | 内容 |
|---|---|
| 日期/时次 | 2018-12-01 **00 / 03 / 06** |
| 层 | **model_level 1–137**（非 flex_extract 8 层子集） |
| 网格 | 0.25°，区域同 pressure 小盒 |
| Job | `e012f0e8-5c37-476d-8f60-13f0cf13a9ca` → `successful`（新 CDS key） |
| 目录 | `target/test-data/era5-cds-hybrid137-official/` |
| 测试入口 | `…/ready/`（仅 prepared，避免 raw 被 lock 盲扫） |

**Prepared 主文件（ready/）：**

| 文件 | size | sha256 |
|---|---:|---|
| `era5_hybrid137_prepared_20181201.nc` | 3563355 | `bf37874175f1b50921f52381d4a9e14d172d83a580255acbe0cc9849c5ad15e6` |
| `era5_surface_20181201.nc` | 130724 | `d978a16a6f3ce41510fbffeb1d613a6057200756c43e3331ccf34b33c0fe8117` |

**准备链（可复现）：**

```text
python tools/run_era5_hybrid_fetch.py
# + cdsapi: lnsp (param 152); hybrid surface 00/03/06 base+flux
cargo run --offline -p trajecta-met --example dump_grib_hybrid_pv
python tools/prepare_era5_hybrid_anchors.py
# ready/ 仅含 prepared hybrid + surface
cargo test --offline -p trajecta-met --test real_netcdf_pipeline real_era5_cds_hybrid137
```

**Profile：** `era5-cds-hybrid137-v0`（`dataset_family: era5_cds_hybrid137`）

**实跑：** `real_era5_cds_hybrid137_rust_full_chain` 通过：

- HybridPressure 拓扑：138 interfaces A/B，active full levels **1–137**  
- 3 时次 lock→inventory→frame  

**科学/合同相关事实（B 未自行裁决）：**

| 事实 | 说明 |
|---|---|
| 垂直速度 | CDS 交付 **omega (`w`, Pa/s)**，映射为 `pressure_vertical_velocity`；**不是** flex_extract 的 `etadot` |
| 表面气压 | prepared 中 `sp = exp(lnsp)`，来源 `era5_lnsp_20181201.nc`，有 comment provenance |
| A/B 系数 | 来自 **同一 CDS 产品 GRIB PV**（probe + `dump_grib_hybrid_pv`），非手填表 |
| 地表时次 | 必须使用 **00/03/06** 专用 surface；不可复用 pressure 的 00/06/12 surface（已踩坑并修正） |
| flex_extract 8 层 | 仍保留为回归；**不得**冒充 137 正式锚点 |

### 2.4 门禁（本机 Windows）

```text
cargo fmt --all --check                                              OK
cargo clippy --offline --workspace --all-targets -- -D warnings      OK
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point
  → 208 passed, 1 filtered out (~60 s)
cargo doc --offline --workspace --no-deps                            OK

# 代表性真实链（子集）
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met \
  --test real_m3_query_chain --test real_cfsr_pipeline   # 此前 00/06/12 链绿
cargo test --offline -p trajecta-met --test real_netcdf_pipeline \
  real_era5_cds_pressure_classic real_era5_cds_hybrid137   # 通过
```

百万点：实现已在；默认 workspace 为控时 skip；**上一会话已完整实跑通过**。

### 2.5 Oracle / 许可边界

- 路径：`tools/flexpart_oracle/` + `target/oracle/M3_FLEXPART_ORACLE_RAW.json`  
- 状态：`oracle_failure` / `flexpart_not_configured`  
- **MIT crate 未链接 GPL**；无数值 samples；**未放宽容差、未科学裁决**  
- Schema 仍服从 `testdata/M3_FLEXPART_ORACLE.schema.json` 与  
  `docs/engineering/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`

### 2.6 Manifest

- `testdata/REAL_MET_MANIFEST.json` 已写入：  
  CFSR 三时次官方集、ERA5 pressure official/classic、**ERA5 hybrid137 ready**  
- 各资料目录下 `FETCH_MANIFEST.json` / `PREPARE_MANIFEST.json` 保留命令与哈希

---

## 3. 关键代码 / 工具（B 新增或本阶段改动）

| 路径 | 作用 |
|---|---|
| `tools/fetch_cfsr_pgbl.py` | CFSR 下载（206/Range/哈希自愈/self-test） |
| `tools/fetch_era5_pressure_cds.py` | ERA5 pressure 官方拉取 |
| `tools/prepare_era5_pressure_anchors.py` | stamp + surface 合并 + classic |
| `tools/run_era5_hybrid_fetch.py` | hybrid 3D 长下载 |
| `tools/prepare_era5_hybrid_anchors.py` | hybrid prepared（A/B + sp + surface 00/03/06） |
| `crates/trajecta-met/examples/dump_grib_hybrid_pv.rs` | 从 CDS GRIB 抽 PV→JSON |
| `crates/trajecta-met/profiles/era5_cds_hybrid137_v0.yaml` | 正式 hybrid profile |
| `crates/trajecta-met/profiles/era5_cf_pressure_netcdf_v0.yaml` | 近地字段扩展 |
| `crates/trajecta-met/src/io/netcdf.rs` | pressure 3D-z 角色；hybrid137 family |
| `crates/trajecta-met/tests/real_m3_query_chain.rs` | 三时次 / 差分 / 百万点 |
| `crates/trajecta-met/tests/real_netcdf_pipeline.rs` | ERA5 pressure/hybrid 链 |
| `tools/flexpart_oracle/*` | GPL 边界 stub |
| `docs/engineering/TRAJECTA_M3_PHASE2_4_STATUS.md` | 工程进度日志 |

**未改动：** A 冻结公式、容差注册表、公共 `Dimension`/canonical 符号约定、MIT 发布物与 GPL 的链接边界。

---

## 4. 已实现但未跑 / 外部阻断

| 项 | 状态 | 说明 |
|---|---|---|
| native-netcdf 全字段差分 | 阻断 | 本机 `netcdf-sys` 需 `NETCDF_DIR` |
| native-eccodes 全字段差分 | 未系统跑 | feature 存在；Windows 环境未配齐 |
| 完整 `TRAJECTA_REQUIRE_REAL_MET=1 cargo test -p trajecta-met --tests` | 未终态全量 | 子集与关键链已跑 |
| Linux / 跨平台矩阵 | 未跑 | 仅 Windows 实跑命令与日志 |
| 真实 FLEXPART 数值 oracle | stub | 无 FLEXPART 二进制与 case harness |
| 百万点默认 CI | 可选 skip | 实现在；需 A/CI 决定是否强制 |
| 旧 CDS 账号卡住 job | 残留 | `e9a4199f` 等曾长期 `accepted`；已用新 key 成功；旧 job 可网页 dismiss |

---

## 5. 请 A 裁决 / 设计的最小问题

1. **Hybrid 垂直速度合同**  
   官方 CDS 给的是 **omega (Pa s⁻¹)**，不是 etadot。  
   当前 Profile 映射为 `pressure_vertical_velocity` 以打通 Transport capability。  
   **请 A 确认：** hybrid 主锚点是否接受 omega 路径；若必须 hybrid-native 垂直速度，给出公式与字段合同（B 不自创）。

2. **Pressure 3D geopotential 消歧**  
   官方 pressure 文件含 3D `z` 与 surface `z` 同名。  
   classic 夹具暂省略 3D `z` 以避免 `AmbiguousSource`。  
   **请 A 确认：** 是否要求 role/layout-aware identity（B 可按设计实现，不先改科学语义）。

3. **`sp = exp(lnsp)`**  
   prepared hybrid 使用该派生并写入 comment。  
   **请 A 确认：** 是否接受为正式锚点表面气压；或要求仅用 single-levels `sp` 且禁止 exp。

4. **容差 / oracle**  
   无注册容差、无 FLEXPART 数值结果。  
   B 只交付 stub + 资料哈希；**A2 终审前不得宣称数值通过**。

5. **NearSurface 全字段 + 查询链**  
   Profile 已挂近地字段；**hybrid/pressure 的完整 NearSurfaceTransport 查询矩阵与 native 差分** 仍待 A 指定通过标准后由 B 补跑或 A 审。

---

## 6. 明确未宣称

- ❌ M3 完成  
- ❌ 三套资料 + native + 跨平台 + oracle **全部**验收通过  
- ❌ 放宽任何 A 冻结容差 / 公式 / 符号  
- ❌ 将 flex_extract 8 层 hybrid 当作 137 正式锚点  
- ❌ git commit（工作区保持未提交，供 A 审阅）

---

## 7. 建议 A 的复现顺序（最短）

```text
# 1) 默认门禁
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point

# 2) CFSR 三时次 + 中间时刻（需资料在 target/test-data/cfsr-ncei-pgbl-official）
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met --test real_m3_query_chain

# 3) ERA5 pressure classic + hybrid prepared
cargo test --offline -p trajecta-met --test real_netcdf_pipeline \
  real_era5_cds_pressure_classic real_era5_cds_hybrid137

# 4) 读 manifests
# target/test-data/cfsr-ncei-pgbl-official/FETCH_MANIFEST.json
# target/test-data/era5-cds-pressure-official/FETCH_MANIFEST.json
# target/test-data/era5-cds-hybrid137-official/PREPARE_MANIFEST.json
# testdata/REAL_MET_MANIFEST.json
# target/oracle/M3_FLEXPART_ORACLE_RAW.json
```

---

## 8. 给 A 的审阅清单（可勾选）

- [ ] CFSR 00/06/12 哈希与 NCEI 来源是否认可为正式锚点  
- [ ] ERA5 pressure 00/06/12 变量/层/近地是否满足 Transport+NearSurface 合同  
- [ ] ERA5 hybrid 137 prepared 的 A/B+sp+omega 路径是否可进入科学验收  
- [ ] 3D `z` AmbiguousSource 规避是否可接受，或需改 identity 合同  
- [ ] 百万点门禁是否进入默认 CI  
- [ ] Oracle stub 是否满足 MIT/GPL 边界；真实 FLEXPART 是否由 A 指定环境  
- [ ] 是否授权 B 继续：native 矩阵 / hybrid 全查询 / 跨平台  

---

**B 签名式说明：** 以上为工程交付与可复现证据清单；科学通过/失败由 A 按冻结合同与实测裁决。  
同步工程日志：`docs/engineering/TRAJECTA_M3_PHASE2_4_STATUS.md`。
