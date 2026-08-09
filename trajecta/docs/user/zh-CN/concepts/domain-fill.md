---
title: Domain-fill 水汽追踪
description: 了解 Trajecta 中等干空气质量采样、有限域交换、质量账本和水汽分析方式。
---

# Domain-fill 水汽追踪

Domain filling 构造气象域内空气的拉格朗日表示。粒子按干空气质量分布，因此每个初始粒子
代表相同份额的大气质量，不对应相同几何体积。粒子轨迹与 carrier weight 共同形成移动的
air-mass 基础，可用于水汽归因和停留时间分析。

Case 通过 `domain_fill_air_mass` 选择这一 population：

```yaml
particle_population:
  strategy: domain_fill_air_mass
  id: moisture-air
  domain_id: limited
  target_particle_count: 50000
```

`domain_id` 指向 Case 中的一个气象域。初始化和边界交换使用该域的 safe core、垂直支持和
边界几何。

## 从气象场得到干空气质量

在 Case 起始时刻，Trajecta 向气象引擎请求 domain-fill snapshot。每个可用水平格点和垂直层
会组合以下输入：

| 输入 | 在质量表示中的作用 |
| --- | --- |
| 水平坐标 | 计算球面网格面积 |
| Interface pressure | 计算静力层质量 |
| 气温与比湿 | 表示湿空气状态及其中的干空气份额 |
| 位势或高度 | 建立垂直几何并放置 layer |
| Surface pressure 与 terrain | 确定本地下边界 |
| 可用 model top | 确定输送上边界 |

计算保留本地 transport floor 以上、可用 top 以下的干空气。比湿参与从总空气状态到干空气
密度的转换。位于气象域安全支持范围外的 cell 不进入初始质量预算。

最终 snapshot 包含按顺序排列的 mass stratum，以及这些 stratum 表示的干空气总质量。

## 两种 population 分辨率

Case 在以下字段中选择一个：

| 字段 | 作用 |
| --- | --- |
| `target_particle_count` | 创建确切数量的初始粒子，将干空气总质量等分给它们 |
| `target_dry_air_mass_per_particle` | 使用指定 carrier mass；初始数量为总质量除以 carrier mass 后向下取整 |

Count 模式便于控制内存和输出规模。Mass-per-particle 模式便于让不同空间域或季节采用接近的
carrier quantum。第二种模式中，小于一个完整 carrier 的干空气质量作为 residual 进入质量
账本。

Carrier 值写入每个粒子的 `dry_air_mass_kg`。粒子是带权空气质量样本，不会取得其初始所在
网格的全部质量。

## 等质量分层放置

Trajecta 按网格 column 与 layer 排列 dry-air stratum，再把累计质量轴分成等宽区间。每个
区间放置一个粒子，依次确定：

1. 区间内的干空气质量坐标；
2. 包含该坐标的 column 与 layer；
3. Column 内的 longitude；
4. 通过均匀采样正弦纬度，在球面面积上放置 latitude；
5. 静力层内的 pressure，再通过 log-pressure 转换为 geometric height。

Terrain 或完整 transport floor 在 cell 内发生变化时，sample 会重新放入本地有效垂直范围，
避免粒子低于 transport floor 或高于当地资料支持上限。

Case random seed、population ID、domain ID、particle identity、lifecycle event 和 sampling
dimension 共同形成确定性随机 key。Worker 数量与线程调度不会改变初始 sample。

## Population 表示的质量

初始化时，干空气表示满足：

```text
sum(initial particle carrier mass) + initial residual mass
```

Count 模式中的初始 residual 为零。提高粒子数会降低单粒子 carrier mass，为空间聚合提供更
多拉格朗日样本，同时增加气象查询、particle-state 行、provenance assignment 和 worker
内存。

比湿参与 dry-air snapshot 构造，也可在路径上的气象查询中读取。Particle-state 产品提供
位置、时间、carrier mass 和所选气象状态。水汽分析可以按源区、受体区、穿越时间或停留
区间整理这些带权轨迹，再与研究所需的湿度场结合。

## 全球域生命周期

全球周期域没有侧向 inflow 或 outflow 边缘。Longitude 通过所选 periodic boundary policy
回绕。除非另一个声明的边界规则终止粒子，例如到达 model top，粒子数通常保持固定。粒子
接触地表时由配置的 reflection policy 处理。

全球 CFSR 快速开始使用 endpoint output 和短时段，初始与最终状态可以直接展示不含侧向
交换的基本 population。

## 有限域 inflow

有限域的四个侧面按垂直 layer 划分。每个 macro-step 中，Trajecta 根据 face area、干空气
密度和边界法向风计算流入干空气质量。Forward 与 backward 会按各自物理积分方向解释 inward
normal。

Incoming mass 按 boundary face 分开累计。累计值达到一个完整 carrier mass 时，粒子在该
threshold 对应的确切时刻出生。Origin 记录 domain 和 boundary-face ID；切向与垂直坐标在
相应 face layer 内确定性采样。

小于下一个 carrier threshold 的质量保留为 boundary residual，并带入后续 macro-step。有限
粒子表示由此与连续干空气通量保持对应。

## 有限域 outflow

活动粒子越过有限域边界时，continuous boundary solver 沿建议路径定位交点。粒子在该物理
交点终止，reason 为 `population_outflow`，classification 为 normal。

同一个 step 中即使有等量质量流入，粒子数量也可能下降。Incoming dry-air mass 需要量化为
完整 carrier particle，剩余部分进入 residual。因此，particle count 用于描述采样状态，质量
守恒则读取 mass ledger。

## 逐 step 质量账本

每个完成的 macro-step 记录以下量：

| 量 | 含义 |
| --- | --- |
| `opening_active_kg` | Step 开始时 live particle 上的 carrier mass |
| `opening_residual_kg` | 带入该 step 的 initial 与 boundary residual |
| `incoming_kg` | 通过有限域侧面流入的干空气质量 |
| `outgoing_kg` | 归入 population outflow 的 carrier mass |
| `normal_terminated_kg` | 由其他声明的 normal rule 终止的 carrier mass |
| `abnormal_terminated_kg` | 与异常粒子终止关联的 carrier mass |
| `closing_active_kg` | Step 结束时 live particle 上的 carrier mass |
| `closing_residual_kg` | Birth 完成后剩余的 sub-carrier mass |

账本计算：

```text
opening active + opening residual + incoming
  = outgoing + normal terminated + abnormal terminated
    + closing active + closing residual
```

Manifest 为每个 step 记录实际 imbalance 和数值 tolerance。Full result verification 会重复
检查各条 ledger record，并计算最终累计 balance。

## Normal 与 abnormal termination

读取 population 历史时，需要区分终止类别：

| 类别 | 示例 | 含义 |
| --- | --- | --- |
| Normal | Population outflow、model-top rule | 已声明的物理生命周期路径 |
| Abnormal | Invalid meteorology、数值边界失败 | 粒子无法在所选模型下继续 |

只要出现 abnormal particle termination，终态运行状态就为
`completed_with_particle_errors`。受影响的粒子及其最终状态仍保留在结果中。`result inspect`
可以读取按 reason 分组的计数，`result trajectory` 可用于查看具体路径。

## Forward 与 backward domain filling

Forward 从初始化空气质量向较晚物理时间积分；backward 向较早时间积分。初始采样都发生在
`time.start`，随后时钟沿 Case direction 推进。Boundary inflow、outflow、birth time、
elapsed age 和 event order 也按该方向定义。

面向受体的水汽研究可以在 receptor time 初始化区域 air mass，再用 backward Case 向早期
气象场追踪 carrier trajectory。Forward Case 可以从源时段出发，查看 air mass 到达后续区域
的过程。需要对照的实验适合明确保持 domain、population resolution、气象准备、integration
step 和 output schedule。

## 选择粒子数与输出间隔

Population size 控制采样密度，output cadence 控制保存路径的时间分辨率。两者影响分析的不
同部分。

一项初步研究可以按以下顺序进行：

1. 先运行短时、低粒子数 Case，查看空间分布。
2. 检查 mass-ledger closure 和 termination reason。
3. 根据研究中的边界穿越或停留过程选择 output interval。
4. 增加粒子数，直到目标聚合量在研究空间尺度上趋于稳定。
5. 将粒子数、carrier mass、seed、integration step 与 output schedule 一起记录。

[Domain-fill 教程](../tutorials/domain-fill.md)使用紧凑 CFSR 项目展示了这一流程。
