---
title: ERA5 压力层 air-mass 教程
description: 准备官方 ERA5 压力层资料，并使用 Trajecta 运行有限域干空气 population。
---

# ERA5 压力层 air-mass 工作流

本教程保留 CFSR domain-fill 项目中的等干空气载体 population，并转到有限的 ERA5
压力层网格。变化集中在三个方面：从尚未备齐资料的项目规划 provider request，将压力层
与地表文件共同准备，以及解释有限水平边界上的正常粒子流出。

Case 覆盖 2018 年 12 月 1 日 06:00 至 06:10 UTC。随包资料助手采用北纬 53° 至 45°、
东经 0° 至 10° 的教程区域。1,000 个粒子按该网格安全核心中的干空气质量初始化。

## 阅读项目绑定

项目索引把逻辑数据集 `era5-pressure` 连接到公开 profile
`era5-cf-pressure-netcdf-v0`：

--8<-- "examples/air-mass-era5-pressure/trajecta-project.yaml"

Case 选择有限域与干空气 population：

--8<-- "examples/air-mass-era5-pressure/cases/air-mass.yaml"

RunProfile 保存面向本机的路径和执行申请：

--8<-- "examples/air-mass-era5-pressure/profiles/product.yaml"

三份文件各自承担清楚的职责。Case 提供模拟时间与方向，同时声明 population、边界规则
和输出。项目索引选择 dataset profile。Profile 指向 `data/`、
`locks/era5-pressure.lock.json` 和 `runs/`，并申请一个使用 Rust reader、预算 1 GiB 的
worker。

## 了解 ERA5 请求

Pressure-level profile 的帧间隔为六小时。对于十分钟 Case，助手会规划 2018 年 12 月
1 日 00、06、12 和 18 UTC。每个日期形成三组 provider request：

| 请求 | 变量与作用 |
|---|---|
| 压力层 | 气温、两个水平风分量、压力垂直速度、比湿和位势；包含 1 至 1,000 hPa 的 37 个层 |
| 地表基础场 | 地表气压、位势、10 m 风、2 m 气温与露点、粗糙度、边界层高度和摩擦速度 |
| 地表通量 | 瞬时感热通量和水汽通量 |

Provider 文件先进入项目 data root 下的私有工作目录。准备阶段检查变量与坐标，添加
Trajecta 识别的 dataset-family metadata，再把 ready NetCDF 提升到 `data/ready/`。
准备后的文件可以由 Profile 选择 Rust 或 native NetCDF 路径读取。

## 在资料到位前创建 plan

即使 `data/` 仍为空，项目也可以保持 configured 状态并通过文档校验：

```text
trajecta --project examples/air-mass-era5-pressure project validate
trajecta --project examples/air-mass-era5-pressure project data-plan --output era5-plan.json
```

Plan requirement 应包含：

| 字段 | 预期值 |
|---|---|
| Case/Profile | `air-mass` / `product` |
| Dataset | `era5-pressure` |
| Dataset profile | `era5-cf-pressure-netcdf-v0` |
| Reader | `rust` |
| Lock path | `locks/era5-pressure.lock.json` |
| Data root | `data` |
| Capabilities | `domain_fill`、`near_surface_transport`、`transport` |
| Physical coverage | 2018-12-01 06:00–06:10 UTC |

这种 staged state 适合先准备 Case，再提交资料请求。修改 Case 会改变 `project_sha256`；
助手会把当前项目与输入 plan 比较，二者变化后需要重新生成 plan。

## 预览并获取资料

可以在 Python 环境中安装产品包列出的资料依赖：

```text
python -m pip install -r requirements-data.txt
```

先预览准确请求：

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json
```

JSON 输出先列出 north-west-south-east 顺序的区域和 acquisition anchors，随后给出
provider dataset 与变量清单。每项请求还包含压力层、目标路径和当前文件状态。随包项目
会显示内置教程区域 `[53, 0, 45, 10]`。

Climate Data Store client 从标准配置或环境读取账户凭据。助手输出和
`TRAJECTA_FETCH_MANIFEST.json` 记录 request metadata 与文件散列，不包含凭据值。

启动下载与准备流程：

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json --execute
```

服务端可能需要一些时间准备请求。已经存在且内容相同的 target 会继续使用。下载和转换
先保存在 `data/.trajecta-fetch/`，ready files 完成后再提升到正式位置。成功结束时，工作
目录被清理，并写入 `data/TRAJECTA_FETCH_MANIFEST.json`。

## Finalize 准备后的项目

检查一份 ready file，随后 finalize 并运行本地文件系统检查：

```text
trajecta data inspect examples/air-mass-era5-pressure/data/ready/ERA5_READY_FILE.nc
trajecta --project examples/air-mass-era5-pressure project finalize
trajecta --project examples/air-mass-era5-pressure doctor --deep
```

将 `ERA5_READY_FILE.nc` 替换为资料助手返回的文件名。Data inspection 会报告 family
marker、有效时次、网格、压力层和 source role。

Finalization 把 ready set 作为一个逻辑数据集扫描。DatasetLock 记录每份 prepared file
的内容散列与大小，并保存时间覆盖和网格签名。垂直签名、profile identity 与 capability
也写入同一个 lock。项目状态随后从 `configured` 变为 `finalized`。

## 运行有限域 Case

以前台方式提交 Profile：

```text
trajecta --project examples/air-mass-era5-pressure run --profile product
```

完成后通过产品命令读取结果：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

初始 output event 包含 1,000 个干空气粒子。最终数量取决于十分钟内是否有轨迹到达安全
网格边界。与边界相交的粒子会在交点时刻获得 terminal state，并记录
`domain_boundary` normal termination。

有限域运行的末端行数较少时，表示已经解析的边界流出。Inspect summary 会把该结果与
invalid meteorology 等 particle error 分开。质量账本也会同时列出 outgoing mass、
terminated carrier mass 和仍然活动的质量。

## 读取有限边界附近的运动

选择一个粒子，比较两个 scheduled state 及可能存在的 terminal state：

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

`physical_time` 是实际气象时刻。这个正向 Case 的 `integration_offset_ns` 为正，
`elapsed_age_ns` 表示出生后的经过时间。粒子若在两个 endpoint event 之间终止，最后一条
记录会使用准确交点时间。

采样风、气压和气温列还带有 validity 与 quality label。使用同一 locked input 比较
Rust-reader Profile 和 native-reader Profile 时，可以同时读取这些字段。

## 调整教程区域

当 Case 含 release geometry 时，通用助手可以由几何范围得到请求区域。当前 domain-fill
Case 没有 source geometry，因此随包 ERA5 教程采用已记录的北纬 53–45°、东经 0–10°
默认值。其他有限区域可以通过 provider utility 显式传入 north-west-south-east box：

```text
python tools/fetch_era5_pressure_cds.py --out-dir PROJECT_DATA_WORK --date YYYY-MM-DD --times 00:00 06:00 12:00 --area NORTH WEST SOUTH EAST
```

随后在该 work root 上运行 `prepare_era5_pressure_anchors.py`，把 ready files 放入 Profile
指定的 data root，再 finalize 项目。Prepared grid 定义 `limited_domain_terminate/v0`
使用的气象域。

扩大空间范围时，需要同步考虑文件体积、内存预算和 query locality。延长时间以后重新生成
project plan，确保 acquisition anchors 覆盖全部物理采样时刻。`horizontal_halo_cells`
还要为网格边缘附近的插值 stencil 留出足够支撑。

## 后续步骤

[Ozone 教程](ozone.md)继续使用同一有限教程区域，并转到 137 个 hybrid 模式层、三小时
anchors、反向执行和基于 potential vorticity 的 population。
[资料家族对照](data-families.md)汇总这些源布局差异。
