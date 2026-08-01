# B 交付：M3 A2 Fixup5（不宣称完成）

对照：`docs/engineering/B_PROMPT_M3_A2_FIXUP5.md`、`docs/engineering/TRAJECTA_M3_A2_B_ROUND_REVIEW.md`。

**未修改** registry version / CFSR 2 ULP 阈值 / rule id / 科学公式 / FLEXPART commit / MIT-GPL 边界。  
**未 git commit。不宣称 M3/A2 完成。**

CFSR 3 ULP 按 A 裁决保留为真实测量；registry 仍为 2 ULP，故 CFSR backend **failed**（exit 1），交 A 发 v1.0.1。

---

## 四栏总览

| 工作包 | implemented | executed | passed | blocked |
|---|---|---|---|---|
| P0-1 exact metadata + FieldSlab 合同 | **yes** | unit + full-field backend | unit 18+ validation; ERA5 数值+metadata 路径 | — |
| P0-2 冻结 expected matrix / 三族强制 | **yes** | Windows full-field | 三族本轮新生成；SUMMARY 要求齐全 | — |
| P0-3 不可拆 U/V vector | **yes** | unit | missing-V / direction fail 负例 | backend 本轮无 vector rule 全场（registry backend 为 scalar） |
| P0-4 gate 对齐统计 + 负例 + schema | **yes** | unit + Draft202012 校验 reports | exact_mismatch 对 ULP 非 0；worst 按 max ULP；reports schema OK | — |
| P0-5 CFSR 独立 3 ULP raw artifact | **yes** | Windows | **复现 A：3 ULP、abs≈3.1019e-25、bits …572 vs …575** | 未改 registry |
| P0-6 query SHA + failed oracle | **partial** | Windows | ERA5 地形选点 formal + SHA 自检；failed oracle schema OK | CFSR query 仍 skeleton（GRIB 地形未接入）；无 complete oracle / 无真实 interpol_* |
| P1-1 IoCallCounters 生产链 | **partial** | million long 1/1 | load 路径 + lock inspector active counters；execute delta=0 长测过 | 非所有 CLI 入口显式 install；inspect 失败路径单测有限 |
| P1-2 Linux core vs A2 full | **yes** (脚本) | **未**在 Linux 主机跑 | n/a | **external_blocked**（本会话 Windows）；脚本不再把 skip 写成 A2 passed |

---

## P0-1 / P0-2 / P0-3 / P0-4 比较器

### 输入合同

- `FieldSlab` + `SideMetadata`（valid_time / grid / vertical / layout / unit / temporal / status）
- `exact_metadata` 逐项比较；未知名由封闭 enum 拒绝
- **禁止** `min(len)`：长度 ≠ `expected_element_count` → structure fail / incomplete
- coverage **仅**来自解码前冻结 expected case ids
- comparison identity 与 slab context 不一致 → incomplete
- vector：缺 V、scalar/vector 规则错配 → config fail；配对 unit/mask/status

### Registry loader

- `deny_unknown_fields`
- calibration path + SHA 校验
- mask/non_finite/decision 结构收紧

### 统计

- 所有 metric：`exact_mismatch_count` = 有限样本数值不等点数
- ULP worst = **max ULP** 点（含 case id / 值）
- abs-rel worst = 相对门槛超限尺度
- invalid = 双方均 invalid

### 负例（unit）

exact pass/fail、NaN、unit/grid metadata、长度截断、缺覆盖、identity mismatch、unvalidated、ULP worst 跟踪、vector 缺分量/方向失败、双方 invalid。

### Schema

生成后 Draft202012 校验 `M3_COMPARISON_REPORT.schema.json`（Python `jsonschema`）。

---

## Backend 全场（冻结 registry v1.0.0）

命令：`python tools/run_m3_backend_comparison.py`  
要求三套齐全；缺输入 → SUMMARY `incomplete` exit 2。

| family | status | compared_total | 说明 |
|---|---|---:|---|
| era5_pressure | **passed** | 297 075 | 数值+metadata 数组通过；**非** A2 认证用语 |
| era5_hybrid | **passed** | 888 675 | 含 lnsp |
| cfsr_pressure | **failed** | 5 897 232 | q 最大 **3 ULP** > registry **2** |

CFSR fail 诊断（report）：

```text
max_ulps=3
exact_mismatch_count ~2.3e5 per file
worst ~ ±9e-10 vs ±9.000000000000003e-10
```

SUMMARY `status=failed`（exit 1）——符合「旧 registry 下 CFSR 仍应 failed」。

---

## P0-5 CFSR raw calibration（未改 registry）

```text
target/m3-comparison/cfsr_ulp_raw_calibration.json
reproduced_max_ulps_ge_3_on_q: true
00: max_ulps=3 gt2=5170 abs=3.101927297073854e-25
    rust_bits=0xbe0eec7bd512b572 native=0xbe0eec7bd512b575  (负值对称点)
06/12: 同 abs，正值 0x3e0eec7bd512b572 vs …575
```

与 A 复核一致。B **未**据此改 registry。

---

## P0-6 Oracle

### Queries

- **formal**（地形规则 sea/plain/mountain）：  
  `target/m3-oracle/queries/era5_{pressure,hybrid}_queries.json`  
  status=`frozen_terrain_selected`；写盘 **bytes/LF**；INDEX size/SHA 自检通过  
- **skeleton**（CFSR GRIB 地形未解码）：  
  `target/m3-oracle/queries-skeleton/cfsr_pressure_queries.json`  
  与 formal **分路径**，不覆盖  
- 生成器 exit 2 当 formal 未齐三族（诚实）

### Failed oracle

- `generate_oracle_stub.py`：**始终非 0**；仅在真实 input files 存在时写 JSON  
- `files` 非空；Draft202012 通过  
- `run_oracle.sh`：缺工具链不写假 complete；README 已去掉不存在的 ps1/src 声称

### 仍 blocked

- 真实 `interpol_wind` / `interpol_partoutput_val`  
- pressure_meter + ETA 双构建  
- complete oracle  
- CFSR 地形选点 formal query  

---

## P1-1 IoCallCounters

- 字段更名为 `format_detection_open_attempt`（不再声称总 open）
- `install_io_counters` / `active_io_counters` 线程上下文  
- `FrameLoader::load` 自动使用 active counters  
- provider 在 publish **尝试后**计数（成败都计）  
- lock `ReaderMetadataInspector` 经 CountingReader  
- **百万点长测已实跑**：`cfsr_pgbl_million_point_performance_matrix` **passed**（~12 min），execute delta=0 断言生效  

---

## P1-2 Linux gate

- `tools/linux_m3_gate.sh`：core vs `a2_full_status`  
- 缺 native/oracle/million → A2=`incomplete`/`external_blocked`，**不会** overall A2 passed  
- oracle 非零不再被当成成功  
- 本会话 **未**在干净 Linux 执行 → Linux A2 = **external_blocked**

---

## 常规门禁（Windows）

```text
cargo fmt --check                         OK
cargo clippy -D warnings                  OK
cargo test --workspace (skip million ×2 filters if any)
  → 233 passed (+ million long separately passed)
Draft202012: registry + 3 backend reports + 3 failed oracles  OK
```

---

## Artifact SHA（节选，以磁盘为准）

运行后请用 `sha256sum` 复核；生成时要点：

| path | 备注 |
|---|---|
| `testdata/M3_TOLERANCES.v1.json` | **未改** |
| `target/m3-comparison/reports/*_backend_report.json` | 三套本轮 |
| `target/m3-comparison/SUMMARY.json` | status=failed |
| `target/m3-comparison/cfsr_ulp_raw_calibration.json` | 3 ULP 复现 |
| `target/m3-oracle/queries/*` | ERA5 formal + INDEX |
| `target/m3-oracle/queries-skeleton/cfsr_*` | skeleton only |
| `target/m3-oracle/artifacts/*_oracle.json` | failed schema-valid |
| `target/m3-million-point-perf.json` | 长测 + counters |

---

## 明确不宣称

- ❌ M3 / A2 完成  
- ❌ ERA5 backend「认证通过」（仅数组/metadata 门禁通过）  
- ❌ CFSR 在 v1.0.0 registry 下通过  
- ❌ FLEXPART oracle 科学闭环  
- ❌ Linux / cross-platform 认证  
- ❌ 擅自把 2 ULP 改为 3  
- ❌ git commit  

## 请 A

1. 在比较器+校准 artifact 基础上发布 **registry v1.0.1（3 ULP）**  
2. CFSR formal terrain query：是否接受 GRIB 地表高度解码进选点器  
3. Oracle harness 优先序（loader vs interpol 接线）  
