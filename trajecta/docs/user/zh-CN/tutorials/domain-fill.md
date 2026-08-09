---
title: Domain-fill 水汽追踪教程
description: 使用 CFSR 构建、运行并解读一个面向拉格朗日水汽追踪的 domain-fill 气团项目。
---

# Domain-fill 水汽追踪教程

Domain filling 使用等干空气载体质量的粒子表示气象域中的大气。大气柱中质量较大的部分
会获得更多粒子，地理采样仍与源网格和垂直坐标相连。得到的粒子历史构成拉格朗日水汽
追踪使用的气团基础。

本教程沿用十五分钟快速入门的项目，并进一步阅读 population、质量账本、输出计划和
结果表。Case 在全球 CFSR 压力层资料上运行 1,000 个粒子，时段为 2009 年 1 月 1 日
06:00 至 06:10 UTC。

## Case 表示的内容

初始时刻，Trajecta 根据气压、气温、比湿、格点面积、地形和可用垂直层构造干空气
snapshot。Snapshot 被划分为 1,000 个等质量 stratum，再按照 Case seed 从每个 stratum
抽取一个粒子，因此每个粒子携带相同的干空气质量。

这一构造带来两个直接结果。粒子计数同时具有质量加权含义；固定粒子数可以控制计算量，
每粒子载体质量则随所选气象域的总干空气质量变化。

积分期间，粒子沿三维风场移动。穿越地表时应用反射规则，越过可用模式顶时终止，经度
则在全球网格两端衔接。每个 macro step 都会写入质量账本，使活动质量、边界交换、终止
质量和初始域中的 residual 保持关联。

## 阅读项目

项目索引把 Case 命名为 `moisture`，Profile 命名为 `product`。Profile 将逻辑数据集
`cfsr` 绑定到 `data/`，选择 Rust reader，把结果写入 `runs/`，并申请一个 worker
线程和 1 GiB 内存。

下面直接引入可执行示例中的科学 Case：

--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"

各部分可以转换成以下研究描述：

| Case 部分 | 本教程中的含义 |
|---|---|
| `time` | 从 06:00 UTC 开始的十分钟正向区间 |
| `meteorology.domains` | 一个全球 CFSR 域，带一格插值 halo |
| `particle_population` | 恰好 1,000 个干空气 domain-fill 粒子 |
| `numerics.time_step` | 两个 300 秒轨迹步 |
| `numerics.integrator` | 球面二阶 Runge–Kutta 积分 |
| `boundaries.policies` | 地表反射、模式顶终止和经度周期 |
| `random_seed` | 稳定的 population 与随机采样键 |
| `outputs` | 两个物理端点的粒子状态，写入 SQLite |

Case 中没有操作系统路径。数据集名称 `cfsr` 通过项目索引和本地 Profile 解析。

## 准备资料并 finalize

按照[演示资料页](../getting-started/demo-data.md)的说明，将四帧文件放入
`examples/domain-fill-cfsr/data/`。随后检查项目状态并写出 data-plan：

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --project examples/domain-fill-cfsr project data-plan --output data-plan.json
```

Requirement 应选择 `cfsr-pgbl-pressure-v0`、reader `rust`，并包含 `transport`、
`near_surface_transport` 与 `domain_fill` capability。物理 coverage 为 06:00–06:10 UTC；
资料助手会把区间扩展为四个六小时 acquisition anchor。

预览本地资料状态并创建 lock：

```text
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
trajecta --project examples/domain-fill-cfsr project finalize
trajecta --project examples/domain-fill-cfsr doctor --deep
```

Finalization 写入 `locks/cfsr.lock.json`，其中包含四个源文件散列、有效时次、网格签名、
压力层签名、所选 profile 和 capability。Deep doctor 随后打开 locked data，并在启动
worker 前检查结果文件系统。

## 运行 Profile

以前台方式提交默认 `product` Profile：

```text
trajecta --project examples/domain-fill-cfsr run --profile product
```

终端会显示队列状态变化和最终结果位置。后续命令用 `RESULT` 代表该路径。这个小型 Case
通常在数秒内完成；操作系统首次读取 GRIB2 文件时可能稍长。

Worker 到达终态后生成可读报告：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

示例时段包含两个 scheduled output event。没有提前终止时，1,000 个粒子会在
`particle_state` 中形成 2,000 行。Inspection summary 还会显示 population origin、
活动与终止数量、质量合计、reader、结果文件和字段质量类别。

## 读取 population

### 先看汇总

`result inspect` 分开统计粒子数与状态行数。一个粒子只在 `particle` 表中创建一次，存活
期间则可在每个 scheduled output event 写入一条状态。Domain-fill 初始粒子的
`origin_kind` 为 `domain_initial`。较长的有限域运行还可能包含从 inflow boundary 出生的
粒子。

终止计数区分正常 lifecycle outcome 与异常执行结果。全球教程网格会衔接经度边界；模式
顶终止或 population outflow 仍可能形成正常终止。Invalid meteorology 和反射处理耗尽会
进入 abnormal group，可作为失败运行的调查起点。

### 跟踪一个粒子

以 human format 读取粒子 0：

```text
trajecta result trajectory RESULT --particle-id 0
```

两条记录先给出 birth identity、物理时间和位置，再列出 integration offset 与 elapsed
age。粒子状态和采样风位于后续字段，气压与气温各自带有质量信息。第二行的经纬度与高度
显示两个数值步产生的位移。

分析程序可以读取 JSONL stream：

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

Stream 先输出 header，随后每条状态对应一个 item，最后给出 summary。同一条命令可以
重复提供 `--particle-id`，一次读取多个粒子。

## 解读质量与 lifecycle

每个 domain-fill 粒子都保存 `dry_air_mass_kg`。使用 target-count 的 Case 会给所有初始
粒子分配相同载体值。其补偿求和加上 residual，等于气象 snapshot 计算得到的干空气质量。

Manifest 中的质量账本沿 macro step 追踪该表示。较长运行中常用的量如下：

| 量 | 含义 |
|---|---|
| Initial 或 incoming mass | 初始化或边界出生引入的干空气载体质量 |
| Active mass | 仍由活动粒子表示的载体质量 |
| Outgoing mass | 通过 domain-fill 边界过程离开的质量 |
| Normal terminated mass | 因声明的物理 lifecycle rule 结束的载体质量 |
| Abnormal terminated mass | 与粒子错误关联的载体质量 |
| Residual mass | 采用 mass-per-particle 模式时，小于一个载体量子的干空气部分 |

比湿参与干空气 snapshot 和气象查询。Particle-state product 提供轨迹与载体权重，后续
水汽诊断可按源区、受体区或时间段组织这些粒子历史。

## 扩展实验

### 延长物理时段

修改 `cases/moisture.yaml` 中的 `time.end`，再运行 `project validate` 并重新生成
data-plan。资料助手的 acquisition anchors 会随新区间移动。附加帧准备完成后，
finalization 为扩展后的 Case 创建更新 lock。

多小时研究若只保留 endpoint output，只能看到起点和终点。需要中间位置时，可在 Case
中选择 interval schedule。状态行数大致等于存活粒子数乘 scheduled event 数，因此输出
cadence 会直接影响结果体积。

### 增加 population 或调整分辨率

提高 `target_particle_count` 可以降低空间汇总中的采样噪声，单粒子的干空气载体质量会
相应减小。Worker 内存、气象查询量和 SQLite 输出都会随 population 增长。

十分钟、1,000 粒子的项目适合作为本地 preflight。更换 worker threads、内存预算、
reader 或 output root 时，可以新建 Profile。科学 Case 得以保留，执行环境也会清楚记录
在结果中。

## 后续步骤

[定时 release 教程](release.md)会用明确点源替换按质量加权的域初始化。
[Air-mass 教程](air-mass.md)继续使用 domain filling，并转到有限的 ERA5 压力层区域，
边界流出由此成为正常 lifecycle 的一部分。
