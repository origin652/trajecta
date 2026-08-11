# Trajecta M6-A0 科学、数值与恢复合同

状态：**M6-A0 authority contract**

冻结日期：2026-08-10

软件版本：`0.1.0-alpha.1`

本合同确定 M6 的公开配置、物种状态、物理过程、执行顺序、随机数身份、辅助资料、过程产物、检查点、恢复规则、验证资产和验收阈值。M6-A0 只建立合同，不实现任何 M6 物理模块，也不改变现有纯平流运行。

可执行校验入口：

```text
python tools/validate_m6_a0_contracts.py
```

## 1. 权威顺序和版本边界

发生冲突时按以下顺序裁决：

1. 本文件；
2. `testdata/M6_PHYSICS_CONTRACT.v1.json`；
3. `testdata/M6_TOLERANCES.v1.json`；
4. `testdata/M6_VALIDATION_ASSETS.v1.json`；
5. 三项产物 Schema 和 `M6_SQLITE_SCHEMA.v2.sql`；
6. M6 总体计划；
7. 后续阶段的实现报告。

版本边界如下：

| 项目 | 冻结值 |
|---|---|
| 软件版本 | `0.1.0-alpha.1` |
| Case `schema_version` | `0` |
| 粒子结果 SQLite `user_version` | `2` |
| 物理合同 | `trajecta/physics/m6/v1` |
| 容差注册表 | `trajecta/tolerances/m6/v1` |
| 检查点 | `trajecta.checkpoint/v1` |
| 过程查询 | `trajecta.process-query/v1` |
| 粒子结果数据库 | `trajecta.particle-state-sqlite/v2` |

SQLite 升级用于公开过程事件和伴随状态。检查点与过程查询采用各自的 v1 合同。它们不会引出新的软件版本、Case 版本或平行执行器。

## 2. Case 中的物理配置

### 2.1 最终形状

`physics` 从模块数组改为选择结构：

```yaml
physics:
  preset: water_vapor_tracking
  remove: []
  overrides:
    - model: boundary_layer_langevin
      maximum_substep: 30 s
  modules: []
```

四个键均参与规范化输出。输入可省略空数组，resolved Case 必须把它们补全。物理量需要显式单位，并在解析后写成 SI 规范值。带量纲的裸数字属于配置错误。

没有 `physics` 时，执行现有纯平流路径。空的 `modules` 与缺少 `physics` 含义不同：前者仍会展开预设，后者不启用 M6 模块。

旧形状中的以下字段不再接受：

```yaml
enabled: true
parameters:
  key: value
```

解析器应在原字段路径返回诊断。M6 不保留旧解析器、转换器或运行期兼容分支。

### 2.2 合并算法

规范化过程严格执行六步：

1. 按预设顺序展开模块；
2. 执行 `remove`；
3. 将 `overrides` 的稀疏字段写入仍然存在的模块；
4. 按声明顺序追加 `modules`；
5. 验证模块唯一性、参数范围和资料能力；
6. 验证顺序与依赖，并写入 resolved Case 和 provenance。

`remove` 中的未知模块、`overrides` 中不存在的模块、重复追加和未知字段均为错误。删除依赖目标后留下依赖者也会失败。

默认顺序来自预设。如果出现任一显式 `order`，最终模块集合中的每个模块都必须给出 `order`。序号从 0 开始，连续且唯一；依赖目标必须排在依赖者之前。解析器不会用名称排序来修补缺口。

### 2.3 预设

| 预设 | 默认模块顺序 | 物种 |
|---|---|---|
| `water_vapor_tracking` | 地形修正 → 边界层 Langevin → 中尺度 Markov → 深对流 → 水汽交换 | water vapor |
| `gas_transport` | 地形修正 → 边界层 Langevin → 中尺度 Markov → 深对流 → 干沉降 → 湿清除 → 半衰期 → OH 氧化 | gas |
| `aerosol_transport` | 地形修正 → 边界层 Langevin → 中尺度 Markov → 重力沉降 → 深对流 → 干沉降 → 湿清除 | aerosol |
| `buoyant_release` | 排放时间剖面 → 烟羽抬升 → 地形修正 → 边界层 Langevin → 中尺度 Markov → 深对流 | water vapor、gas 或 aerosol |

预设中的系数是产品合同的一部分。用户只能覆盖 `M6_PHYSICS_CONTRACT.v1.json` 为相应模块列出的字段。

## 3. 物种和粒子状态

### 3.1 强类型物种

所有物种具有 `id`、`display_name` 和 `kind`。`kind` 决定后续字段，未知字段和跨类型字段会被拒绝。

水汽：

```yaml
- id: water
  display_name: Water vapor
  kind: water_vapor
```

气体：

```yaml
- id: sulfur_dioxide
  display_name: Sulfur dioxide
  kind: gas
  molar_mass: 64.066 g/mol
  henry_constant: 1.23e-3 mol/(m3 Pa)
  surface_reactivity: 0.1
  half_life: 2 h
  oh_reaction:
    pre_exponential: 1.0e-18 m3/(molecule s)
    temperature_exponent: 0.0
    activation_temperature: 0 K
```

`molar_mass`、`henry_constant` 和 `surface_reactivity` 是气体必填项。`half_life` 与 `oh_reaction` 可省略；对应模块只处理声明了该过程的物种。一个运行中至少要有一个物种被每个启用的损失模块选中，否则配置失败，避免空模块悄然通过。

气溶胶：

```yaml
- id: sulfate
  display_name: Sulfate aerosol
  kind: aerosol
  material_density: 1770 kg/m3
  shape: sphere
  diameter:
    distribution: truncated_lognormal_mass_basis
    geometric_mean: 0.5 um
    geometric_standard_deviation: 1.8
    minimum: 0.01 um
    maximum: 100 um
```

正式直径区间是 `0.01–100 µm`。几何标准差必须大于 1。分布以质量为权重进行截断归一化。每个粒子出生时由 `CounterRng` 采样一次直径，随后保持不变；检查点需要保存该值。

### 3.2 forward 与 backward

forward 粒子保存每个物种的质量。水汽质量由干空气质量和比湿定义：

```text
m_v = m_dry q / (1 - q),  0 <= q < 1
```

backward 粒子保存每个物种的伴随权重。过程产物使用 `adjoint_weight`、`source_sensitivity` 和 `importance_weight`。数据库、命令输出和文档不得把这些量标为质量。

## 4. 统一执行管线

### 4.1 共同子步

每个宏步先收集以下稳定性上限：

- 输送中点误差和网格跨越；
- Langevin 相关时间；
- 中尺度 OU 相关时间；
- 对流柱更新时间；
- 重力沉降的垂直跨层；
- 局地质量过程的最大指数损失；
- 用户允许覆盖的 `maximum_substep`。

最小上限转换为整数纳秒。宏步被划分为等长基础子步，最后一个子步只吸收整数纳秒余数。整个粒子批次共用同一组边界，线程不能为单个粒子另选步长。任何上限非有限、非正，或取整后小于 1 ns 时，运行以 `substep_underflow` 类科学错误结束。

### 4.2 分裂顺序

每个共同子步执行：

```text
排放计划和出生
→ 局地过程半步
→ 合成中点速度并移动
→ 深对流柱转移
→ 局地过程半步
→ 合并过程事件
```

局地过程采用对称二阶 Strang 分裂。已解析风、边界层随机速度、中尺度速度和气溶胶重力沉降速度在同一个中点运动积分中相加。代码中只保留这一条生产路径，不建立 fast/accurate 双实现。

### 4.3 随机数

生成器固定为 `Philox4x32-10`。随机键为：

```text
(seed, particle_stable_id, module_id, macro_step,
 substep, sampling_dimension, draw_index)
```

模块不得消费共享的顺序 RNG。抽样维度由机器合同固定，新增维度需要修改合同。正态抽样使用开放区间均匀数和固定配对的 Box–Muller 变换。出生期抽样使用 `macro_step=0, substep=0`。

相同 seed、Case、资料锁和 source identity 在 w1 与 wN 下必须产生逐字节一致的规范化科学结果。调度顺序、线程完成顺序和恢复次数不参与随机键。

## 5. 输送修正和随机运动

### 5.1 边界层 Langevin

模块采用 Thomson well-mixed 框架。速度过程写为 Itô 系统：

```text
du_i = a_i(x, u, t) dt + b_ij(x, t) dW_j
```

扩散张量和漂移需要使指定的 Eulerian 速度概率密度成为 Fokker–Planck 方程的稳态解。离散实现不得省略非均匀湍流中的 well-mixed 漂移项。

稳定和中性边界层使用 Hanna 关系计算三个速度标准差及 Lagrangian 时间尺度。对流边界层使用偏斜的双高斯垂直速度密度，并以二阶矩、三阶矩和空气密度梯度构造 well-mixed 漂移。稳定性过渡需要连续，速度方差和时间尺度保持正值。默认最大子步为 30 s。

验证包括：常密度均匀湍流的稳态正态分布、非均匀方差下的 well-mixed 分布、OU 精确矩、对流边界层偏度和 Willis–Deardorff 浓度分布。

实现依据 Thomson、Luhar–Britter 和 Cassiani 等公开论文独立推导。FLEXPART GPL 源码仅用于检查概念和参数名称，不复制控制流或逐行表达。

### 5.2 三维中尺度 Markov 扰动

对每个风分量，在粒子周围的两个时间面、四个水平角点和两个垂直层上计算插值权重。局地加权方差为：

```text
sigma_i^2 = sum_j w_j (u_ij - sum_k w_k u_ik)^2
```

权重非负且总和为 1。缺少任一正式角点时遵守气象查询的有效性规则，不用零值补齐。

相关时间固定为原生气象时间间隔的一半：

```text
tau = 0.5 delta_t_met
r = exp(-delta_t / tau)
u'_next = r u'_old + sqrt(1-r^2) sigma xi
```

三个分量使用独立维度。`sigma=0` 时结果精确为 0。默认最大子步为 300 s。

### 5.3 次网格地形

局地地面高度为：

```text
z_surface = z_met + z_dem(point) - mean_dem(meteorology_cell)
```

`mean_dem` 使用球面面积权重计算。GMTED2010 点高、均值和标准差均以米表示。经度周期和极区单元按气象网格拓扑处理。

地形标准差对混合层的增量为：

```text
delta_h = sigma_z                         for N^2 <= 0
delta_h = min(sigma_z, 2 |V_h| / N)      for N^2 > 0
h_mix_effective = h_mix + delta_h
```

`N` 的数值下限为 `1e-4 s-1`。粒子若落到修正地面以下，沿已经冻结的地面边界策略求交；模块不能直接把高度钳回地面。

## 6. 深对流柱

每个气象柱和原生时间区间运行一次 Emanuel–Živković-Rothman 诊断。输入至少包含压力、温度、比湿、位势高度和地面压力。参数值收录于 `module_constants.deep_convection_column`，其中准平衡系数采用论文标准值 `0.2`。

诊断得到上升流、下沉流、卷入、脱离和补偿下沉质量通量。它们转换为非负生成矩阵，再在共同子步上形成列随机转移矩阵 `K`：

```text
K_ij >= 0
sum_i K_ij = 1
```

forward 列质量按 `m_next = K m` 更新。粒子目标层由 `K` 的对应列确定性抽样。backward 使用 `lambda_prev = K^T lambda_next`；若抽样提议分布不同于转移列，需要写出显式重要性权重。零质量通量生成精确单位矩阵。

负概率超过 `1e-14`、列和偏差超过 `1e-12`、诊断量非有限或垂直列不完整时，运行失败。归一化不能掩盖超限残差。

验证采用解析矩阵、forward/adjoint 点积、柱质量闭合，以及 ARM TWP-ICE 单柱资料。对流实现从论文独立表达，不移植 GPL Fortran。

## 7. 水汽交换

每个气象格点和时间段先按粒子干空气质量计算目标水汽变化：

```text
delta_M_target = sum_p m_dry,p [q_1/(1-q_1) - q_0/(1-q_0)]
```

地表蒸发 `E` 和降水 `P` 按格点球面面积与时间长度积分。分配顺序固定为：

1. 蒸发按格点内干空气质量比例增加水汽；
2. 降水按当前水汽质量比例移除水汽；
3. `delta_M_target - (E - P)` 写为 `unresolved_moisture_tendency`；
4. 正残差按干空气质量分配，负残差按当前水汽质量分配；
5. 记录每个粒子、物种和宏步的合并事件。

每次分配都采用确定性粒子 ID 顺序和补偿求和。最后一个正权重粒子吸收浮点余量，使格点账本与分配总和在合同容差内一致。

当总权重为零、`q` 越界，或任何移除会使质量低于允许的舍入下限时，运行以科学错误结束。模块不会把负值钳为零后继续。

backward 对相同分配和活动集合应用解析 Jacobian 的转置。活动集合、归一化总量和残差进入过程诊断，以支持点积复验。

## 8. 沉降、清除和化学

### 8.1 气溶胶重力沉降

球形粒子的 Cunningham 修正为：

```text
C_c = 1 + (2 lambda / d) [1.257 + 0.4 exp(-1.1 d / (2 lambda))]
```

空气平均自由程按温度和压力缩放，标准值为 `6.6e-8 m`。空气黏度使用 Sutherland 关系。终端速度由浮力修正重力与阻力平衡求解，阻力系数跨 Stokes、过渡和高 Reynolds 数区间连续。直径、密度、空气状态或迭代结果无效时明确失败。

终端下沉速度只加入中点运动。干沉降模块使用不含重力项的表面传输速度，避免重复计算。

### 8.2 干沉降

气体使用 Wesely 类阻力网络：

```text
v_d = 1 / (R_a + R_b + R_c)
```

`R_c` 由 Henry 常数、表面反应性、IGBP 土地覆盖、LAI、季节、温度和积雪状态决定。IGBP 1–17 到 13 类 Wesely 表的映射由机器合同固定。雪水当量达到 1 mm 时采用雪冰类别。

气溶胶使用 Zhang 粒径依赖的准层流和表面收集效率。公开的干沉降速度为湍流、布朗扩散、拦截和惯性碰撞部分；重力速度只由 `gravitational_settling` 提供。表面吸收事件记录实际移除质量或 backward 生存乘子。

### 8.3 湿清除

湿清除区分：

- 云内液态；
- 云内冰态；
- 云内混合相；
- 云下雨；
- 云下雪。

253.15 K 以下采用冰相，273.15 K 以上采用液相，中间按温度线性分配混合相。气体云内吸收使用 Henry 平衡和反应性；气溶胶云内活化及云下碰并采用 Grythe 2017 的分相参数化。雨雪通量分别进入清除率，不用总降水反推相态。

质量按 `m_next = m exp(-Lambda delta_t)` 积分。`Lambda` 非有限或负值时失败。Laakso 观测用于气溶胶云下清除门禁。

### 8.4 半衰期和 OH 氧化

半衰期损失率为：

```text
k_half = ln(2) / t_half
```

OH 反应率为：

```text
k_oh(T) = A (T / 300 K)^n exp(-D/T) [OH]
```

两个过程同时存在时先相加，再进行一次指数积分：

```text
m_next = m exp[-(k_half + k_oh) delta_t]
```

不产生反应产物。backward 使用相同生存乘子的转置标量算子。

Spivakovsky 论文的正确 DOI 为 `10.1029/1999JD901006`。早期计划曾误引一篇森林反射率论文，现已纠正。当前候选 `OH_7lev_agl.dat` 缺少足够的网格、单位、生成链和再分发说明，因此 OH 分支在 M6-A6 前保持 `external_blocked`。运行期不会改用常数 OH 场。

## 9. 排放时间剖面和烟羽抬升

### 9.1 时间剖面

显式时间序列可与月、星期和小时因子相乘：

```text
r(t) = r_series(t) M_month(t) W_weekday(t) H_hour(t)
```

所有因子非负。时区必须显式选择 `UTC` 或 IANA 时区。IANA 日历从 UTC 时刻映射到当地时间，因此夏令时重复小时保留两个不同 UTC 区间，跳过的小时不生成虚构区间。

`normalization` 必须选择：

- `preserve_declared_total`：在声明区间内按积分缩放，使总排放量保持不变；
- `preserve_rate_amplitude`：直接使用组合因子，不再归一化。

宏步内对分段常数时间序列精确积分。事件保存组合因子和计划质量。

### 9.2 统一烟羽柱

烟囱和面源/火源进入同一个一维卷入柱。烟囱提供出口半径、速度、温度和流量；面源提供有效半径、热通量和质量通量。两者转换为初始体积、动量和浮力通量。

柱模型积分质量、动量、浮力和标量守恒式，默认卷入系数为 `0.1`。环境温度、密度和水平风沿柱插值。达到中性浮力、垂直速度耗尽、资料顶或最大允许高度时终止，并把脱离分布转换为出生高度概率。

Kincaid SF6 数据库用于烟囱羽流高度和地面浓度验证。正式归档的 SHA-256 已写入验证资产注册表。

## 10. 辅助资料和许可

四类资料都使用全球共享 DatasetLock，不按 Case 裁剪，也不随 Windows/Linux 软件包重复打包。`data-plan` 报告缺失项；Runner 不下载资料。

| 资料 | 选择 | 本地使用与再分发 | 阶段 |
|---|---|---|---|
| GMTED2010 | 30 arc-second mean 和 standard deviation | USGS 公共领域资料，保留来源说明；软件包不内置 | A2 |
| MCD12C1.061 | 年度 IGBP `Land_Cover_Type_1` | NASA 开放数据，下载需 Earthdata，发布派生物需引用 | A5 |
| MCD15A3H.061 | 2003–2022 LAI 月气候场 | NASA 开放数据，保留 QA、处理链和引用 | A5 |
| Spivakovsky 2000 OH | 月平均三维 OH | 来源链和再分发范围待核实 | A6 blocked |

LAI 气候场接受通过 mandatory QA 的有效反演，按球面面积聚合到 0.05° 全球格点。每月同时保存均值、标准差和有效样本数。处理脚本、输入清单与 SHA 随派生资产发布。

## 11. 过程数据库与查询

SQLite v2 在粒子和轨迹表之外增加：

```text
particle_adjoint
particle_aerosol_property
process_summary
process_event
water_vapor_event
deposition_event
chemistry_event
emission_event
convection_event
```

连续湍流和地形修正只写 `particle_state` 的状态分量与运行汇总，避免逐子步事件膨胀。离散质量、沉降、化学、排放和对流过程写 `process_event`。

事件合并键固定为：

```text
(run_id, particle_id, module_id, substance_id, macro_step)
```

同一键只能有一行。明细表与公共事件表一一对应。full verify 检查：

- forward 初始质量 + 事件增减 = 最终质量；
- backward 初始权重、存活乘子、源敏感度及对流权重与最终伴随状态闭合；
- 明细表方向与公共事件方向一致；
- 每个公共事件恰有一个与 `detail_kind` 对应的明细行；
- continuous-only 模块没有离散事件。

新增命令：

```text
result processes RESULT [filters] [--events] [--max-records N]
```

筛选项包括粒子、模块、物种和 UTC 起止时间。默认输出闭合汇总。`--events` 按物理时间、粒子 ID、模块、物种和事件序号的稳定顺序流式读取。JSON 先写入 create-new spool，成功后输出单一 envelope；JSONL 输出 header、event 和唯一 summary；human 使用相同查询结果。

## 12. 检查点与恢复

### 12.1 发布检查点

默认每经过 1800 s 单调墙钟时间提出一次检查点请求，并在下一个完整宏步边界执行。每个运行只保留最近两代已验证检查点。

检查点内容包括：

- 粒子 SoA 和固定粒径；
- forward 质量或 backward 伴随状态；
- 模块持久状态；
- 物理时钟和下一个宏步；
- 随机键高水位；
- 输出事件高水位和逐粒子样本高水位；
- SQLite backup 与过程事件游标；
- Case、Profile、DatasetLock、source identity 和合同身份。

发布顺序为：同目录临时目录 → 完整写入 → 文件 sync → SQLite integrity/WAL 检查 → SHA 校验 → Schema 校验 → 目录 sync → 原子 rename。manifest 最后生成，且不把自身列入 payload。未完成的临时目录不参与恢复。

### 12.2 `job resume`

```text
job resume JOB_ID
```

resume 只接受：

- 原任务处于可恢复的非成功终态；
- 检查点 Schema、结构、SHA、SQLite integrity 和 payload 全部通过；
- source identity、resolved Case、Profile 和 DatasetLock 完全一致；
- 检查点位于完整宏步边界；
- 高水位与数据库内容一致。

恢复创建同一 `job_series_id` 下的新 `run_id` 和 attempt。SQLite backup 形成新 attempt 的自包含数据库，删除父 attempt 检查点高水位之后的行，保留父 attempt 的 run 行和显式谱系。旧 attempt 目录保持原样。

新 attempt 从保存的下一个宏步继续。恢复结果与未中断运行的规范化科学字节必须一致。`job rerun` 继续表示从头运行。

### 12.3 自动恢复

以下原因可自动恢复：

- 受控主机关机；
- 进程中断；
- worker lease 丢失且确认原 worker 已退出。

最多执行 3 次自动恢复；初始 attempt 不计入该数字。每次均经过完整 resume preflight。连续失败达到上限后等待人工处理。

OOM、磁盘不足、检查点损坏和科学错误不自动恢复。`complete` 或 full verify 已通过的任务不得 resume，也不得被 daemon 自动重跑。

## 13. 容差与验证

所有阈值在性能优化前冻结。实现不得通过扩大样本容差、改变归一化或删减指标来修绿。

| 门 | 阈值 |
|---|---:|
| 解析/制造解相对误差 | `1e-10` |
| 质量和通量闭合相对误差 | `1e-10` |
| forward/adjoint 点积相对误差 | `1e-8` |
| OU 均值 | `0.02 sigma` |
| OU 方差相对误差 | `0.03` |
| OU 一阶相关绝对误差 | `0.02` |
| 外部实验归一化偏差绝对值 | `0.20` |
| 外部实验归一化 RMSE | `0.30` |

解析门同时采用 `M6_TOLERANCES.v1.json` 中按量纲给出的绝对下限。误差判定为：

```text
abs(actual - expected) <= max(absolute_floor, relative * abs(expected))
```

OU 正式统计至少使用 1,000,000 个样本和 8 个独立 seed，并报告原始矩和双侧 95% 区间。外部资料优先用公开不确定度归一化；没有公开不确定度时，验证资产需要在看模型结果前声明观测尺度。

正式矩阵覆盖 Windows x86_64、Ubuntu 24.04 x86_64、三类气象资料、正反向、四类任务及 w1/w4。故障注入覆盖缺字段、锁不匹配、数据库损坏、检查点撕裂、worker 丢失、OOM 和磁盘不足。

## 14. 性能与资源

性能采用同一主机、同一气象输入、同一 CPU 集合和交替执行顺序。先预热 1 次，正式运行 3 次，使用中位数。

50k 粒子、24 h、相同物理范围：

```text
Trajecta equivalent core / FLEXPART core <= 1.25
优化目标 <= 1.00
```

100k 粒子、24 h、完整适用预设：

| 指标 | 门 |
|---|---:|
| peak RSS | `<= 4 GiB` |
| `particles.sqlite` | `<= 4 GiB` |
| 50k → 100k 时间比例 | `<= 2.4` |
| 检查点墙钟开销 | `<= 5%` |
| 观测插桩开销 | `<= 3%` |

允许批处理、查表和通过科学门的确定性近似。替换算法时删除旧生产实现，不保留隐藏回退或快/准模式。

## 15. 阶段门和已知阻塞

| 阶段 | A0 后状态 | 进入条件 |
|---|---|---|
| A1 | contract ready | 强类型 Case、管线与 BL 切片按本合同实现 |
| A2 | contract ready | GMTED2010 全局锁就绪 |
| A3 | contract ready | ARM 单柱派生资产连同输入身份冻结 |
| A4 | contract ready | 水汽格点账本和伴随门完整 |
| A5 | contract ready | 两项 MODIS 全局锁及 LAI 派生链就绪 |
| A6 | external blocked | 先解决 OH 资料来源链；湿清除和半衰期可开发，阶段不能签署 |
| A7 | contract ready | Kincaid 归档身份已经冻结 |
| A8 | contract ready | A1–A7 的持久模块状态定义完成 |
| A9 | contract ready | 前述阶段全部签署后运行矩阵 |

OH 阻塞只限制 A6 签署。A0 不用未经审定的替代资料掩盖该问题。

## 16. 排除范围和法律边界

M6 不包含复杂化学网络、多相反应、气溶胶老化、再悬浮、冠层交换、海盐生成、山岳波和尾流。插件和导出器仍属于后续阶段。

Trajecta 发布代码保持 MIT 边界。FLEXPART 用于隔离的参考运行和概念核对。M6 的公式、控制流、类型和测试需从公开论文独立表达，GPL 源文件不得复制进 crate、测试 fixture 或生成代码。

M6-A0 完成后停在合同阶段。下一阶段只有在用户确认后开始。
