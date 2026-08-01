# B 交付：M3 Oracle pressure-coordinate P0/P1 返工

日期：2026-07-20
依据：`docs/engineering/B_PROMPT_M3_ORACLE_PRESSURE_COORDINATE_REWORK.md`
FLEXPART：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`

**未改 query / registry / Trajecta 算法 / 容差。未 commit。不宣称 M3/A2 完成。**
**未跑 WSL full+million**（等 A 复核 P0）。

---

## 逐条验收

| # | 要求 | 状态 |
|---|---|---|
| 1 | ERA5/CFSR pressure → `verttransform_gfs`；hybrid → `verttransform_ecmwf` | **done** |
| 2 | 三维身份：`source_family` / `vertical_coordinate` / `pbl_height_mode` | **done**（sidecar + harness_version tokens） |
| 3 | ERA5 pressure：official BLH prescribed；CFSR：official HPBL；hybrid：Richardson diagnosed | **done** |
| 4 | pressure calcpar：地上第一层 + prescribed hmix + richardson 只供 wstar；`ierr<0` hard fail | **done** |
| 5 | ieee_is_finite；缺字段/NaN/Inf/shape 不得 complete | **done** |
| 6 | CFSR bilinear cell center + 四角；query SHA 不变 | **done** |
| 7 | nm/compile identity 从 `--binary` build 目录强制读取 | **done** |
| 8 | 旧产物隔离；旧 CFSR 13 hard fails 作废 | **done** |
| 9 | WSL 独立 build 重建；canonical/audit `.text` SHA | **done** |
| 10 | MIT：pressure adapter provisional → `unvalidated`；不修绿 | **done** |

---

## 三族身份与 transform

| family | source_family | vertical_coordinate | pbl_height_mode | vertical_transform | schema build_mode | adapter |
|---|---|---|---|---|---|---|
| era5_pressure | era5 | pressure | official_prescribed | **verttransform_gfs** | pressure_meter | pressure_meter_adapter |
| cfsr_pressure | cfsr | pressure | official_prescribed | **verttransform_gfs** | pressure_meter | pressure_meter_adapter |
| era5_hybrid | era5 | hybrid_eta | richardson_diagnosed | **verttransform_ecmwf** | eta | eta_hybrid |

冻结 schema 仅允许 `build_mode ∈ {pressure_meter, eta}`，故 adapter 名与三维身份写入：

- `artifacts/*_oracle_identity.json` sidecar
- `oracle.harness_version` 分号 token（`adapter=…;source_family=…;vertical_coordinate=…;pbl_height_mode=…;vertical_transform=…`）

### pressure-coordinate 科学路径要点

1. Driver 调用 **`verttransform_gfs`**（非 ecmwf）。
2. `oracle_calcpar` 以显式 `pbl_height_mode` 分支（公式/常量与冻结 NCEP/ECMWF 分支一致）：
   - prescribed：按 local `ps` 找第一地上 pressure level → Obukhov；hmix 保留官方 BLH/HPBL；richardson 只写 wstar/hmixplus。
   - diagnosed：hybrid ETA 路径。
3. Adapter 桥：GFS 将 z 网格气压写入 `pplev`，meter 插值读 `prs` → transform 后 **`prs = pplev`**（机械别名，不改公式）。
4. Richardson **无** hmixmin/旧值/零值静默回退；token 仅 `0`。

---

## Raw oracle 75/75

| family | status | ok/75 | richardson | silent fallback |
|---|---|---:|---|---|
| era5_pressure | **complete** | 75 | **0** | **0** |
| cfsr_pressure | **complete** | 75 | **0** | **0** |
| era5_hybrid | **complete** | 75 | **0** | **0** |

### Query SHA（未改）

| family | query_sha256 prefix |
|---|---|
| era5_pressure | `dfc6818d…` |
| cfsr_pressure | `d3b7d0a5…` |
| era5_hybrid | `ccfdf320…` |

### Binary / harness（canonical pressure rebuild）

| family | binary_sha256 prefix | harness_sha256 prefix | artifact sha256 prefix |
|---|---|---|---|
| era5_pressure | `1aa2085d` | `8c175654` | `4990fad2` |
| cfsr_pressure | `1aa2085d` | `78d3cbe8` | `db07713e` |
| era5_hybrid | `96bf830e` | `98fd35f8` | `23fac506` |

完整值见 `target/m3-oracle/ORACLE_RUN_SUMMARY.json` 与 `*_oracle_identity.json`。

### nm 证据

- pressure：`verttransform_gfs`, `interpol_wind`, `interpol_partoutput_val`, `interpol_pbl`, `oracle_calcpar`
- ETA：`verttransform_ecmwf` + 同上插值/calcpar

### Canonical vs audit

`target/m3-oracle/AUDIT_BINARY_COMPARE.json`：

- ELF **`.text` SHA equal**（pressure 与 eta）
- compile_flags **path-normalized equal**
- nm **symbol set equal**
- whole-binary SHA 可因 build-id/路径不同（已注明）

---

## CFSR bilinear diagnostics

文件：`target/m3-oracle/work/cfsr-pressure/cfsr_surface_oracle_adapter_diagnostics.json`

Mountain 四角 terrain (m)：

```text
5509.447, 5167.124, 771.060, 2686.144
```

Bilinear cell center：

```text
terrain_m ≈ 3533.444
sp_pa_t0  = 67735.0
```

`center_underground` 使用 **bilinear center SP**，不是角点。
另保留 `mountain_grid_anchor`（最近格点）以免与 center 混淆。

---

## MIT（旧 registry，不修绿）

| family | MIT rollup | common report |
|---|---|---|
| era5_pressure | **partial** | **unvalidated**（provisional adapter variant 未注册） |
| cfsr_pressure | **partial** | **unvalidated** |
| era5_hybrid | **hard_gate_failed** | failed（**3** hard rows；旧 registry 真实失败） |

comparison variant：

- pressure：`trajecta-vs-flexpart-pressure-adapter-provisional`
- hybrid：`trajecta-vs-flexpart-v11.1`

**旧 CFSR 13 hard fails 作废，不进入 ADR。**
pressure 两族因 variant 未注册保持 unvalidated，**不得写成 passed**。

---

## 门禁

```text
cargo fmt --all --check                     OK
cargo clippy --offline --workspace --all-targets -- -D warnings  OK
cargo test --offline --workspace            OK（million 过滤）
cargo doc --offline --workspace --no-deps   OK
python -m py_compile tools/flexpart_oracle/*.py tools/run_m3_oracle_comparison.py  OK
schema Draft 2020-12                        OK（oracle JSON）
```

Build dirs：`target/m3-oracle/build/pressure-meter-pcrework`、`eta-hybrid-pcrework`（+ audit 对照目录）。
入口：`tools/flexpart_oracle/run_oracle.sh`（运行前清理 canonical 产物）。

---

## 明确未做

- 未改 registry / frozen query / Trajecta 科学公式 / 容差
- 未 commit
- 不宣称 M3/A2 完成
- 未跑 WSL `linux_m3_gate.sh` full+million

---

## 请 A

1. 注册 pressure-adapter provisional variant 或发布新 registry 版本后，再裁决 native/interpolated hard gate。
2. hybrid 仍有 3 个 hard fails：是否 ADR / science 裁决。
3. P0 通过后是否授权 WSL full+million。

**B 交付到此止。**
