# Trajecta M6-A1 交付报告

日期：2026-08-11

结论：**M6-A1 passed。强类型物理配置、首个生产物理切片、边界层 Langevin、SQLite v2 和过程结果查询已经闭合。当前停在 A1，不进入 A2。**

## 1. 本轮范围

M6-A1 完成以下纵向切片：

- 将 `physics` 接入 Case 的解析、展开、验证和 resolved document；
- 建立强类型物种状态，区分 forward 质量与 backward 伴随权重；
- 在 `trajecta-core` 建立单一物理执行管线和共同子步；
- 实现边界层 Thomson/Hanna Langevin 过程及对流边界层双高斯垂直速度；
- 将物理状态接入粒子 SoA、生产 Runner、边界处理和 provenance；
- 将生产结果升级为 SQLite `user_version = 2`；
- 实现过程汇总、事件公共结构、full verify 和 `result processes`；
- 建立真实四帧 CFSR、w1/w4、M5 纯平流连续性和外部实验验证。

后续阶段的模块仍会在构建 Runner 时返回稳定的阶段不可用诊断。A1 没有实现中尺度 Markov、地形修正、深对流、水汽交换、沉降、化学、排放或检查点恢复。

## 2. 公开配置

### 2.1 `physics`

Case 支持 A0 冻结的选择结构：

```yaml
physics:
  preset: water_vapor_tracking
  remove:
    - mesoscale_markov
  overrides:
    - model: boundary_layer_langevin
      maximum_substep: 30 s
  modules:
    - model: first_order_decay
      order: 8
```

解析和归一化遵循固定顺序：

1. 展开 preset；
2. 应用 `remove`；
3. 稀疏覆盖已有模块；
4. 追加新模块；
5. 检查唯一性、连续顺序和依赖；
6. 将完整模块列表写入 resolved Case。

旧 `enabled`、字符串参数表、重复模块、部分显式顺序和依赖倒置均返回带字段路径的诊断。未配置 `physics` 的 Case 继续使用纯平流。

### 2.2 物种

`SubstanceSpec` 现有三个强类型分支：

| 类型 | A1 状态 | 已冻结字段 |
|---|---|---|
| water vapor | 生产接线 | identity 与逐粒子质量/伴随状态 |
| gas | 配置和 schema | 摩尔质量、Henry 常数、表面反应性、半衰期、OH 参数 |
| aerosol | 配置和 schema | 材料密度、截断对数正态粒径分布 |

气溶胶粒径范围固定为 `0.01–100 µm`。粒径抽样和相关过程从 A5 开始接入生产执行。

## 3. 物理执行管线

### 3.1 共同子步

生产 Runner 在每个宏步内建立统一的有符号子步分区。分区同时满足 Case 时间步、模块 `maximum_substep` 和边界层相关时间限制。forward 与 backward 共用相同的绝对子步边界，方向只改变有符号时间和需要反向的空间 well-mixed 漂移。

物理速度在中点运动积分前合成。边界层过程分别生成 midpoint 与 endpoint 状态；存活粒子提交 endpoint，步内终止粒子保留 midpoint 状态。地面反射会按本子步碰撞次数的奇偶性映射持久垂直随机速度，水平分量保持不变。

### 3.2 确定性随机数

过程随机键包含：

```text
seed
particle stable ID
module stable ID
macro step
substep
sampling dimension
draw index
```

实现使用 Philox4x32-10 counter RNG。出生、midpoint 和 endpoint 使用互不重叠的固定 draw index；线程调度和 chunk 顺序不进入随机键。

### 3.3 阶段边界

当前生产管线只执行 `boundary_layer_langevin`。预设中属于 A2–A7 的模块可以被解析和解析后展示；若运行仍包含这些模块，Runner 在首次粒子执行前返回：

```text
physics.module_stage_not_available
```

没有加入占位执行、静默降级或备用算法。

## 4. 边界层 Langevin

### 4.1 生产定义

稳定和中性条件使用 Hanna 速度方差与 Lagrangian 时间尺度。对流条件使用二阶矩、三阶矩构造的双高斯垂直速度 PDF。水平速度采用精确 OU 更新；非高斯垂直速度使用局地 PDF 漂移、well-mixed 空间漂移和确定性 counter RNG 创新量。

CFSR 没有直接提供 Monin–Obukhov 长度时，通用查询路径复用 met 层已有 surface-exchange 推导。推导使用相同帧、相同地面通量和近地层字段，并保留字段 provenance。

### 4.2 尾部与边界稳定性

外部实验复验期间关闭了三处边界问题：

- 地面和混合层顶附近使用有限的内侧梯度模板；
- 地面反射同步映射持久垂直随机速度；
- 双高斯 PDF 与截断一阶矩在对数域计算。

非高斯空间项采用局地尾矩/PDF 比值与尾矩对数空间梯度。该形式与 well-mixed 方程一致，避免用相隔数米的两个极端尾值直接相减。新增回归覆盖 PDF 普通浮点密度已经下溢的 `±100σ` 尾部，以及曾产生失稳的 25.85 m/s 垂直速度状态。

## 5. 结果与查询

### 5.1 SQLite v2

生产输出使用 SQLite `user_version = 2`。公共结构包含：

- forward `particle_mass`；
- backward `particle_adjoint`；
- 粒子物理状态列；
- `process_summary`；
- `process_event`；
- A0 冻结的水汽、沉降、化学、排放和深对流明细表。

A1 的边界层过程属于连续随机运动，只写粒子状态和零残差汇总，不制造离散过程事件。full verify 检查方向字段、事件—汇总关系、连续/离散事件策略以及 forward 质量或 backward 伴随闭合。

### 5.2 `result processes`

新增命令：

```text
trajecta result processes RESULT \
  [--particle-id ID] \
  [--module ID] \
  [--substance ID] \
  [--start UTC] [--end UTC] \
  [--events] [--max-records N]
```

默认返回按模块和物种排序的闭合汇总。`--events` 流式读取公共事件；`--max-records` 只与事件模式组合。human、JSON 和 JSONL 均使用稳定输出顺序。SQLite v1 结果返回 `result.process_schema_unavailable`，不会伪造空的 v2 过程结果。

## 6. 科学验证

### 6.1 OU 统计

正式中性 OU 门使用 8 个独立 seed，每个 seed 1,000,000 个样本。全部 seed 通过 A0 冻结门：

```text
|mean| <= 0.02 sigma
relative variance error <= 0.03
absolute lag-one correlation error <= 0.02
```

报告输出保留原始一阶、二阶和交叉矩，并给出双侧 95% 区间。

### 6.2 非均匀湍流 well-mixed 门

正式联合分布回归在 Ubuntu 24.04 release 构建中运行：

```text
particles: 8,192
substeps:  400
result:    passed
runner:    21.72 s
```

初始高度按空气密度分布采样，局地垂直速度按对应 Hanna PDF 采样。完成后同时检查八个高度带的粒子数、归一化垂直速度均值和方差。日常测试另有非均匀方差制造解，锁定完整 Thomson 系数。

### 6.3 Willis–Deardorff 对流边界层实验

原始 Willis–Deardorff 1976 论文 DOI：

```text
10.1002/qj.49710243212
```

原文没有公开仓储全文。实验点取自 NCAR 公开的权威复绘：

```text
Weil et al. (2012)
DOI:       10.1007/s10546-012-9704-y
PDF:       https://www2.mmm.ucar.edu/people/sullivan/talks/papers/weil_blm.pdf
PDF SHA:   38d084754c806ec6772aeca44d901115a1faa72ae0214371981d47b57e9bb61f
figure:    Figure 4, PDF page 12
```

数字化资产包含 6 个无量纲时间剖面和 71 个实验点。标记由 PDF 内嵌 glyph 的 bounding box 提取，资产保留原始 PDF 坐标、图框、轴变换和 marker bbox；图例标记不计入实验点。

```text
testdata/M6_WILLIS_DEARDORFF_CBL.v1.json
SHA-256: f9813ab8d1c155724e5efcaa1a5a64aa2e00dc67747ddf45801bab72d11cea6e

tools/digitize_m6_wd76_cbl.py
SHA-256: 02b57405f0bf12a24fade2312bf8ebfae85295820041c9e4c56491aedb539abc
```

Ubuntu 24.04 正式结果：

```text
particles:        8,192
observations:     71
normalized bias: -0.022741018311444865
normalized RMSE:  0.29782655009824394
gate:             |bias| <= 0.20, RMSE <= 0.30
result:           passed
```

六个剖面的 RMSE 分别为：

```text
X=0.12  0.03432014122608928
X=0.25  0.04914707233162787
X=0.38  0.15427220357382485
X=0.50  0.26889332406975436
X=1.55  0.5000680980837627
X=2.95  0.219575760511782
```

A0 冻结门作用于完整 71 点集合。各剖面统计同时保留，便于后续模型阶段观察误差分布。

## 7. 生产与兼容性验证

### 7.1 真实 CFSR

四帧 `pgbl00` CFSR fixture 通过 production Runner、资料锁、Rust reader、物理查询、SQLite v2、provenance 和 full verify。显式设置 `TRAJECTA_REQUIRE_M6_A1_FORMAL=1` 后测试通过，未走 fixture 缺失的跳过分支：

```text
cfsr_production_boundary_layer_and_m5_advection_continuity
passed in 23.45 s
```

### 7.2 确定性

相同 Case、seed 和资料下，w1 与 w4 的以下规范化 SHA 逐字节一致：

- scientific content；
- normalized SQLite SQL；
- canonical output。

forward 与 backward 分别使用质量和伴随状态；两种方向的生命周期、过程汇总和 full verify 均通过。

### 7.3 M5 纯平流连续性

未配置 `physics` 的运行保持 M5 平流路径。SQLite v1 与 v2 的公共平流投影一致，冻结 SHA 为：

```text
950e16e5bb6a584ddcaa8ac13da54359a9423a616c863f7d76c829e0bad9a277
```

## 8. 最终门禁

```text
cargo test --offline -p trajecta-core
passed: 211 / ignored: 13

cargo test --offline -p trajecta-met
passed: 200 / ignored: 2

TRAJECTA_REQUIRE_M6_A1_FORMAL=1 \
cargo test --offline -p trajecta-core --test m6_a1_cfsr -- --nocapture
passed

cargo test --release --offline -p trajecta-core \
  formal_willis_deardorff_concentration_profiles --lib -- --ignored --nocapture
passed on Ubuntu 24.04

cargo test --release --offline -p trajecta-core \
  nonuniform_gaussian_turbulence_preserves_the_well_mixed_joint_distribution \
  --lib -- --ignored --nocapture
passed on Ubuntu 24.04

cargo test --offline --workspace
passed in 719.6 s

cargo clippy --offline --workspace --all-targets -- -D warnings
passed

cargo doc --offline --no-deps
passed

cargo fmt --all -- --check
passed

python tools/validate_m6_a0_contracts.py
passed: production_stage=m6_a1, SQLite v2, 12 modules, 4 presets

python tools/validate_m5_a0_contracts.py
passed

python tools/validate_m4_a0_contracts.py
passed

git diff --check -- .
passed
```

## 9. 合同同步

边界层持久状态和随机 draw high-water 接入后，checkpoint v1 example 的实际 SHA 已同步到 A0 交付报告。M6 文件卫生门从固定数量改为明确文件清单，并将 WD76 数字化资产列入允许集合。未知的 `M6_*` 文件仍会使校验失败。

## 10. 边界遵守

- 软件版本保持 `0.1.0-alpha.1`；
- Case `schema_version` 保持 `0`；
- SQLite `user_version` 升至 A0 冻结的 `2`；
- 没有加入旧 physics 格式兼容层、备用算法或快/准双模式；
- 没有实现 `job resume`、检查点发布或自动恢复；
- 没有启动 A2；
- 没有读取或修改 `E:\flexpart\origo-validation-v1.json`；
- 没有使用子代理；
- 没有 commit 或 push。

## 11. 停止点

M6-A1 已完成并停在此处。M6-A2 的中尺度 Markov 与 GMTED2010 地形修正尚未开始。
