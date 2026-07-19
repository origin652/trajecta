# B 交付：M3 A2 Oracle / Counters / Linux（不宣称完成）

对照 `docs/B_PROMPT_M3_A2_ORACLE_COUNTERS_LINUX.md`。  
**不修改**科学公式 / 容差阈值 / selector / FLEXPART commit / MIT-GPL 边界。  
**不 git commit。不宣称 M3/A2 完成。**

---

## 总览四栏

| 工作包 | implemented | executed | passed | blocked |
|---|---|---|---|---|
| IoCallCounters + CountingReader + FrameLoader | **yes** | unit + wired into million-point load path | unit pass; million long **not re-run this turn** after counter wire | million long re-execution pending wall-clock |
| Tolerance registry + unified comparator | **yes** | unit tests + full-field backend runs | unit 7 pass; ERA5 backend reports **passed** | — |
| Native → registry comparison (3 families) | **yes** | Windows full-field | ERA5 pressure/hybrid **passed**; CFSR **failed** (3 ULP > registry 2) | CFSR hard-gate failure awaiting **A adjudication** (B will not widen) |
| FLEXPART GPL oracle harness | **partial** | query generator only | queries ≥75/family generated | **no** complete oracle JSON: Fortran driver not linked to FLEXPART `interpol_*`; gfortran path may be external |
| MIT oracle comparator | **partial** (registry path ready) | not against real oracle | n/a | blocked on complete oracle artifacts |
| Linux gate script | **yes** (`tools/linux_m3_gate.sh`) | **not** on clean Linux host this session | n/a | **external_blocked** (Windows session; no Linux runner evidence) |

---

## 1. Reader/provider 计数器

**实现**

- `crates/trajecta-met/src/io/metrics.rs` — `IoCallCounters` / `IoCallSnapshot`（`Arc`，原子，无全局静态）
- `crates/trajecta-met/src/io/counting_reader.rs` — 装饰真实 `MetReader`
- `FrameLoader::load_with_io(..., Option<Arc<IoCallCounters>>)`  
  - filesystem open（detect）  
  - build_index / decode（CountingReader）  
  - provider_frame_load（publish 成功后）

**合同行为**

- `prepare`（帧已 cache）：snapshot 不变  
- `PreparedBatch::execute`：snapshot 不变（硬断言写入 million-point 测试）  
- 帧加载阶段：`total > 0` 且 `build_index/decode/provider_frame_load > 0`

**证据路径**：`crates/trajecta-met/tests/real_million_point_perf.rs`（已改；**本轮未重跑 12min 长测**）

---

## 2. Registry 解析与统一比较器

**实现**

- `crates/trajecta-met/src/validation/tolerance.rs`  
  - load + SHA  
  - unique match（0→unvalidated，>1→ambiguous）  
  - exact / ordered binary64 ULP / abs-rel / vector  
- `crates/trajecta-met/src/validation/report.rs`  
  - `adjudicate` → `trajecta.m3.comparison_report/v1`  
  - metadata/mask/non-finite 先于数值  
  - report_only → `reported`  
  - overall `passed|failed|incomplete|unvalidated`

**合成负例（unit）**：exact pass/fail、NaN reject、missing coverage incomplete、zero-match unvalidated。

**未做**：jsonschema crate 运行时校验（结构由 serde + 手工字段对齐 schema；可用外部 ajv 再验）。

---

## 3. Native 全场 → 正式 registry

**入口**

- Example：`adjudicate_backend_fields`（全场逐点，非 worst 代理）  
- 脚本：`tools/run_m3_backend_comparison.py`  
- Spec 分隔符 `::`（兼容 Windows `E:`）

**Windows 实跑结果**

| Report | status | compared_count Σ | notes |
|---|---|---|---|
| `target/m3-comparison/reports/era5_pressure_backend_report.json` | **passed** | 297 075 | pressure+surface 全字段×3 时次 |
| `target/m3-comparison/reports/era5_hybrid_backend_report.json` | **passed** | 888 675 | 含 **lnsp** |
| `target/m3-comparison/reports/cfsr_pressure_backend_report.json` | **failed** | 5 897 232 | 00/06/12 全场；**q (0.1.0)** max **3 ULP** > registry **2** |

CFSR 失败样例（不扩阈值）：

```text
field grib:0.1.0:isobaric
maximum_ulps: 3
maximum_absolute_difference: ~1.73e-18 … 3.47e-18
rule: backend/cfsr-grib/two-ulp-fields/v1
```

按合同：保留 artifact，交 **A** 裁决。B **不**改 `M3_TOLERANCES.v1.json`。

`SUMMARY.json`：`status=failed_or_incomplete`（因 CFSR failed）。

---

## 4. FLEXPART GPL oracle

**冻结 commit**：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`  
**checkout 存在**：`E:/flexpart`（邻目录）

**已实现**

- `tools/flexpart_oracle/README.md`（GPL 边界）  
- `tools/flexpart_oracle/generate_queries.py` → 每族 **75** 条（27+18+12+18）  
  - `target/m3-oracle/queries/{era5_pressure,era5_hybrid,cfsr_pressure}_queries.json`  
- `tools/flexpart_oracle/run_oracle.sh` — commit 检查 + toolchain 探测；**拒绝**假 complete

**未实现 / blocked**

- 调用真实 `interpol_wind` / `interpol_partoutput_val` 的 GPL 驱动  
- pressure_meter vs ETA 双构建  
- schema-valid **complete** oracle JSON  
- 地形驱动 sea/plain/mountain 选点（当前 query 为骨架点 + 明确 `skeleton_sites_not_terrain_selected`）

`generate_oracle_stub.py`：仍非 v1 科学 oracle（保持探针定位，不作为主门禁成功）。

---

## 5. MIT oracle 比较

比较器基础设施 = validation 模块（flexpart_common_semantics / report_only 规则已在 registry）。  
**未执行**：无 complete oracle 输入 → 状态 **blocked**。

---

## 6. Linux 门禁

- 脚本：`tools/linux_m3_gate.sh`（fmt/clippy/tests/doc/era5/backend/oracle/million 可选）  
- **本会话未在干净 Linux x86_64 实跑** → `external_blocked`  
- 不把 Windows 结果冒充 Linux / cross-platform 认证

---

## 7. 常规门禁（本机 Windows）

```text
cargo fmt --all -- --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 预期 214+（含 validation / contract inventory 82 modules）
cargo doc --offline --workspace --no-deps           OK（先前已跑）
```

百万点长测：代码已接计数器断言；**本轮未重跑**（注明 blocked/pending execution）。

---

## 8. Artifact SHA（节选）

运行后以磁盘为准；生成摘要时请重算：

```text
testdata/M3_TOLERANCES.v1.json
target/m3-comparison/reports/era5_pressure_backend_report.json   status=passed
target/m3-comparison/reports/era5_hybrid_backend_report.json     status=passed
target/m3-comparison/reports/cfsr_pressure_backend_report.json   status=failed (3 ULP)
target/m3-comparison/SUMMARY.json
target/m3-oracle/queries/*_queries.json                          n=75 each
```

---

## 9. 明确不宣称

- ❌ M3 / A2 完成  
- ❌ CFSR backend 认证通过  
- ❌ FLEXPART oracle 科学闭环  
- ❌ Linux / cross-platform 认证  
- ❌ git commit  
- ❌ 擅自放宽 2 ULP 或任何 hard gate  

## 10. 请 A 裁决 / 下一刀

1. **CFSR q 全场 max 3 ULP** vs 注册表 2 ULP：是否调查 decoder 还是升级 registry（需 A 书面）？  
2. Oracle harness：优先 GPL 驱动最小 `interpol_*` 路径，还是先完成地形选点冻结 query？  
3. 百万点长测是否强制本轮补跑作为 A2 签字前置？
