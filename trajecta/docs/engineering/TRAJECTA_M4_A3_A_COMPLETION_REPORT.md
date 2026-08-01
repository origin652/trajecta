# Trajecta M4-A3：A 完成裁决报告

状态：**M4-A3 complete；M4 整体未完成**
日期：2026-07-24
仓库：`E:\flexpart\trajecta`
基线：`main @ 18664f112d7d7cd3155dd10c71b7fecdc7d4ca17`
提交状态：**未 commit / 未 push**

> 责任声明：B 模型可以由用户另行使用，但 A 不调用子代理，也不代用户启动 B。若后续有合同已冻结、
> 风险为中等及以下的机械任务需要交接，A 只编写详细 Prompt，由用户自行发送。M4-A3 的科学实现、
> 根因诊断、Windows/WSL/native 实跑、GPL oracle、差异分类、artifact 汇总和最终裁决均由 A 完成。

## 1. 裁决

M4-A3 的退出条件包括：现代球面 Ertel PV、具名臭氧规则注册表、PV60 legacy 标量规则、carrier
dry-air mass 与 ozone substance mass 分离、三套真实资料双平台双方向运行、独立 GPL 标量 oracle，
以及现代 PV 与 FLEXPART legacy 行为的诚实差异报告。

本轮冻结证据如下：

| 项 | 结果 |
|---|---|
| Windows ozone 真资料矩阵 | **6/6 passed** |
| WSL Ubuntu-24.04 ozone 真资料矩阵 | **6/6 passed** |
| 平台矩阵合计 | **12/12 passed；0 failed；0 external_blocked** |
| 每格规模 | **1,000 粒子、600 s、2 × 300 s 数值步** |
| RunOutcome / manifest | **Complete / complete** |
| abnormal termination | **0** |
| ozone mass | **每格 1,000 个正有限质量记录** |
| carrier mass ledger | **逐步在冻结容差内** |
| SQLite | **integrity_check=ok** |
| GPL PV60 scalar oracle | **10/10 passed** |
| Rust/native 真资料差分 | **6/6 pair reported；0 blockers** |
| native logical-row coverage | **完整；无 Rust-only/native-only key** |
| schema/hash/finite gate | **passed** |

因此 A 正式裁决：**M4-A3 完成**。该结论不扩大为 M4 完成；M4-A4 的 10 万粒子性能、RSS、
SQLite 体积、执行期 I/O、1/4 worker 决定性以及跨平台差异终审仍未完成。

## 2. 已冻结的科学与生命周期

### 2.1 现代 PV

生产算法 ID 为 `ertel_pv_spherical/v1`。它在 native pressure/hybrid grid 上计算完整球面
pressure-coordinate Ertel PV：

```text
theta = T * (100000 Pa / p)^(Rd/Cpd)

PV = -g * [
  (zeta_p + 2 Omega sin(phi)) * d(theta)/dp
  + d(u)/dp / R * d(theta)/d(phi)
  - d(v)/dp / (R cos(phi)) * d(theta)/d(lambda)
]
```

实现使用实际 pressure 的非均匀三点 Lagrange 垂直导数；hybrid 横向导数先把相邻柱重映射到
中心点实际 pressure；缺 halo、少于三层、非单调 pressure、精确极点或非有限输入均硬失败。

### 2.2 PV60 规则

规则 ID 为 `flexpart_stratospheric_ozone_pv60/v1`：

```text
height_asl > 3000 m
hemisphere_pv = pv       when latitude >= 0
hemisphere_pv = -pv      when latitude < 0
hemisphere_pv > 2 PVU

ozone mole fraction = hemisphere_pv * 60e-9
ozone mass = dry-air carrier mass * mole fraction * 48/29
```

3 km 与 2 PVU 都是严格大于。规则外的 carrier 空气不产生 ozone mass，但不会因此删除 carrier。
ozone mass 是独立 substance mass；输送和守恒账本继续使用 dry-air carrier mass。

### 2.3 生命周期

- 初始化只在 PV60 eligible dry-air mass 内精确生成 target count；
- fixed-mass target 指 carrier mass，不指 ozone mass；
- 有限域边界只对 eligible inward dry-air flux 生成臭氧粒子；
- 粒子生成后 ozone mass 持久保存，不因以后穿越 3 km 或 2 PVU 阈值而重新赋值、清零或删除；
- manifest 写入 population ID 与 ozone rule ID；SQLite 每个粒子写独立 ozone mass 行。

## 3. Hybrid `MissingOzoneDiagnostic` 根因与修复

首次正式 ERA5 hybrid 运行在播种阶段报 `Population("MissingOzoneDiagnostic")`。快照审计显示：

- 有 4,508 个 native layer slots 没有可用 PV；
- 这些 slots 的最高高度约为 2,562 m；
- 它们全部低于 PV60 严格 `height_asl > 3000 m` mask。

旧顺序先要求每层存在 PV，再判断高度，因此把几何上永远不可能参与臭氧播种的低层缺 PV 错误升级为
fatal。修复后的顺序为：

1. 先按 3 km 几何阈值裁掉无 eligible 厚度的层或边界面；
2. 边界再裁掉零 inward flux；
3. 只对剩余真正 eligible 的质量要求 PV；
4. 真正跨过 3 km 且缺 PV 仍返回 `MissingOzoneDiagnostic`。

冻结单测覆盖低层缺 PV、低层边界缺 PV、零入流边界缺 PV，以及真正 eligible 缺 PV 的硬失败。

## 4. 三套真实资料：Windows / WSL 12 格

证据根目录：

- `target/m4-a3/windows/p1000/`
- `target/m4-a3/wsl/p1000/`

| family | direction | Windows | WSL |
|---|---|---:|---:|
| ERA5 pressure | forward | 36.247 s | 41.696 s |
| ERA5 pressure | backward | 35.801 s | 42.236 s |
| ERA5 hybrid | forward | 68.400 s | 75.901 s |
| ERA5 hybrid | backward | 68.866 s | 73.590 s |
| CFSR pressure | forward | 25.073 s | 30.542 s |
| CFSR pressure | backward | 24.623 s | 30.366 s |

每格均满足：

- seeded/final particle rows = 1,000；
- ozone mass rows = 1,000，且质量为正有限值；
- numerical steps = 2；
- `RunOutcome::Complete`；
- `abnormal_count == 0`；
- mass ledger 两步均在冻结 tolerance 内；
- manifest、provenance bundle、SQLite 均完整，SQLite integrity 为 `ok`。

环境身份：

- Windows：Microsoft Windows NT 10.0.19045，`rustc 1.95.0` GNU host，`cargo 1.95.0`；
- WSL：Ubuntu-24.04 / WSL2 kernel `6.18.33.2-microsoft-standard-WSL2`，`rustc 1.94.0`，
  `cargo 1.94.0`，glibc `2.39`；
- WSL 身份是在正式运行后只读复采；运行 artifact 本身仍以各格 summary/manifest SHA 为准。

## 5. Rust/native 真资料差分

编排器：`tools/run_m4_a3_native_diff.py`
机器报告：`target/m4-a3/native-diff/M4_A3_RUST_NATIVE_DIFF.json`
状态：`measured_all_pairs`，6 对比较，0 blockers。

Windows native 环境：

- native-netCDF：netCDF-C `4.9.3`，HDF5 `1.14.6`；
- native-eccodes：ecCodes `2.47.0`；
- libclang `22.1.8`，DLL SHA-256
  `95d25a00db6c27c29627e8b955a0288c40dd67533a94adc010e8acb9d22bfad1`；
- SQLite `3.51.1`。

比较器按逻辑主键逐表对齐 `particle`、`particle_mass`、`output_event`、`particle_state`、
`termination`。它比较状态、终止、U/V/W、pressure、temperature、validity、quality、位置、carrier
mass、ozone mass 和逻辑行覆盖；run UUID 和 SQLite 文件字节不参加比较。

| family | direction | Rust | native | 结果 |
|---|---|---:|---:|---|
| ERA5 pressure | forward | 35.09 s | 32.70 s | 全逻辑数值逐值 exact |
| ERA5 pressure | backward | 32.72 s | 31.28 s | 全逻辑数值逐值 exact |
| ERA5 hybrid | forward | 71.55 s | 72.67 s | 全逻辑数值逐值 exact |
| ERA5 hybrid | backward | 71.62 s | 69.97 s | 全逻辑数值逐值 exact |
| CFSR pressure | forward | 25.17 s | 30.50 s | 仅 W 有 132 个非 exact 样本 |
| CFSR pressure | backward | 25.92 s | 30.59 s | 仅 W 有 129 个非 exact 样本 |

CFSR W 的最大差异：

| direction | exact mismatch | max abs | max rel | max ULP |
|---|---:|---:|---:|---:|
| forward | 132 | `6.938893903907228e-18 m/s` | `3.55718128847746e-14` | 161 |
| backward | 129 | `6.938893903907228e-18 m/s` | `4.870198923500685e-14` | 322 |

六对比较均无：logical key 缺失、非有限值、value-presence mismatch、状态差异、终止差异、
validity/quality 差异、位置差异、U/V/P/T 差异、carrier/ozone mass 差异。ERA5 的 canonical SQL
digest 相同；CFSR 因上述 W 舍入差而不同。完整轨迹差异按合同为 `report_only`，因此不为获得字节相同
而修改现代 W、PV、RK2 或 reader。

## 6. GPL PV60 scalar oracle

工具：`tools/flexpart_oracle/run_m4_a3_ozone_scalar_oracle.py`
artifact：`target/m4-a3/oracle/M4_A3_OZONE_SCALAR_ORACLE.json`
FLEXPART commit：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`

结果：

- 10/10 records passed；
- 覆盖 3 km 严格边界、2 PVU 严格边界、南北半球符号、赤道和小 carrier mass；
- 最大相对差约 `7.6087e-8`，小于冻结 `5e-7`；
- 容差来源仅为 FLEXPART default `REAL(32)` 与 Rust `f64` 的表示差；
- GPL Fortran driver 与 MIT Rust subject 通过独立进程和文本输入连接，没有 GPL 代码链接进 workspace crates。

本轮同时修复了 Windows 沙箱可复跑性：只读 Git 调用使用局部 `safe.directory`；gfortran 的
TMP/TEMP/TMPDIR 指向仓库 target；UCRT64 runtime 被放在 PATH 首位，避免加载 Mellanox 附带的
ABI 不兼容 `libwinpthread-1.dll`；Windows 可执行文件显式使用 `.exe`。

## 7. 现代 Ertel PV 与 FLEXPART `calcpv` 的差异裁决

FLEXPART v11.1 的冻结源码身份已进入 oracle artifact。关键源码位置：

- `src/getfields_mod.f90:328`：`calcpv`；
- `src/getfields_mod.f90:594`：legacy PV 主式；
- `src/initdomain_mod.f90:859`：3 km / 2 PVU mask；
- `src/initdomain_mod.f90:950`：ozone mass；
- `src/par_mod.f90:107`：`ozonescale=60, pvcrit=2`。

两者不是同一个离散算法：

| 方面 | Trajecta `ertel_pv_spherical/v1` | FLEXPART `calcpv` |
|---|---|---|
| 物理式 | 完整 pressure-coordinate Ertel，含两项 baroclinic tilting | `-g dtheta/dp (f + isentropic relative vorticity)` |
| 水平风导数 | native grid 三点 Lagrange；hybrid 邻柱映射到中心 pressure | 搜索相邻柱同 theta 层后做 legacy 差分，失败时退回同 pressure-level 风 |
| 垂直导数 | 实际 pressure 非均匀三点；顶底 one-sided | 当前层上下相邻层两点差 |
| 常量/精度 | `Omega=7.2921150e-5`、`g=9.80665`、f64 | `f=0.00014585 sin(phi)`、`g=9.81`、default REAL |
| 极点 | 精确极点无唯一 east basis，硬无效 | 用相邻纬圈平均值填极点 |
| 角色 | 现代生产科学算法 | 版本化 legacy 行为参考 |

因此本报告不声称两者场值应逐点相同，也不声称一个在所有资料和尺度上必然“更准”。Trajecta 的目标是
忠实实现公开的完整 Ertel 定义；FLEXPART `calcpv` 的价值是冻结历史行为。真正 hard-gated 的 legacy
部分是 3 km / 2 PVU / 60 ppbv / 48÷29 的标量规则，已经由 GPL oracle 通过。`calcpv` 场差异只作
`report_only`；A3 未进行完整 FLEXPART `calcpv` 三维场数值等价声明，也未为修绿反向拟合现代 PV。

## 8. Artifact SHA-256

平台矩阵 summary：

| platform | family | direction | SHA-256 |
|---|---|---|---|
| Windows | CFSR pressure | backward | `be58ae298d6eea0b213a99d39c807751913fcdc083358253944ad67f46d1ef5a` |
| Windows | CFSR pressure | forward | `bfacb85d5b0fa82f56cffe30677fe43388d56fc99cc4a4533e9472e992560ab2` |
| Windows | ERA5 hybrid | backward | `f1202f65f0603bed01e0c319fe78bb69eebde07b2399425b8215527f75394342` |
| Windows | ERA5 hybrid | forward | `3226288328547e2f4dc6f3dedefb6b18e8c67abb2dcd6727d3a03b3ab9e0fe89` |
| Windows | ERA5 pressure | backward | `e90f02c123480facffa3a6613918037d45507f1484a70a2d9196c62070aed23b` |
| Windows | ERA5 pressure | forward | `e2c45b3087ad251162c42e77f65a398cfab858b84c401b58b7fbc8e08b82de35` |
| WSL | CFSR pressure | backward | `da67c209ecafa8095c5df970fe3ad4c44ed9907d94e785dfc07d397e6989574e` |
| WSL | CFSR pressure | forward | `1997ba9b5949027e1f82b9f025b99a72433a1c238824b1e0755c633ab77ee345` |
| WSL | ERA5 hybrid | backward | `d0d9a2f4d2b6be3a242d9e901a111965ee09d7cdd66060d7c256c0460fa5f078` |
| WSL | ERA5 hybrid | forward | `f963c64d08d5be0b59c6792562faf57d5cdd123263c5373f8261071618490c9d` |
| WSL | ERA5 pressure | backward | `fc8c76d149ad148dea1b86ed90abfe94f637cc414b52514a9abab911f42576ca` |
| WSL | ERA5 pressure | forward | `0de34d9cbd2f8358f3fab3ed6fcf7adcb53391d845bf91e93480886ded96ca01` |

汇总 artifact：

| artifact | SHA-256 |
|---|---|
| `target/m4-a3/native-diff/M4_A3_RUST_NATIVE_DIFF.json` | `ec39285e3528d311d5341ff3a1afcafdfddcf84840a560b6923eef4d7f02f7ca` |
| `target/m4-a3/oracle/M4_A3_OZONE_SCALAR_ORACLE.json` | `fb6ce0ac845988c3160df550be1a4763d0d82e589f8f4216fc45bce471193505` |

native diff 报告还逐对记录 summary、manifest、SQLite、provenance bundle 的独立 SHA；SQLite 文件字节
不作为 Rust/native 等价条件。

## 9. 最终门禁

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | OK |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | OK |
| `cargo test --workspace --offline` | **411 passed / 6 ignored** |
| `cargo doc --workspace --no-deps --offline` | OK |
| `python -m py_compile ...` | OK |
| `python tools/validate_m4_a0_contracts.py` | passed |
| `python tools/flexpart_oracle/run_m4_a3_ozone_scalar_oracle.py` | 10/10 passed |
| Windows/WSL A3 真资料矩阵 | 12/12 passed |
| Windows Rust/native A3 差分 | 6/6 pair reported，0 blockers |
| artifact JSON parse | OK |
| `git diff --check` | OK |

6 个 ignored 为 3 个显式 M4-A2 真资料入口、1 个显式 M4-A3 真资料入口和 2 个旧 M3 百万点性能
测试。A3 的 12 格与 native 6 对已由独立正式 runner 实际运行，未用 ignored 状态冒充通过。

## 10. 未完成与禁止扩大声明

- M4-A4 尚未完成；
- WSL 100,000 粒子 air-mass hybrid 正向/反向长测尚未执行；
- peak RSS、SQLite 最终体积、50k→100k scaling、预加载后 reader/provider I/O delta 尚未冻结；
- 1 worker / 4 workers 的最终 A4 大矩阵和跨平台轨迹差异量化尚未完成；
- 未宣称 Trajecta PV 与 FLEXPART `calcpv` 三维场数值等价；
- 未修改 `E:\flexpart\origo-validation-v1.json`；
- 未 commit / 未 push；
- **不宣称 M4 完成**。
