---
title: ERA5 hybrid ozone 教程
description: 准备 ERA5 hybrid 模式层资料，并使用 Trajecta 运行反向平流层臭氧 population。
---

# ERA5 hybrid ozone 工作流

Ozone 项目组合了前面教程分别介绍的三项能力：按干空气质量填充气象域，在粒子出生时
分配 carried substance，以及从较晚的受体时刻反向积分到较早状态。气象资料使用 ERA5
全部 137 个 hybrid 模式层。

示例从 2018 年 12 月 1 日 06:00 UTC 开始，反向运行至 05:50 UTC。Trajecta 在 PV60
规则选择的平流层范围内准确初始化 1,000 个粒子。资料助手沿用 pressure-level 教程的
北纬 53–45°、东经 0–10° 区域。

## 阅读项目与 Profile

项目索引把逻辑数据集 `era5-hybrid` 映射到公开 profile
`era5-cds-hybrid137-v0`：

--8<-- "examples/ozone-era5-hybrid/trajecta-project.yaml"

面向本机的 Profile 选择 Rust reader 和本地路径：

--8<-- "examples/ozone-era5-hybrid/profiles/product.yaml"

Case 位于 `examples/ozone-era5-hybrid/cases/ozone.yaml`，主要设置如下：

| Case 选择 | 教程值 |
|---|---|
| 物理起点 | 2018-12-01 06:00 UTC |
| 物理终点 | 2018-12-01 05:50 UTC |
| 方向 | 反向 |
| Domain | 有限 ERA5 hybrid 网格 |
| 初始 population | PV60 eligible mask 内的 1,000 个干空气载体 |
| Carried substance | `ozone` |
| 时间步长 | 300 秒 |
| Boundaries | 地表反射、模式顶终止和有限域终止 |
| 输出 | SQLite 中的两个物理端点 |
| 随机种子 | 4203 |

RunProfile 申请一个 worker 线程和 1 GiB 内存。Lock 路径为
`locks/era5-hybrid.lock.json`，prepared files 从 `data/` 下发现。

## 理解 hybrid 坐标

ERA5 模式层会随大气和地形变化。层 `k` 上的气压由该层系数和本地地表气压重建，因此
垂直坐标随水平位置与时间变化，单独的三维变量文件只是完整 anchor 的一部分。

准备流程会组合以下输入：

| 输入组 | 内容 |
|---|---|
| Hybrid 三维字段 | 模式层 1–137 上的气温、东西风、南北风、比湿和压力垂直速度 |
| 地表气压输入 | 用于派生 canonical surface pressure 的对数地面气压 |
| 地表基础场 | 位势、10 m 风、2 m 气温与露点、粗糙度、边界层高度和摩擦速度 |
| 地表通量 | 瞬时感热通量和水汽通量 |
| 垂直坐标 metadata | 官方 half-level coefficient table 和 preparation identity |

Profile 会派生地表气压与近地比湿，映射通量符号，并为 `diagnostics` capability 计算
potential vorticity。Hybrid frame 相隔三小时；05:50–06:00 的物理区间需要规划
00、03、06 和 09 UTC。

## 理解 PV60 初始化

命名 ozone rule 先把干空气质量限制在海拔高于 3,000 m，且 hemisphere-normalized
potential vorticity 大于 2 个 potential-vorticity unit 的位置。南半球的值会先进行
符号归一化，再应用阈值。

Trajecta 在 eligible dry-air mass 内形成恰好 1,000 个等载体 stratum，并从每个 stratum
采样一个粒子。Ozone mole fraction 与 potential vorticity 成正比，斜率为每个
potential-vorticity unit 60 parts per billion by volume。随后根据 ozone 与干空气摩尔质量，
把 mole fraction 转换成 carried ozone mass。

结果中由此形成两个相关量。`dry_air_mass_kg` 是 eligible atmosphere 的载体权重，
`particle_mass` 中的 `ozone` 行是出生时分配的物质质量。输送过程中，该质量随粒子移动。
Population 质量账本则追踪有限域 inflow、outflow 和 termination 对干空气载体的影响。

## 在下载前规划资料

校验项目并保存 data-plan：

```text
trajecta --project examples/ozone-era5-hybrid project validate
trajecta --project examples/ozone-era5-hybrid project data-plan --output hybrid-plan.json
```

单项 requirement 应选择：

| 字段 | 预期值 |
|---|---|
| Dataset | `era5-hybrid` |
| Dataset profile | `era5-cds-hybrid137-v0` |
| Reader | `rust` |
| Capabilities | `diagnostics`、`domain_fill`、`near_surface_transport`、`transport` |
| Coverage | 按时间先后排列的 2018-12-01 05:50–06:00 UTC |
| Lock path | `locks/era5-hybrid.lock.json` |

虽然数值执行为反向，plan 中的 coverage 仍按物理时间排列。方向保存在 resolved Case 中。

安装资料工具依赖并预览请求：

```text
python -m pip install -r requirements-data.txt
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json
```

Preview 会列出每项 Climate Data Store request 的模式层和变量代码，并显示地表配套字段。
区域、anchors 和本地 target 位于同一项请求下。Provider client 从标准配置或环境读取
凭据，打印的 plan 只包含请求参数。

## 下载并准备 anchors

启动完整 provider 与 preparation pipeline：

```text
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json --execute
```

Raw service files 与 derived coefficient material 在 `data/.trajecta-fetch/` 中组合。准备阶段
检查 dimensions、variables、valid times、surface companions、level ordering 和 coefficient
identity。完成的 NetCDF anchors 移入 `data/ready/`，助手还会写入包含 request 与 file
identity 的 `data/TRAJECTA_FETCH_MANIFEST.json`。

Hybrid request 的体积和准备步骤都多于 pressure-level 教程。Provider 可能异步准备
model-level 请求；相同命令再次运行时，provider utilities 会继续使用已有且匹配的工作
内容。

## Finalize 并运行

检查一个 ready anchor，创建 lock，并检查本地 I/O：

```text
trajecta data inspect examples/ozone-era5-hybrid/data/ready/ERA5_HYBRID_READY_FILE.nc
trajecta --project examples/ozone-era5-hybrid project finalize
trajecta --project examples/ozone-era5-hybrid doctor --deep
```

Inspection response 应识别 `era5_cds_hybrid137`、137 个模式层、有效时次、source roles
和 prepared grid。Finalization 把三维主字段、对数地面气压、地表文件与 coefficient
metadata 一同写入 DatasetLock。

以前台方式运行 Profile：

```text
trajecta --project examples/ozone-era5-hybrid run --profile product
```

Population 初始化会读取 06:00 UTC snapshot，派生 potential vorticity，选择 eligible
dry-air strata，并分配 ozone。随后 worker 朝 05:50 UTC 推进并写入 endpoint result。

## 读取反向结果

校验并汇总完成目录：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

Event sequence 按执行方向排列：第一个 scheduled event 为 06:00，第二个为 05:50 UTC。
轨迹向更早物理时间推进时，`integration_offset_ns` 变为负值；`elapsed_age_ns` 仍为非负，
表示粒子从 06:00 出生后经历的积分时间。

Particle summary 会分开列出总干空气载体质量与总 ozone mass。Origin 标识 population 与
domain。Boundary-intersection termination 使用准确物理时刻，该时刻可能位于两个 scheduled
endpoint 之间。

分析时，可以将 particle records 以 JSONL stream 输出，再按 `particle_id` 与 SQLite 中
的 `ozone` mass row 连接。按末端位置、初始位置或 termination face 分组，可以观察所选
区间内不同方向的反向输送。

## 扩展 ozone 研究

更长的反向时段从提前 `time.end` 开始。重新生成 data-plan，使三小时 anchors 覆盖扩展后
的时间区间，再准备新增文件并 finalize。需要中间输送状态时，可将 endpoint output 改为
interval schedule。

提高 `target_particle_count` 可以降低 eligible stratospheric mass 内的采样噪声，同时
减小每粒子的 carrier mass。Potential-vorticity sampling、trajectory integration 和
SQLite output 的计算量也会增长。Profile 的内存与 worker threads 可独立于科学 Case
调整。

更换区域时，需要准备新的 hybrid grid、surface companion 和 coefficient metadata。
Limited-domain boundary 由 ready grid 定义，因此 outflow count 与 inflow birth 的空间
含义也随之变化。

## 后续步骤

[Population 概念](../concepts/populations.md)对照三种初始化策略，
[资料家族页](data-families.md)解释 hybrid 与 pressure-level anchor 在 cadence 和垂直签名
上的差异。科学比较材料集中在 [Validation](../validation/index.md) 区域。
