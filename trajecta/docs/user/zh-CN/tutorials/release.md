---
title: 定时 release 教程
description: 使用明确的事件时刻、geometry、高度、粒子数和 tracer 质量运行确定性点源 release。
---

# 定时 release 教程

Release Case 从一个源事件开始。事件声明粒子的出生时段、水平与垂直位置、创建数量，以及
携带的命名物质质量。已知源区、注入过程、观测站或受体实验都可以采用这种 population
模型。

示例项目在经度 0°、纬度 0°、海拔 1,000 m 的位置创建 1,000 个粒子。所有粒子出生于
2009 年 1 月 1 日 06:00 UTC，共同携带一千克名为 `water` 的 tracer，并在全球 CFSR
气象场中向前运行十分钟。

## 阅读 Case

完整科学配置直接来自示例项目：

--8<-- "examples/release-cfsr/cases/release.yaml"

Case 与 domain-fill 示例采用相同的数值步、全球边界规则、端点输出和 CFSR 时段。
Population 部分则改为：

| Release 字段 | 示例值 |
|---|---|
| Population ID | `release` |
| Event ID | `event` |
| Event interval | 2009-01-01 06:00 UTC 这一时刻 |
| 粒子数 | 1,000 |
| 物质质量 | 整个事件共 1 kg `water` |
| 水平 geometry | Inline GeoJSON point `[0, 0]` |
| 垂直坐标 | 固定海拔 1,000 m |
| 随机种子 | 4201 |

`substances` 将稳定标识符 `water` 与可读名称关联起来。结果表使用该标识符作为粒子质量
列的键。

## 理解事件分配

### 时间与质量

当 `start` 等于 `end` 时，所有粒子的 birth time 相同。事件覆盖非零时段时，Trajecta
会在区间内分配确定性的分层出生时刻。Event 内的粒子 ordinal 与 Case seed 共同确定
schedule，因此 worker 执行顺序不会改变出生时刻。

每种命名物质的质量按粒子数分配。本教程前 999 个粒子使用常规浮点份额 0.001 kg，最后
一个份额补偿舍入余量，使保存的粒子质量合计为声明的一千克。

Release 粒子不表示干空气载体，因此 `dry_air_mass_kg` 为零。科学 payload 保存在
`particle_mass` 中，并以 particle 与 substance 为键。

### Geometry 与垂直位置

`Point` 会把全部粒子放在同一经纬度。其他可用 GeoJSON 类型包括 `MultiPoint`、
`LineString`、`MultiLineString`、`Polygon` 和 `MultiPolygon`。对应的采样权重分别来自
等点权、球面大圆线长或球面多边形面积。Geometry 可以直接写入 Case，也可以从项目内
相对路径指向 `.geojson` 文件。

垂直位置支持海拔高度、距地高度和气压。缺少 upper bound 时采用固定值；同时提供 lower
与 upper 时在闭区间中采样。距地高度和气压 release 会在粒子准确的出生位置与时刻查询
气象资料，解析为几何海拔后再开始输送。

## 准备共享 CFSR 资料

Release 项目使用快速入门的同一四帧资产。把文件复制到它的本地 data root：

=== "Windows PowerShell"

    ```powershell
    Copy-Item .\examples\domain-fill-cfsr\data\* `
      .\examples\release-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    cp examples/domain-fill-cfsr/data/* examples/release-cfsr/data/
    ```

随后校验并查看 requirement：

```text
trajecta --project examples/release-cfsr project validate
trajecta --project examples/release-cfsr project data-plan --output release-plan.json
python tools/fetch_trajecta_data.py --project examples/release-cfsr --plan release-plan.json
```

该 population 请求 `transport` 与 `near_surface_transport`。初始位置来自 release
geometry，因此 requirement 中没有 `domain_fill` capability。

创建 DatasetLock 并运行 deep local check：

```text
trajecta --project examples/release-cfsr project finalize
trajecta --project examples/release-cfsr doctor --deep
```

即使底层 CFSR 文件来自 domain-fill 项目或共享目录，新建的 `locks/cfsr.lock.json` 仍属于
当前 release 项目。

## 运行 release

以前台方式提交 Profile：

```text
trajecta --project examples/release-cfsr run --profile product
```

后续命令使用运行返回的结果目录：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta run report --result RESULT
```

示例的 endpoint schedule 包含两个 output event。初始事件中会出现全部 1,000 个粒子；
仍位于垂直模式范围内的粒子也会出现在末端事件。Inspect summary 给出准确状态行数和全部
正常终止统计。

## 读取 origin 与 tracer 质量

### Population 和质量记录

Release 粒子的 `origin_kind` 为 `release`，`origin_event_id` 为 `event`。离开源点后，
这个 origin 仍随粒子保存。`particle_mass` 为每个粒子保存一条 `water` 记录；
`particle_state` 保存随时间变化的位置、输送字段、状态和终止信息。

Run report 汇总声明质量与粒子表示的物质质量。多事件 Case 可以按 origin event ID 聚合
轨迹和质量，无需根据时间戳反推 release 来源。

### 单粒子轨迹

一次读取三条路径：

```text
trajecta result trajectory RESULT --particle-id 0 --particle-id 1 --particle-id 2
```

初始事件中，三个粒子共享相同位置和高度，各自的 stable ID 与质量行仍彼此独立。末端记录
展示局地风场在两个积分步内使它们分离或保持共位的情况。

较大的选择可以采用 JSONL，避免在内存中组装一个很大的响应：

```text
trajecta --format jsonl result trajectory RESULT --all
```

外部分析工具需要读取完整结果时，可以把 stream 重定向到分析文件。

## 调整 release 设计

### 增加时段或多个事件

设置不同的 `start` 与 `end` 可以表达持续 release。声明的粒子数会分布在该区间内，每个
粒子拥有准确 birth time。模拟区间需要沿所选方向包含相关 event time。

在 `events` 下增加条目，可以描述不同源时段、位置、物质或高度。每个事件使用稳定且唯一
的 ID，并拥有自己的粒子数和质量 map；所有事件共享 population seed 与 Case numerics。

Event time 改变以后重新生成 data-plan。气象 coverage 需要覆盖全部出生时刻和随后的轨迹
区间。

### 更换 geometry 或垂直坐标

复杂线段或多边形可以保存在项目相对路径的 GeoJSON 文件中。Resolved run 会记录源文件
散列和 canonical geometry hash。点或较短的坐标列表适合直接写入 Case。

源高度随地形变化时选择 `above_ground`，绝对几何海拔则选择 `above_sea_level`。以等压面
描述的源可以选择 pressure。Lower 与 upper 同时存在时，release 会覆盖一个垂直层，而非
单一表面。

粒子数控制 release 的空间与时间采样。物质总质量仍取 event 声明值，因此增加粒子后，
每粒子质量会降低。输出 cadence 与运行时长随后决定这些粒子产生的状态行数。

## 后续步骤

[Air-mass 教程](air-mass.md)会回到 domain filling，并引入通过项目 plan 下载的有限域
ERA5 资料。[Population 概念](../concepts/populations.md)集中比较 release、air-mass 与
ozone lifecycle。
