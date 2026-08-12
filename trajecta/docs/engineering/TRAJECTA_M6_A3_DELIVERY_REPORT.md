# Trajecta M6-A3 交付报告

日期：2026-08-12

结论：**M6-A3 深对流纵向切片已接入生产执行管线，并通过 Windows 与 Ubuntu 24.04 的正式验证。当前停在 A3；未提交、未推送，也未进入 M6-A4。**

## 1. 本轮范围

M6-A3 完成以下内容：

- 独立实现 Emanuel–Živković-Rothman 深对流柱；
- 从 ERA5 hybrid、ERA5 pressure 和 CFSR pressure 查询完整局地热力柱；
- 将完整对流转移接入共同子步后的生产粒子循环；
- forward 使用守恒转移矩阵，backward 使用转置提议与显式重要性权重；
- 将对流事件写入 SQLite v2，并接入 `result processes`；
- 在 full verify 中检查事件合同、过程闭合和逐粒子方向状态；
- 建立 ARM TWP-ICE 七柱验证资产和独立参考结果；
- 完成三资料族、正反向及 w1/w4 的双平台正式矩阵。

水汽交换、沉降、化学、排放和检查点恢复仍由后续阶段实现。M6-A4 未开始。

## 2. 深对流柱

### 2.1 实现身份

生产算法使用固定标识：

```text
emanuel_zivkovic_rothman_conservative_column/v1
```

实现位于单一物理管线内，没有另建替代执行器，也没有保留旧对流算法。完整处理顺序为：

```text
中点运动 → 边界处理 → 完整深对流转移
```

对流在完整共同子步上应用一次。管线中没有半步对流，也没有 fast/accurate 双模式。

### 2.2 柱诊断与转移矩阵

局地热力柱包含：

- 压力；
- 几何高度；
- 温度；
- 比湿；
- 地形高度；
- 地面气压；
- 垂直有效性。

缺层、断裂柱或非有限热力值会返回明确错误。运行期不会用零值或中性状态填补缺失层。

柱诊断计算云底质量通量、卷入与脱混、上升质量通量、补偿下沉和降水下沉气流。各层间质量交换组成非负连续时间生成矩阵。随后使用 uniformization 积分一个完整共同子步，得到列随机转移矩阵 `K`。

矩阵发布前检查：

```text
negative probability tolerance = 1e-14
column-sum tolerance           = 1e-12
Poisson tail tolerance         = 1e-15
```

零对流柱精确返回单位算子。非零柱要求所有概率有限且非负，每个源列之和在容差内等于 1。

### 2.3 Forward 与 backward

Forward 粒子直接从 `K[:, source]` 抽取目标层，质量保持不变。

Backward 粒子从转置行对应的归一化提议分布抽样，并把该行和作为重要性权重。粒子保存伴随权重，结果中使用 `adjoint_weight` 和 `importance_weight`；这些量没有标为质量。

解析点积测试检查：

```text
<K x, y> = <x, K^T y>
```

百万样本测试检查 backward 抽样估计的无偏性。

### 2.4 确定性随机键

深对流抽样使用 `CounterRng`。固定随机身份为：

```text
module ID         = 0x7be2949e34aeca8d
sampling dimension = 96
draw index         = 0
```

模块 ID 等于：

```text
SHA-256("deep_convection_column")[0..8]
```

完整随机键还包含 seed、粒子稳定 ID、宏步和共同子步。线程调度顺序不进入随机键，因此 w1 与 w4 可产生一致的规范化科学结果。

## 3. 生产查询与执行接线

气象查询新增完整对流柱的 prepare/execute 路径。prepare 固定时间窗、水平支持、垂直柱和 caller-order permutation；execute 阶段不读取源文件。

三类资料均通过现有生产 reader 提供柱几何：

| 资料族 | 垂直结构 | M6-A3 状态 |
|---|---|---|
| ERA5 hybrid | hybrid 层与地面气压 | passed |
| ERA5 pressure | pressure levels | passed |
| CFSR pressure | pressure levels | passed |

粒子完成中点运动和边界处理后，以当前位置和共同子步中点时间查询热力柱。选中的层转移随后更新粒子高度。Backward 同时更新所有物种的伴随权重。

同一粒子、物种和宏步内的多次子步转移会按物理顺序合并。Forward 保存首个源层和最终目标层；backward 保存最终追溯源层和初始目标层。转移概率及重要性权重按算子顺序相乘。

## 4. SQLite、查询与结果验证

SQLite `user_version` 保持 2。M6-A3 使用 A0 已冻结的公共表：

```text
process_event
process_summary
convection_event
```

对流明细包含：

- 源层和目标层；
- 合并后的转移概率；
- backward 重要性权重；
- 转移矩阵最大列闭合残差。

Forward 的过程质量增量为零。Backward 的生存乘子和源敏感度保持各自语义，重要性权重单独记录。事件和汇总在同一 SQLite 事务内提交。

`result processes` 已覆盖以下路径：

- 默认模块/物种汇总；
- 按粒子、模块、物种和 UTC 范围筛选；
- `--events` 事件读取；
- human、JSON 和 JSONL 输出；
- forward 质量闭合；
- backward 按事件顺序递推的伴随闭合。

full verify 新增逐粒子对流审计。它从初始方向状态依次应用事件权重，并与最终 `particle_mass` 或 `particle_adjoint` 对照。事件类型、模块身份、方向字段、列残差和明细表连接也会一并检查。

## 5. 冻结验证资产

M6-A3 资产位于 `testdata/m6-a3/`，没有改变 A0 顶层冻结清单。

| 文件 | SHA-256 |
|---|---|
| `M6_ARM_TWP_ICE_COLUMN.schema.json` | `24bd510a0b51272c2ef47508d918df681e4779005f05aa26ba28dd81b92e428a` |
| `M6_ARM_TWP_ICE_COLUMN.v1.json` | `144ac0a0e0f4043df5fb5b1fe57ddebe57e2c65c9cc5df6b33c5bf727e86b259` |
| `tools/derive_m6_twpice_column.py` | `b09f38d2687c518f1dd2004049e4dd4a79293c7d9cef085a907ade5089fcca0e` |

验证集包含 ARM TWP-ICE event C 的七个热力柱。每柱检查：

- 柱间质量通量闭合；
- 转移矩阵列随机性；
- forward/adjoint 点积；
- 云底质量通量；
- 上升和下沉界面质量通量。

综合门使用 A0 冻结阈值：

```text
|normalized bias| <= 0.20
normalized RMSE   <= 0.30
```

独立参考实现取自：

```text
repository: https://github.com/climlab/climlab-emanuel-convection
commit:     c6cc8a0dcbfe32d79ce82b3e270e6af834558f28
summary SHA-256:
78bea5ea6afa91e15db01d6b4faf69ad0b81efb3dd2de9690d30d7ce64f7ddd1
```

生产实现根据论文独立表达，没有移植参考仓库源码。

## 6. 双平台正式验证

### 6.1 Windows x86_64

最新生产源码的正式 A3 验证结果：

```text
deep-convection core:          4/4 passed
ARM TWP-ICE seven columns:     passed
CFSR production artifact:     passed in 156.24 s
CLI process query:             passed in   1.63 s
three-family 12-cell matrix:   passed in 624.18 s
```

12 格矩阵覆盖：

```text
ERA5 hybrid / ERA5 pressure / CFSR pressure
× forward / backward
× w1 / w4
```

每格使用生产 Runner、真实资料锁、SQLite v2、process events、provenance 和 full verify。各资料族同方向的 w1/w4 规范化 content、SQL 和 canonical-output digest 完全一致。

### 6.2 Ubuntu 24.04 x86_64

最后一次随机模块 ID 修正后，Ubuntu 24.04 使用最新源码重新完成正式验证：

```text
deep-convection core:          4/4 passed; test body 10.01 s
ARM TWP-ICE seven columns:     passed; test body  0.06 s
CFSR production artifact:     passed; test body 190.35 s
CLI process query:             passed; test body  2.34 s
three-family 12-cell matrix:   passed; test body 784.09 s
```

Ubuntu 测试在沙盒外的 `Ubuntu-24.04` 发行版执行。Cargo target 使用 `/tmp/trajecta-m6-a3-target`，没有混用 Windows 构建产物。

## 7. 回归与源码门禁

Windows 全仓回归结果：

```text
cargo test --offline --workspace --quiet
624 passed / 15 ignored
elapsed: 1,478.1 s
```

该次 workspace 运行发生在长测入口收紧之前，本机存在真实 fixture，因此同时执行了 A2 与 A3 的真实资料矩阵。生产代码和正式矩阵均通过。

其余门禁：

```text
cargo clippy --offline --workspace --all-targets -- -D warnings
passed

cargo doc --offline --no-deps
passed

cargo fmt --all -- --check
passed

python tools/validate_m4_a0_contracts.py
passed

python tools/validate_m5_a0_contracts.py
passed

python tools/validate_m6_a0_contracts.py
passed: production_stage=m6_a2, SQLite v2, 12 modules, 4 presets

python tools/validate_m6_a3_contracts.py
passed: production_stage=m6_a3, seven TWP-ICE columns, SQLite v2

python -m py_compile \
  tools/derive_m6_twpice_column.py \
  tools/validate_m6_a3_contracts.py
passed

git diff --check
passed
```

A0 validator 保持冻结字节：

```text
tools/validate_m6_a0_contracts.py
SHA-256: 51e026fcb51f233852a7ca340c2b2f8997bc2a0bb9e1d8fa2e7d68bef53050e4
```

A3 使用独立 validator：

```text
tools/validate_m6_a3_contracts.py
SHA-256: 6f5a2237397b01dd4c3e95546f06cadaff5036391b4cf02f1c930e10f27b1949
```

## 8. 长测入口调整

真实资料矩阵此前会在本机存在 fixture 时进入普通 workspace 测试。A3 收口时已将入口改为显式 formal 控制：

```text
M6-A2: TRAJECTA_REQUIRE_M6_A2_FORMAL=1
M6-A3: TRAJECTA_M6_FORMAL=1
```

普通 `cargo test --workspace` 现在直接跳过 A2/A3 的真实资料矩阵、A3 生产查询产物生成和对应 CLI 查询。设置正式变量后，测试会执行完整内容；缺少 fixture 时直接失败。

该调整只改变测试调度条件。生产实现、正式案例参数、资料锁、数值结果和验收阈值没有变化。调整后的普通入口已快速确认：core integration 3/3、CLI integration 1/1，均在 0.00 秒内完成跳过分支。之后没有再次运行长矩阵。

## 9. 边界遵守

- 软件版本保持 `0.1.0-alpha.1`；
- Case `schema_version` 保持 `0`；
- SQLite `user_version` 保持 `2`；
- 纯平流及 M6-A1/A2 的生产路径继续通过回归；
- 没有增加旧算法兜底或快/准双路径；
- 没有修改 `E:\flexpart\origo-validation-v1.json`，其 SHA-256 仍为 `d6e79bb88bbedf224f8f55745caf3995b6528a9b63cb679ed3daca99ac1253b0`；
- 没有使用子代理；
- 没有 commit 或 push；
- M6-A4 未开始。

## 10. 停止点

M6-A3 已完成本地实现和双平台正式验证。工作树保留待审阅改动，当前停在 A3，等待下一步指令。
