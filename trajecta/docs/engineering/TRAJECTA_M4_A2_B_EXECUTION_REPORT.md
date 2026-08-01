# Trajecta M4-A2：B 真实资料正式执行矩阵报告

> 历史责任说明（2026-07-24）：本报告记录的是已经发生的 B 执行轮次。自本报告之后项目不再有 B/C 模型；M4-A2 后续实现、诊断、Windows/WSL 执行、artifact 汇总、报告和验收全部由 A 独立完成。本报告不得被解释为仍可向 B/C 返工或补跑。

状态：**正式 12 格已执行；0/12 passed；不宣称 M4-A2 完成**
角色：B（编排 / 执行 / artifact 汇总 only）
日期：2026-07-23
仓库：`E:\flexpart\trajecta`
git HEAD：`18664f112d7d7cd3155dd10c71b7fecdc7d4ca17`
**未 commit / 未 push**
**未修改数值核心**（boundary / air_mass / integrator / runner lifecycle / met derive·vertical·surface_layer / profiles / numerical contract / A0 science contract / origo-validation）

---

## 1. 总裁决

| 维度 | 状态 |
|---|---|
| 编排脚本 `tools/run_m4_a2_real_matrix.py` | **implemented** |
| WSL 三族双向 64 smoke | **executed / passed**（6/6） |
| Windows ERA5-pressure 1k 预检 | **executed / passed** |
| Windows hybrid 尺度扫描（64/256/1k/2k/5k） | **executed**（见 §5；5k 起出现 particle errors） |
| 正式矩阵 12 格（三族×双向×Win/WSL×10000） | **executed / failed**（**0/12 passed**） |
| 正式 A2 退出条件（三族、双向、Win+WSL、每条 10k、Complete & abnormal==0） | **未满足** |
| external_blocked | **无**（WSL 工具链经本地 cache 同步后可编译运行） |

**B 不修绿。** 数值/气象/domain-fill 失败原样保留，交 A。

---

## 2. 环境身份

### Windows
- OS：Windows 10.0.19045
- rustc：1.95.0 (59807616e 2026-04-14)
- cargo：1.95.0 (f2d3ce0bd 2026-03-21)
- Python：3.12.12
- 证据：`target/m4-a2/summary/windows_identity.json`

### WSL
- Distro：Ubuntu-24.04（WSL2）
- Kernel：`6.18.33.2-microsoft-standard-WSL2`
- rustc/cargo：**1.94.0**（已安装工具链；`stable`/`1.95.0` 缺 cargo 二进制，**未联网安装**，`rustup default 1.94.0`）
- `CARGO_TARGET_DIR=/tmp/trajecta-m4-a2-target`（与 Windows `target/` 隔离）
- Offline crate 缓存：从 `C:\Users\dell\.cargo\registry` **本地 rsync** 到 WSL（非联网下载）
- netcdf 4.9.2 / eccodes 2.34.1 / libclang llvm-18：系统已有
- 证据：`target/m4-a2/logs/wsl_probe.log`、`wsl_set_toolchain.log`、`wsl_sync_cargo.log`、各 cell `stdout.log` 头

### 入口
```text
cargo test --offline [--release] -p trajecta-core --test m4_a2_real_data \
  real_air_mass_domain_fill_three_families_forward_backward \
  -- --ignored --nocapture --exact
```
环境变量：`TRAJECTA_M4_A2_PARTICLES` / `FAMILY` / `DIRECTION` / `ARTIFACT_DIR`

编排：
```text
python tools/run_m4_a2_real_matrix.py --phase wsl-smoke64
python tools/run_m4_a2_real_matrix.py --phase windows-formal10000
python tools/run_m4_a2_real_matrix.py --phase wsl-formal10000
```

---

## 3. 分阶段结果

### 3.1 WSL smoke 64（前置）— passed

| family | direction | status | elapsed_s |
|---|---|---|---:|
| era5-pressure | forward | **passed** | 128.3 |
| era5-pressure | backward | **passed** | 22.6 |
| era5-hybrid | forward | **passed** | 32.8 |
| era5-hybrid | backward | **passed** | 33.3 |
| cfsr-pressure | forward | **passed** | 19.7 |
| cfsr-pressure | backward | **passed** | 20.4 |

汇总：`target/m4-a2/summary/wsl-smoke64_summary.json`

### 3.2 预检
| 项 | 状态 | 说明 |
|---|---|---|
| Windows era5-pressure forward **1000** | **passed** | ~150 s release |
| Windows era5-hybrid forward 64/256/1000/2000 | **passed** | 尺度扫描 |
| Windows era5-hybrid forward **5000** | **failed** | `completed_with_particle_errors`, abnormal_count=1 |

### 3.3 正式 10,000 矩阵（12 格）— 全部 failed

| # | platform | family | direction | particles | status | elapsed_s | manifest.status | abnormal | sqlite integrity | 失败类 |
|---:|---|---|---|---:|---|---:|---|---:|---|---|
| 1 | windows | era5-pressure | forward | 10000 | **failed** | 271.3 | completed_with_particle_errors | 16 | ok | A：invalid_meteorology |
| 2 | windows | era5-pressure | backward | 10000 | **failed** | 267.2 | completed_with_particle_errors | 12 | ok | A：invalid_meteorology |
| 3 | windows | era5-hybrid | forward | 10000 | **failed** | 1.6 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 4 | windows | era5-hybrid | backward | 10000 | **failed** | 0.9 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 5 | windows | cfsr-pressure | forward | 10000 | **failed** | 2.4 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 6 | windows | cfsr-pressure | backward | 10000 | **failed** | 2.0 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 7 | wsl | era5-pressure | forward | 10000 | **failed** | 529.1 | completed_with_particle_errors | 16 | ok | A：invalid_meteorology |
| 8 | wsl | era5-pressure | backward | 10000 | **failed** | 388.8 | completed_with_particle_errors | 12 | ok | A：invalid_meteorology |
| 9 | wsl | era5-hybrid | forward | 10000 | **failed** | 6.1 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 10 | wsl | era5-hybrid | backward | 10000 | **failed** | 5.0 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 11 | wsl | cfsr-pressure | forward | 10000 | **failed** | 3.8 | failed | 0 | ok | B：AirMass(InvalidSample) |
| 12 | wsl | cfsr-pressure | backward | 10000 | **failed** | 3.9 | failed | 0 | ok | B：AirMass(InvalidSample) |

机器可读表：`target/m4-a2/summary/formal10000_matrix_table.json`
阶段汇总：`target/m4-a2/summary/windows-formal10000_summary.json`、`wsl-formal10000_summary.json`

**正式通过计数：0 / 12**

---

## 4. 两类失败（交 A）

### 失败类 A — 首个最小正式复现（时间序第一）

**Windows + ERA5 pressure + forward + 10000**

| 字段 | 值 |
|---|---|
| 期望 | `RunOutcome::Complete`，`abnormal_count == 0` |
| 实际 | `Ok(CompletedWithParticleErrors)` |
| abnormal_count | **16** |
| by_reason | `invalid_meteorology: 16`，`population_outflow: 201`（normal） |
| particle 行 | sqlite `particle: 10000`（已 seed） |
| mass_ledger | 有记录；max \|imbalance\|/tolerance ≈ **1.7e-4**（步进质量门本身未爆） |
| provenance-bundle | 有 |
| particles.sqlite integrity_check | **ok** |
| 确定性对照 | WSL 同格 abnormal 同为 **16**（跨平台一致） |
| backward | Win/WSL abnormal 同为 **12** |

冻结目录：
- `target/m4-a2/first_failure/01_windows_era5-pressure_forward_p10000/`
- 正式格：`target/m4-a2/windows/p10000/era5-pressure/forward/`
- 同行 WSL：`target/m4-a2/wsl/p10000/era5-pressure/forward/`

断言点：`crates/trajecta-core/tests/m4_a2_real_data.rs`（`run() == Ok(Complete)`）

### 失败类 B — domain-fill 采样在 10k 硬失败

**ERA5 hybrid / CFSR pressure × forward/backward × Win/WSL @ 10000**

| 字段 | 值 |
|---|---|
| 期望 | Complete |
| 实际 | `Err(Population("AirMass(InvalidSample)"))` |
| manifest.status | `failed` |
| failure.code | `run.population` |
| 耗时 | ~1–6 s（seed 阶段即失败） |
| provenance | 无（未完成） |
| mass_ledger | 空 |

尺度旁证（Windows hybrid forward，release）：

| particles | 结果 |
|---:|---|
| 64 | passed |
| 256 | passed |
| 1000 | passed |
| 2000 | passed |
| 5000 | failed：`completed_with_particle_errors`, abnormal=1 |
| 10000 | failed：`AirMass(InvalidSample)`（可稳定复现） |

冻结：
- `target/m4-a2/first_failure/02_windows_era5-hybrid_forward_p10000/`
- `target/m4-a2/first_failure/03_windows_cfsr-pressure_forward_p10000/`
- `target/m4-a2/first_failure/04_windows_era5-hybrid_forward_p5000_minish/`

---

## 5. 每格验证项说明

测试体与编排器联合检查：

| 检查项 | 类 A（era5-pressure 10k） | 类 B（hybrid/cfsr 10k） |
|---|---|---|
| RunOutcome::Complete | **否**（CompletedWithParticleErrors） | **否**（Population error） |
| manifest.status complete | **否** | **否** |
| abnormal_count == 0 | **否**（16/12） | n/a（未跑完） |
| seeded_particles == 10000 | sqlite particle=10000 | seed 失败 |
| mass ledger 逐步 ≤ tol | 是（max frac ≪ 1） | 无 ledger |
| particles.sqlite integrity ok | 是 | 是（空/部分库） |
| run-manifest.json | 有 | 有（Failed） |
| provenance-bundle.json | 有 | **无** |
| dataset lock/profile/content SHA | 非空 | 非空 |
| 静默 fallback | 未见 | 未见 |
| family/direction/platform/N 与路径一致 | 是 | 是 |

> 测试在断言失败时**不会**写出 `M4_A2_REAL_MATRIX_SUMMARY.json`；编排器以 `cell_meta.json` + 磁盘 manifest/sqlite 为准，**不把缺 summary 写成 passed**。

---

## 6. Artifact 布局

```text
target/m4-a2/
  summary/
    windows_identity.json
    wsl-smoke64_summary.json
    windows-formal10000_summary.json
    wsl-formal10000_summary.json
    formal10000_matrix_table.json
    ALL_CELLS_SUMMARY.json
  first_failure/
    01_windows_era5-pressure_forward_p10000/
    02_windows_era5-hybrid_forward_p10000/
    03_windows_cfsr-pressure_forward_p10000/
    04_windows_era5-hybrid_forward_p5000_minish/
  windows/p{N}/{family}/{direction}/
    cell_meta.json stdout.log stderr.log
    <case>/<run_id>/run-manifest.json particles.sqlite provenance-bundle.json...
  wsl/p{N}/{family}/{direction}/
    （同上；CARGO_TARGET_DIR 仅在 /tmp/trajecta-m4-a2-target）
  logs/
    wsl_probe.log wsl_set_toolchain.log wsl_sync_cargo.log
    wsl_smoke64_orchestrator2.log
    windows_formal10000_orchestrator.log
    wsl_formal10000_orchestrator.log
```

每格独立目录；后格失败**不覆盖**前格证据。

---

## 7. 状态字段汇总（按合同分类）

| 状态 | 内容 |
|---|---|
| **implemented** | `tools/run_m4_a2_real_matrix.py`；逐格 artifact 校验；本报告 |
| **executed** | WSL smoke64；Win 1k 预检；hybrid 尺度扫描；Win 10k×6；WSL 10k×6 |
| **passed** | WSL smoke 6/6；Win era5-pressure 1k；Win hybrid ≤2k 尺度点 |
| **failed** | 正式 12/12 @10000；hybrid 5k 旁证 |
| **external_blocked** | 无（正式矩阵阶段） |

---

## 8. 明确未做 / 未宣称

- ❌ 未宣称 M4-A2 / M4 完成
- ❌ 未 commit / push
- ❌ 未改容差、公式、算法 ID、失败分类
- ❌ 未把失败写成 skip/passed
- ❌ 未降低 10k 粒子数后伪称正式通过
- ❌ 未跑两个旧 M3 百万点测试
- ❌ 未修改 §一 禁止路径
- ❌ 未在 WSL 联网 `rustup component add` / apt 安装

---

## 9. 请 A

1. 审阅 **失败类 A**：era5-pressure @10k 的 `invalid_meteorology`（Win/WSL abnormal 计数一致 16/12）——是否 seed 域/边界/近表面查询在高粒子密度下暴露。
2. 审阅 **失败类 B**：hybrid/CFSR @10k 的 `AirMass(InvalidSample)`；对照 hybrid 2k pass / 5k particle_error / 10k InvalidSample。
3. 质量账本：类 A 步进 mass imbalance 在 tol 内，但 abnormal 非 0 → 正式门仍失败。
4. 需要 B 补跑的尺度/诊断（仍不改核心）请明示。

**B 正式执行矩阵止于此，交 A。**
