# B 交付：Oracle P0/P1 修正（无静默 Richardson 回退）

日期：2026-07-20
**未改 query / registry / Trajecta 算法 / 容差。未 commit。不宣称 M3 完成。**

FLEXPART：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`

---

## 逐项

| # | 要求 | 状态 |
|---|---|---|
| 1 | metdata_format 按族固定 ERA5=ECMWF / CFSR=NCEP | **done**（`TRAJECTA_ORACLE_METDATA_FORMAT`） |
| 2 | CFSR 官方 HPBL `centre=7,discipline=0,category=3,number=196` → `blh` | **done** |
| 3 | 删除 Richardson 静默科学回退；失败 → failed/partial | **done**（`error stop`，无 hmixmin 回退） |
| 4 | PBL 按存在性/shape/finite 校验；允许真实 0；缺字段不得 complete | **done** |
| 5 | CFSR diagnostics 冻结 sea/plain/mountain query cell | **done**（query SHA 未变） |
| 6 | CFSR harness SHA 含 `oracle_calcpar_mod.f90` | **done** |
| 7 | compile identity/nm 从实际 `--binary` build 目录读取 | **done** |
| 8 | `run_oracle.sh` 三族入口；patcher 全部 retire | **done** |
| 9 | WSL 独立重建复跑；无静默 fallback | **done**（见下） |
| 10 | 旧 registry 真实 MIT 失败，不修绿 | **done** |

---

## WSL 三族 raw 结果

| family | oracle status | ok/75 | richardson token | 备注 |
|---|---|---:|---|---|
| era5_pressure | **failed** | 0 | **hard_fail** | ECMWF 路径全网格 `richardson` 失败；**无静默回退** |
| cfsr_pressure | **complete** | 75 | **0** | NCEP + 官方 HPBL→blh→hmix |
| era5_hybrid | **complete** | 75 | **0** | ECMWF；ETA 完整 |

**静默 Richardson fallback 使用次数：0**（仅 `0` 或 `hard_fail`，无数值回退）。

### Compile identity（从 binary build 目录）

- pressure：`-O2 -g -m64 -cpp … -UUSE_NCF -UETA -Duseomp …`
- ETA：`-O2 -g -m64 -cpp … -UUSE_NCF -DETA -Duseomp …`
- nm（ETA）：`interpol_wind` / `interpol_partoutput_val` / `verttransform_ecmwf` / `oracle_calcpar`

### CFSR HPBL / diagnostics

- GRIB keys：`centre=7,discipline=0,parameterCategory=3,parameterNumber=196,typeOfLevel=surface`
- `cfsr_surface_oracle_adapter_diagnostics.json`：
  - frozen query SHA = queries 文件 SHA（未改点）
  - sea / plain / mountain 四角 + 中心
  - mountain surface pressure + underground fraction by level
  - mountain center 例：terrain≈5509 m，sp≈51100 Pa，blh≈85 m

### PBL 校验

- 必选：`ishf` 存在且 finite；`zust|fricv` 存在且 finite ≥ 0（真实 0 合法）
- NCEP 额外必选：`blh` 存在且 finite ≥ 0
- ECMWF：不要求 blh 预填；hmix 由 richardson 计算

### ERA5 pressure failed（诚实）

冻结 FLEXPART `richardson` 在区域 pressure-level 网格上出现 `ierr=-10`（no stable layer）。
按指令：**不得 hmixmin 静默回退** → oracle `status=failed`，75 条 `oracle_failure`。

---

## MIT（旧 registry，不修绿）

| family | MIT |
|---|---|
| cfsr_pressure | hard_gate_failed（13） |
| era5_hybrid | hard_gate_failed（3） |
| era5_pressure | external_blocked（oracle failed，无 subject） |

Windows gates：`fmt` / `clippy -D warnings` / **236 tests** OK。
WSL MIT：本机 WSL cargo toolchain 不可用；raw oracle 在 WSL 生成，MIT 在 Windows 消费 JSON。

---

## 入口 / 退役

- 三族入口：`tools/flexpart_oracle/run_oracle.sh`
- 退役（exit 2）：`_patch_eta_driver.py`、`_patch_p0_drivers.py`、`_patch_adjudicate_p0.py`、`_apply_p0_format_pbl.py`

---

## 请 A

1. ERA5 **pressure-level** 在 ECMWF `richardson` 硬失败时，是否接受 oracle `failed`，或改 pressure-meter 专用路径裁决
2. CFSR complete + hybrid complete 的 hard_gate 数值差是否进入 ADR
3. WSL 侧 MIT 是否要求补齐 Linux cargo toolchain

**B 不 commit，不宣称 M3 完成，不修绿 registry。**
