---
title: 区域填充水汽追踪教程
description: 使用 CFSR 资料建立区域填充粒子群，运行一个小型拉格朗日水汽追踪案例，并读取质量和生命周期结果。
---

# 区域填充水汽追踪教程

区域填充用一组携带等量干空气质量的粒子表示计算区域内的大气。初始粒子的空间分布由气象快照
中的空气质量决定：空气柱质量较大的位置会得到更多粒子，每个粒子代表的干空气质量保持一致。
粒子随三维风场运动后，其位置和携带的水汽状态可以用于分析水汽来源、去向与停留过程。

本教程在全球 CFSR 气压层资料上生成 1,000 个粒子，从 2009 年 1 月 1 日 06:00 UTC 正向积分
十分钟。案例沿用快速入门中的示例项目，并进一步说明粒子群生成、质量账本、输出事件和结果表。

## 这次计算表示什么

在初始时刻，Trajecta 根据每个网格单元的气压、气温、比湿、格点面积、地形和有效气压层，计算
区域内的干空气质量。随后将总质量划分成 1,000 个相等的质量区间，并按案例中的随机种子在每个
区间内抽取一个粒子。

这种抽样方式有两项重要性质：

- 每个初始粒子的 `dry_air_mass_kg` 相同；
- 粒子在空间上的密度随干空气质量分布变化。

因此，对粒子做计数或加权汇总时，可以保持明确的质量含义。粒子总数决定采样分辨率，单粒子的
载体质量则由区域内总干空气质量除以粒子数得到。

积分期间，粒子由三维风场输送。到达地表时应用反射规则，越过资料可表示的模式顶时终止。全球
网格在经度方向周期衔接，粒子穿过 180° 经线后从另一侧继续。每个数值宏步都会更新质量账本，
记录仍然活跃的质量、边界交换、正常终止、异常终止和未分配余量。

## 阅读案例配置

项目把案例命名为 `moisture`，把运行配置命名为 `product`。运行配置将逻辑资料 `cfsr` 绑定到
`data/`，使用纯 Rust 读取器，把结果写入 `runs/`，并为工作进程申请一个线程和 1 GiB 内存。

以下内容直接来自示例案例：

```yaml
--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"
```

配置可以按下表理解：

| 配置部分 | 本例含义 |
| --- | --- |
| `time` | 从 06:00 UTC 开始，向未来积分十分钟 |
| `meteorology.domains` | 使用全球 CFSR 区域，并预留一格水平插值边缘 |
| `particle_population` | 按干空气质量生成 1,000 个区域填充粒子 |
| `numerics.time_step` | 每步 300 秒，共推进两个数值步 |
| `numerics.integrator` | 使用球面二阶 Runge–Kutta 积分器 |
| `boundaries.policies` | 地表反射、模式顶终止、经度周期衔接 |
| `random_seed` | 固定粒子抽样和随机键 |
| `outputs` | 在时段起点和终点写入粒子状态 |

案例文件只描述科学问题，不包含本机绝对路径。逻辑资料名称 `cfsr` 由项目索引和运行配置共同
解析到本地文件。

## 准备资料

按照[演示资料](../getting-started/demo-data.md)页面下载四个 CFSR 文件，并放入
`examples/domain-fill-cfsr/data/`。然后校验项目并生成资料计划：

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --project examples/domain-fill-cfsr project data-plan --output data-plan.json
```

计划应选择 `cfsr-pgbl-pressure-v0` 和 `rust` 读取器。区域填充还需要以下资料能力：

| 能力 | 用途 |
| --- | --- |
| `transport` | 提供三维轨迹积分所需字段 |
| `near_surface_transport` | 处理近地层运动和地表反射 |
| `domain_fill` | 计算干空气质量并生成区域填充粒子 |

案例的物理时段只有 06:00–06:10 UTC。CFSR 每六小时一帧，时间插值还需要相邻帧，因此下载计划
包含 00、06、12 和 18 UTC。

先预览本地资料状态，再完成项目定稿：

```text
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
trajecta --project examples/domain-fill-cfsr project finalize
trajecta --project examples/domain-fill-cfsr doctor --deep
```

`project finalize` 会创建 `locks/cfsr.lock.json`。资料锁记录四个源文件的 SHA-256、有效时次、
网格签名、气压层签名、内置资料配置和能力集合。`doctor --deep` 随后检查已经锁定的资料，并
验证结果目录能否完成 SQLite 创建、WAL 写入、完整性检查和收尾。

!!! tip "修改案例后重新生成资料计划"

    时间、区域、粒子群或资料映射发生变化后，旧计划中的 `project_sha256` 会失效。先重新运行
    `project data-plan`，再准备资料并重新定稿。

## 运行案例

以前台方式运行 `product`：

```text
trajecta --project examples/domain-fill-cfsr run --profile product
```

前台命令会持续显示任务状态，并在结束时给出结果目录。下文用 `RESULT` 表示该路径。这个十分钟
小案例通常很快完成；操作系统第一次读取 GRIB2 文件时，运行时间可能略长。

计算结束后依次运行：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

端点输出包含两个计划事件。若全部 1,000 个粒子都存活到终点，`particle_state` 会有 2,000 行。
出现正常终止时，后一个事件的状态行会相应减少。`result inspect` 会给出实际粒子数、状态行数、
终止原因、读取器和结果文件清单。

## 读取粒子和状态

### 粒子数与状态行数

一个粒子只在 `particle` 表中登记一次。它每参加一次计划输出，就在 `particle_state` 中写入
一条状态。区域填充初始粒子的 `origin_kind` 为 `domain_initial`。有限区域的长时段案例还可能
从流入边界生成新粒子，这类粒子会记录自己的边界来源。

终止统计分为正常生命周期结束和异常粒子错误。穿出有限区域、越过模式顶或按粒子群规则退出，
都可能是正常终止。`invalid_meteorology` 等诊断表示某个粒子所需的气象状态无法有效计算，需要
结合时间、位置和质量类别进一步检查。

### 查看单个粒子

读取粒子 0 的人类可读轨迹：

```text
trajecta result trajectory RESULT --particle-id 0
```

记录会显示粒子 ID、物理时刻、经纬度、高度、积分偏移和存活时长。气压、气温和采样风等字段
也会随状态一起返回。对本例而言，起点和终点两行即可显示两个 300 秒积分步造成的位移。

分析程序可以改用 JSONL：

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

输出以一条流头开始，随后每条粒子状态各占一行，最后给出汇总。同一命令可以多次提供
`--particle-id`，一次读取若干粒子。

## 质量账本与水汽信息

每个区域填充粒子都保存 `dry_air_mass_kg`。本例按固定粒子数生成初始群体，因此 1,000 个初始
粒子拥有相同的干空气载体质量。浮点数逐项相加会产生微小舍入误差，生成器会在分配时补偿这部分
余量。补偿后的粒子质量之和加上未分配余量，应与气象快照计算得到的区域干空气总质量一致。

质量账本沿数值宏步记录以下项目：

| 项目 | 含义 |
| --- | --- |
| 初始或流入质量 | 初始化或边界新生粒子带入的干空气质量 |
| 活跃质量 | 当前仍由活动粒子表示的质量 |
| 流出质量 | 通过区域边界离开的质量 |
| 正常终止质量 | 按声明的物理生命周期规则结束的质量 |
| 异常终止质量 | 与粒子计算错误相关的质量 |
| 未分配余量 | 采用固定单粒子质量时，不足一个质量单位的剩余部分 |

比湿既参与初始干空气质量计算，也作为气象状态随轨迹采样。结果中的粒子位置、时间、载体质量和
比湿可按源区、受体区或时间段重新组织，为后续水汽归因计算提供输入。

## 扩展案例

### 延长时段

修改 `cases/moisture.yaml` 中的 `time.end`，重新执行 `project validate` 和
`project data-plan`。资料计划会按新区间增加 CFSR 时次。新文件准备完成后，再执行
`project finalize` 更新资料锁。

长时段研究通常需要中间输出。端点计划只保留起点和终点；间隔计划可以保存演变过程。结果状态
行数大致等于每个输出时刻仍然活跃的粒子数之和，因此输出频率会直接影响 SQLite 体积和写入量。

### 增加粒子数量

提高 `target_particle_count` 可以细化区域质量抽样，并降低单个粒子的载体质量。计算量、气象
查询次数、内存占用和结果行数都会随粒子数增加。正式扩大前，可以先用目标输出频率运行一个
较小粒子群，估算结果目录所需空间。

**调整本机资源**

工作线程、内存预算、读取器和结果目录都属于运行配置。可以复制 `profiles/product.yaml` 创建
另一个运行配置，在保留科学案例的同时比较执行环境。每次结果都会保存解析后的案例和运行配置，
便于确认它实际采用的参数。

## 接下来

[定时释放教程](release.md)从一个明确源项生成粒子，适合已知释放位置和质量的研究。
[气团输送教程](air-mass.md)继续使用区域填充，并转向有限区域的 ERA5 气压层资料。
