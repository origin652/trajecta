---
title: 定时释放教程
description: 设置点源释放的时刻、位置、高度、粒子数量和示踪物质量，并读取释放粒子的轨迹结果。
---

# 定时释放教程

释放型案例从一个源事件开始。事件规定粒子何时生成、从哪里出发、位于什么高度、生成多少粒子，
以及这些粒子合计携带多少指定物质。已知排放源、注入过程、观测点或受体试验，都可以从这种
粒子群开始。

本例在东经 0°、北纬 0°、海拔 1,000 米的位置生成 1,000 个粒子。所有粒子于
2009 年 1 月 1 日 06:00 UTC 同时生成，共同携带 1 千克名为 `water` 的示踪物，随后在全球
CFSR 风场中正向积分十分钟。

## 阅读案例

完整配置直接取自示例项目：

--8<-- "examples/release-cfsr/cases/release.yaml"

数值步长、全球边界规则、端点输出和气象时段与区域填充示例相同，主要差别位于
`particle_population`：

| 释放设置 | 本例取值 |
| --- | --- |
| 粒子群 ID | `release` |
| 事件 ID | `event` |
| 释放时间 | 2009-01-01 06:00 UTC 的单一时刻 |
| 粒子数量 | 1,000 |
| 物质质量 | 整个事件合计 1 kg `water` |
| 水平几何 | 内嵌 GeoJSON 点 `[0, 0]` |
| 垂直位置 | 海拔 1,000 m |
| 随机种子 | `4201` |

`substances` 为 `water` 提供稳定标识和可读名称。结果表使用稳定标识作为粒子物质质量的键。

## 粒子如何分配

### 释放时间

当事件的 `start` 与 `end` 相同时，全部粒子在同一时刻生成。若两个时刻不同，Trajecta 会在
该区间内按分层方式分配确定的出生时间。事件内序号和案例随机种子共同决定分配结果，因此工作
线程的执行顺序不会改变粒子的释放时间。

释放区间必须位于模拟时段内。正向案例按时间增加方向经过事件，反向案例按时间减少方向经过事件。
改变方向时，需要同时检查模拟起止时间和所有释放事件。

**示踪物质量**

每种物质的事件总质量按粒子数分配。本例中，绝大多数粒子的 `water` 质量为 0.001 kg，最后一份
会补偿浮点除法产生的微小余量，使全部粒子质量之和严格对应声明的 1 kg。

释放粒子的干空气载体质量为零，科学载荷保存在 `particle_mass` 表中，并通过粒子 ID 和物质 ID
关联。包含多种物质时，每个粒子可以有多条质量记录。

### 水平几何和垂直位置

GeoJSON `Point` 会把全部粒子放在同一经纬度。还可使用 `MultiPoint`、`LineString`、
`MultiLineString`、`Polygon` 和 `MultiPolygon`。不同几何采用相应的抽样权重：

- 多点按点等权抽样；
- 线按球面大圆弧长度抽样；
- 多边形按球面面积抽样。

简单几何可以直接写在案例中。较复杂的源区可保存为项目内的 `.geojson` 文件，并用项目相对
路径引用。

垂直位置支持海拔高度、离地高度和气压。只给下界时表示固定位置；同时给出上下界时，会在闭区间
内抽样。离地高度和气压释放需要在每个粒子的具体出生位置与时刻查询气象资料，然后再开始输送。

## 准备共用的 CFSR 资料

释放项目可以复用快速入门下载的四个 CFSR 文件。把文件复制到该项目的资料目录：

=== "Windows PowerShell"

    ```powershell
    Copy-Item .\examples\domain-fill-cfsr\data\* `
      .\examples\release-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    cp examples/domain-fill-cfsr/data/* examples/release-cfsr/data/
    ```

随后校验项目、生成资料计划并预览本地状态：

```text
trajecta --project examples/release-cfsr project validate
trajecta --project examples/release-cfsr project data-plan --output release-plan.json
python tools/fetch_trajecta_data.py --project examples/release-cfsr --plan release-plan.json
```

释放型粒子群只需要 `transport` 和 `near_surface_transport`。初始位置由释放几何直接给出，
因此无需 `domain_fill` 能力。

创建该项目自己的资料锁，并运行深度环境检查：

```text
trajecta --project examples/release-cfsr project finalize
trajecta --project examples/release-cfsr doctor --deep
```

即使底层 CFSR 文件来自另一个项目或共享目录，`locks/cfsr.lock.json` 仍属于当前项目。它记录
本次运行配置实际选择的文件清单和内容散列。

## 运行释放案例

以前台方式提交 `product`：

```text
trajecta --project examples/release-cfsr run --profile product
```

将命令给出的结果目录代入以下命令：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

端点计划会产生两个输出事件。初始事件应包含全部 1,000 个粒子；仍在可用垂直范围内的粒子还会
出现在终点事件中。摘要会给出实际状态行数和正常终止统计。

## 读取来源和物质质量

### 粒子群与质量记录

释放粒子的 `origin_kind` 为 `release`，`origin_event_id` 为 `event`。粒子离开源点后，这一来源
仍然保留。`particle_mass` 为每个粒子保存一条 `water` 质量记录，`particle_state` 则保存随时间
变化的位置、输送状态和终止信息。

在多事件案例中，可以按 `origin_event_id` 分组轨迹和质量，无需根据时间戳猜测粒子属于哪次
释放。运行报告还会汇总各物质声明的总质量和结果中实际表示的质量。

### 单个粒子轨迹

一次读取三个粒子：

```text
trajecta result trajectory RESULT --particle-id 0 --particle-id 1 --particle-id 2
```

三者在初始事件拥有相同位置和高度，但粒子 ID 与质量记录相互独立。终点记录会显示局地风场是否
使它们分离。随机过程固定后，相同输入会产生稳定的粒子 ID 和释放分配。

读取大量粒子时，可使用 JSONL 避免在内存中构造一个大型 JSON 响应：

```text
trajecta --format jsonl result trajectory RESULT --all
```

外部分析程序可以把该流重定向到单独文件，或逐行处理。

## 修改释放设计

### 连续释放和多个事件

将事件 `start` 与 `end` 设为不同时间，即可表示一个持续释放区间。声明的粒子数会在区间内获得
各自确定的出生时刻。模拟时段必须沿所选积分方向覆盖这些事件。

`events` 下可以增加多个条目，分别描述不同时间、位置、物质或高度。每个事件需要唯一且稳定的
ID，并拥有自己的粒子数和质量映射；案例的数值设置和粒子群随机种子由这些事件共用。

事件时间变化后，应重新生成资料计划，确保气象覆盖包含每个出生时刻和后续轨迹区间。

### 复杂源区

详细的线源或面源适合放在项目内的 GeoJSON 文件中。解析后的运行会记录源文件 SHA-256 和规范化
几何散列。点源或很短的坐标列表直接写入案例更便于阅读。

`above_ground` 适用于随地形起伏的离地高度；`above_sea_level` 表示绝对海拔。源项按等压面
描述时可选择气压坐标。给出上下界后，释放位置会覆盖一个垂直层。

**调整粒子数和输出**

粒子数决定源项在时间、水平和垂直方向上的采样密度。事件总质量保持不变，粒子越多，每个粒子
分得的质量越小。最终状态行数还取决于运行时长和输出频率。扩大粒子群前，可以先确定分析所需
的时间分辨率，避免保存大量不会使用的中间状态。

## 接下来

[气团输送教程](air-mass.md)回到区域填充粒子群，并使用从资料计划下载的有限区域 ERA5 气压层
资料。[粒子群模型](../concepts/populations.md)集中比较释放、气团和臭氧粒子的生成与生命周期。
