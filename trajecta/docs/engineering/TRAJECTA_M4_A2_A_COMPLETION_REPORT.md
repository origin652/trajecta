# Trajecta M4-A2：A 完成裁决报告

状态：**M4-A2 complete；M4 整体未完成**
日期：2026-07-24
仓库：`E:\flexpart\trajecta`
基线：`main @ 18664f112d7d7cd3155dd10c71b7fecdc7d4ca17`
提交状态：**未 commit / 未 push**

> 责任声明：当前及后续阶段不再有 B、C 模型，M4-A2 的实现、根因诊断、Windows/WSL
> 实跑、artifact 汇总、合同更新、报告和验收均由 A 独立完成。旧
> `TRAJECTA_M4_A2_B_EXECUTION_REPORT.md` 只保留为修复前 0/12 失败的历史证据，不能被解释为
> 仍可向 B/C 返工或补跑。

## 1. 裁决

M4-A2 的书面退出条件是：ERA5 pressure、ERA5 hybrid、CFSR pressure 三族 air-mass
domain-fill，正向与反向，在 Windows 和 WSL 上均以 10,000 粒子实际运行，并通过逐步/最终质量守恒
hard gate。

本轮冻结证据满足该条件：

| 项 | 结果 |
|---|---|
| Windows 正式矩阵 | **6/6 passed** |
| WSL Ubuntu-24.04 正式矩阵 | **6/6 passed** |
| 总计 | **12/12 passed；0 failed；0 external_blocked** |
| 每格粒子数 | **10,000** |
| 每格模拟时长 | **600 s** |
| 每格数值步 | **2 × 300 s** |
| RunOutcome / manifest | **Complete / complete** |
| abnormal termination | **0** |
| SQLite integrity | **ok** |
| provenance bundle | **存在且通过校验** |
| 质量账本 | **逐步与最终 hard gate 全部通过** |

因此 A 正式裁决：**M4-A2 完成**。该裁决不扩展为 M4 完成；M4-A3 与 M4-A4 仍未完成。

## 2. 修复前失败与真实根因

### 2.1 ERA5 pressure：近地 mixed route 使用了不完整 transport 层

修复前 10k 正向/反向分别产生 16/12 个 `invalid_meteorology`，Windows 与 WSL 计数一致。
根因不是随机性或粒子密度，而是两个时间端点的 mixed route 选中了结构上最低、但 U/V/W 支撑不完整
的 pressure level。

修复：`lowest_complete_transport_anchor/v1` 从底向上寻找同一层，要求 U、V 与 geometric W 在相关
时间端点和冻结时间插值上同时完整；只有该锚点以下的合法近地缺口走 surface route。正式 10k
重跑后两方向 abnormal 均为 0。

### 2.2 ERA5 hybrid：米级大圆路径的角度与切向量数值失稳

根因是连续边界路径使用 `acos(dot)` 求极短弧角，并用
`(b-cos(omega)*a)/sin(omega)` 构造切向量；米级路径中两式分别丢失有效位和发生灾难性相消，最终
触发 `Boundary(RootFindingFailed)`。

修复：`great_circle_atan2_oriented_basis/v1` 使用：

```text
omega = atan2(norm(cross(a,b)), dot(a,b))
n = normalize(cross(a,b))
t = normalize(cross(n,a))
```

单点冻结回放与正式 hybrid 10k 双平台双方向均通过。

### 2.3 CFSR：transport-floor pressure 的 1 ULP 越界

原 `exp(lerp(ln p))` 在退化锚点 `p_lowest == p_surface == 100000 Pa` 上往返，使结果高于地面压约
1 ULP，domain-fill 把合法列误判为 `InvalidSample`。

修复：`dry_air_transport_floor_log_pressure/v1` 改为压力比幂插值：

```text
p_floor = p_surface * (p_lowest / p_surface)^height_fraction
```

只允许解析舍入界内钳回物理范围，真实越界仍 hard fail。冻结 CFSR 退化锚点单测和正式矩阵通过。

### 2.4 CFSR：动量 roughness 被错误用于拒绝 2 m 标量锚点

6 个出生即失败点的 CFSR 动量粗糙度约为 `2.08–2.70 m`。旧标量剖面要求 2 m 锚点高于动量
`z0`，因而把合法的 2 m 温度/湿度观测错误判为无定义。

修复：`two_metre_anchored_businger_dyer_scalar/v1` 使用锚点差公式：

```text
S(z) = S(2 m) + s*/k * [ln(z/2 m) - psi_h(z/L) + psi_h(2 m/L)]
```

动量 `z0` 在标量差分中严格相消。6 点真实重放和正式 CFSR 10k 双平台双方向均通过。

### 2.5 物理输入双轨语义

- raw finite negative specific humidity 保留用于查询与审计；density、hydrostatic、surface layer 和
  domain-fill 等物理消费者在每个源时次先执行
  `specific_humidity_nonnegative_projection/v1`，再做时间插值；
- raw exact-zero roughness 保留；对数 surface-layer 消费者仅将 exact zero 投影为冻结的
  `1e-4 m`，负值和非有限值仍失败；
- 10 m 风采用向量锚定 Businger-Dyer 比例式，不以 `u*` 重复加法约束，也不静默截负风速。

这些规则均已写入 A0 科学合同、机器数值合同与 provenance transform。

## 3. 冻结科学身份

本轮新增并由机器合同测试锁定：

| 含义 | algorithm id |
|---|---|
| 稳健短弧角与有向基底 | `great_circle_atan2_oriented_basis/v1` |
| 初始粒子随完整 transport floor 水平平移 | `dry_air_transport_floor_following_horizontal_relocation/v1` |
| transport-floor 压力比插值 | `dry_air_transport_floor_log_pressure/v1` |
| 2 m 标量锚定相似度剖面 | `two_metre_anchored_businger_dyer_scalar/v1` |
| 最低联合 U/V/W transport 锚点 | `lowest_complete_transport_anchor/v1` |

对应冻结文件：

- `crates/trajecta-core/src/science.rs`
- `crates/trajecta-met/src/science.rs`
- `testdata/M4_NUMERICAL_CONTRACT.v1.json`
- `crates/trajecta-core/tests/m4_contracts.rs`
- `docs/engineering/TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`

## 4. 正式 12 格结果

证据根目录：`target/m4-a2/a-formal/`

| family | direction | Windows | WSL | max ledger fraction（两平台一致） |
|---|---|---|---|---:|
| ERA5 pressure | forward | passed | passed | 0.00017259189701175287 |
| ERA5 pressure | backward | passed | passed | 0.00017259639763478045 |
| ERA5 hybrid | forward | passed | passed | 0.0001726591345821083 |
| ERA5 hybrid | backward | passed | passed | 0.00017237508709534374 |
| CFSR pressure | forward | passed | passed | 0.0 |
| CFSR pressure | backward | passed | passed | 0.0 |

机器汇总：

- `target/m4-a2/a-formal/summary/windows-formal10000_summary.json`
- `target/m4-a2/a-formal/summary/wsl-formal10000_summary.json`
- `target/m4-a2/a-formal/summary/ALL_CELLS_SUMMARY.json`

WSL 首次在受限沙箱内以 `E_ACCESSDENIED` 失败；获得沙箱外执行授权后，Ubuntu-24.04 的 6 个真实
cell 全部通过。因此该事件是执行环境权限问题，不是科学、资料或 Linux 工具链失败，最终汇总中的
`external_blocked` 为 0。

## 5. 跨平台边界

Windows 与 WSL 对每格的粒子数、数值步、abnormal 计数和质量账本数值一致，但
`particles.sqlite`、provenance bundle 和 normalized output digest 并不相同。

M4-A2 冻结退出条件要求三族、双方向、双平台和守恒通过，不要求跨平台 bitwise 相同。因此 digest
差异不阻断 A2，但本报告**不宣称跨平台 bitwise 一致或数值等价已经量化**。跨平台轨迹差异、native
对比和可接受误差裁决保留给 M4-A4。

## 6. 本轮最终门禁

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | OK |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | OK |
| `cargo test --workspace --offline` | **387 passed / 5 ignored** |
| `cargo doc --workspace --no-deps --offline` | OK |
| `python -m py_compile tools/validate_m4_a0_contracts.py tools/run_m4_a2_real_matrix.py` | OK |
| `python tools/validate_m4_a0_contracts.py` | passed |
| `git diff --check` | OK |
| `real_hybrid_short_path_boundary_replay`（release） | passed |
| `real_cfsr_domain_fill_invalid_meteorology_replay`（release） | passed |
| 正式 10k Windows/WSL 矩阵 | **12/12 passed** |

5 个 ignored 为 3 个显式 M4-A2 真实长链入口和 2 个旧 M3 百万点性能测试；正式 A2 三族 12 格已由
独立编排器实际执行并冻结在上述 artifact 目录，不以 ignored 状态冒充通过。

## 7. 未完成与禁止扩大声明

- M4-A3 ozone domain-fill 尚未完成；
- M4-A4 的 native/Windows/WSL 数值差异、性能、RSS、SQLite 体积、I/O 与 10 万粒子长测尚未完成；
- 尚未对跨平台不同 digest 作科学容差裁决；
- 未修改 `E:\flexpart\origo-validation-v1.json`；
- 未 commit / 未 push；
- **不宣称 M4 完成**。
