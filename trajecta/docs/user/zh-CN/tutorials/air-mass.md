---
title: ERA5 气压层气团教程
description: 准备 ERA5 气压层资料，在有限区域内生成等干空气质量粒子，并读取边界流出和质量结果。
---

# ERA5 气压层气团教程

本教程继续使用区域填充中的等干空气质量粒子，并把气象资料换成有限区域的 ERA5 气压层产品。
与全球 CFSR 示例相比，这里新增三个实际环节：

1. 在气象资料尚未下载时，根据项目生成服务方请求；
2. 将气压层变量和地表变量准备成一个可锁定的资料集合；
3. 读取粒子穿出有限水平边界后的正常终止和质量流出。

案例从 2018 年 12 月 1 日 06:00 UTC 正向积分至 06:10 UTC。资料助手采用北纬 45–53°、
东经 0–10° 的教程区域。Trajecta 会在该网格的安全内部区域按干空气质量生成 1,000 个粒子。

## 项目如何连接三个文件

项目索引把逻辑资料 `era5-pressure` 映射到内置资料配置
`era5-cf-pressure-netcdf-v0`：

--8<-- "examples/air-mass-era5-pressure/trajecta-project.yaml"

案例选择有限区域和等干空气质量粒子群：

--8<-- "examples/air-mass-era5-pressure/cases/air-mass.yaml"

运行配置保存本机路径和资源请求：

--8<-- "examples/air-mass-era5-pressure/profiles/product.yaml"

三个文件各自承担一类信息：

| 文件 | 保存内容 |
| --- | --- |
| 案例（`Case`） | 时段、方向、区域、粒子群、边界、数值方法和输出 |
| 项目索引 | 案例与运行配置名称，以及逻辑资料到内置资料配置的映射 |
| 运行配置（`RunProfile`） | `data/`、资料锁、`runs/`、读取器、线程和内存预算 |

`product` 使用纯 Rust 读取器，申请一个工作线程和 1 GiB 内存。资料锁写入
`locks/era5-pressure.lock.json`。

## ERA5 请求包含什么

内置资料配置以六小时为一个锚点。对于十分钟案例，下载助手会规划
2018 年 12 月 1 日 00、06、12 和 18 UTC。每个日期包含三组服务方请求：

| 请求组 | 变量与用途 |
| --- | --- |
| 气压层 | 37 个气压层上的气温、两个水平风分量、压力垂直速度、比湿和位势 |
| 地表基础 | 地表气压、位势、10 米风、2 米气温与露点、粗糙度、边界层高度和摩擦速度 |
| 地表通量 | 瞬时感热通量和水汽通量 |

下载文件先保存在项目资料目录下的私有工作区。准备阶段检查变量、坐标和时次，写入 Trajecta
识别的资料系列元数据，再将可以使用的 NetCDF 文件移入 `data/ready/`。准备后的文件可以由
运行配置选择纯 Rust 或原生 NetCDF 读取路径。

## 先生成计划，稍后下载

资料目录为空时，项目仍可完成文档校验并进入 `configured` 状态：

```text
trajecta --project examples/air-mass-era5-pressure project validate
trajecta --project examples/air-mass-era5-pressure project data-plan --output era5-plan.json
```

本例生成的要求应包含：

| 字段 | 预期值 |
| --- | --- |
| 案例 / 运行配置 | `air-mass` / `product` |
| 逻辑资料 | `era5-pressure` |
| 内置资料配置 | `era5-cf-pressure-netcdf-v0` |
| 读取器 | `rust` |
| 资料锁路径 | `locks/era5-pressure.lock.json` |
| 资料目录 | `data` |
| 能力 | `domain_fill`、`near_surface_transport`、`transport` |
| 物理覆盖 | 2018-12-01 06:00–06:10 UTC |

资料计划包含 `project_sha256`。如果计划生成后又修改案例或运行配置，下载助手会发现摘要不一致，
并要求重新生成计划。这样可以避免根据旧时段或旧区域继续下载。

## 预览和下载资料

先为 Python 资料工具安装依赖：

```text
python -m pip install -r requirements-data.txt
```

不加 `--execute` 时，助手只显示将要发出的请求和目标文件：

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json
```

JSON 预览先列出区域、资料锚点和服务方数据集名称，再给出变量、气压层、目标路径及现有文件
状态。区域采用“北、西、南、东”顺序，本例为 `[53, 0, 45, 10]`。

哥白尼气候数据存储（CDS）客户端从官方配置文件或环境变量读取账户凭据。请求预览和
`TRAJECTA_FETCH_MANIFEST.json` 只保存请求参数及文件散列。

确认预览后开始下载和准备：

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json --execute
```

CDS 可能需要一段时间准备请求。匹配的已有目标会被复用；下载中的文件和中间转换结果保存在
`data/.trajecta-fetch/`。全部文件通过检查后，助手将它们移入 `data/ready/`，清理工作目录，
并写出 `data/TRAJECTA_FETCH_MANIFEST.json`。

!!! tip "下载中断后可再次执行同一命令"

    助手会检查已存在的匹配文件和临时下载，不必从头创建全部请求。

## 完成项目定稿

从助手输出中选择一个准备好的文件，先查看它的资料信息：

```text
trajecta data inspect examples/air-mass-era5-pressure/data/ready/ERA5_READY_FILE.nc
trajecta --project examples/air-mass-era5-pressure project finalize
trajecta --project examples/air-mass-era5-pressure doctor --deep
```

将 `ERA5_READY_FILE.nc` 换成实际文件名。`data inspect` 会报告资料系列标记、有效时次、网格、
气压层和源角色。

项目定稿会把 `data/ready/` 下的相关文件视为一个逻辑资料集合。生成的资料锁记录每个文件的
SHA-256、大小和时间覆盖，也记录网格签名、垂直签名、内置资料配置及能力。完成后，项目状态从
`configured` 变为 `finalized`。

## 运行有限区域案例

以前台方式提交：

```text
trajecta --project examples/air-mass-era5-pressure run --profile product
```

完成后读取结果：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

初始输出事件包含 1,000 个干空气载体粒子。终点仍有多少粒子，取决于十分钟内是否有轨迹到达
网格的安全边界。粒子与水平边界相交时，会在精确交点和时刻写入终止状态，终止原因为正常的
`domain_boundary`。

因此，终点状态行少于 1,000 并不自动表示计算错误。`result inspect` 会把区域流出与
`invalid_meteorology` 等异常粒子诊断分开。质量账本同时记录流出质量、正常终止质量和仍然
活跃的载体质量。

## 阅读边界附近的运动

用 JSONL 读取一个粒子的计划状态和可能出现的终止状态：

```text
trajecta --format jsonl result trajectory RESULT --particle-id 0
```

`physical_time` 是实际气象时刻。正向案例中的 `integration_offset_ns` 为正，
`elapsed_age_ns` 表示粒子自生成以来经历的时间。粒子若在两个端点事件之间穿出区域，最后一条
记录会保留精确的相交时刻，不会推迟到下一次计划输出。

采样风、气压和气温字段带有有效性与质量标签。使用相同资料锁比较纯 Rust 和原生读取器时，
这些标签可帮助定位某个字段或插值位置的差异。

## 换成自己的区域

含释放几何的案例可以由助手推导下载范围。区域填充案例没有源几何，本教程使用内置的
53–45° N、0–10° E 范围。准备其他有限区域时，可以直接使用底层 ERA5 气压层工具：

```text
python tools/fetch_era5_pressure_cds.py --out-dir PROJECT_DATA_WORK --date YYYY-MM-DD --times 00:00 06:00 12:00 --area NORTH WEST SOUTH EAST
```

然后对工作目录运行 `prepare_era5_pressure_anchors.py`，把准备好的文件放到运行配置所指向的
资料目录，再执行 `project finalize`。准备后网格的安全内部范围就是
`limited_domain_terminate/v0` 使用的计算区域。

区域扩大后，文件体积和气象查询内存会增加。时段延长后，应重新生成资料计划，确保每个物理采样
时刻都有相邻锚点。`horizontal_halo_cells` 还需在网格边缘为插值保留足够的邻近格点。

## 接下来

[平流层臭氧教程](ozone.md)沿用相同有限区域，改用 137 个 ERA5 混合模式层、三小时时次和反向
积分，并按位涡生成粒子群。[资料系列与轨迹方向](data-families.md)集中比较各类资料的结构。
