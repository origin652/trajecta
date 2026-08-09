---
title: CFSR 演示资料
description: 下载、校验、检查并放置 Trajecta 快速入门使用的四帧 CFSR 资料。
---

# CFSR 演示资料

Trajecta 的第一个项目使用 Climate Forecast System Reanalysis 的一个小型资料片段。资产
包含 2009 年 1 月 1 日的四帧全球压力层分析，每六小时一帧，总量约 22 MiB，普通工作站
即可方便地完成首次运行。

资料作为独立 release asset 发布，与 Windows 和 Ubuntu 安装包分开。下载一次后，两种
平台包均可使用，程序更新时也可继续保留这份资料。

## 归档内容

下载
[`trajecta-demo-cfsr-20090101-v1.zip`](https://github.com/origin652/trajecta/releases/download/v0.1.0-alpha.1/trajecta-demo-cfsr-20090101-v1.zip)。
解压目录包含 `data/`、`MANIFEST.json` 和来源说明。

| 有效时次 | 文件 | 字节数 | SHA-256 |
|---|---|---:|---|
| 2009-01-01 00:00 UTC | `pgbl00.gdas.2009010100.grb2` | 5,465,079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| 2009-01-01 06:00 UTC | `pgbl00.gdas.2009010106.grb2` | 5,436,947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| 2009-01-01 12:00 UTC | `pgbl00.gdas.2009010112.grb2` | 5,452,202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |
| 2009-01-01 18:00 UTC | `pgbl00.gdas.2009010118.grb2` | 5,494,468 | `a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade` |

归档本身的 SHA-256 为：

```text
cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8
```

`MANIFEST.json` 以机器可读形式重复记录文件名、字节数、散列、来源地址和有效时次。

## 为什么示例使用四帧

内置 `cfsr-pgbl-pressure-v0` dataset profile 描述六小时间隔的资料。轨迹每次采样时，
Trajecta 会选择包围该时刻的两帧，并在时间方向插值得到气象字段。快速入门从 06:00
运行至 06:10 UTC，因此采样时刻位于 06 UTC 分析场及其后一帧附近。

演示项目会锁定完整的四帧资产。这组文件覆盖一整天的源分析场，后续教程扩展 Case 时
还有可用余量。DatasetLock 记录四个文件的身份，查询引擎则按每次采样时间选择所需的
时间括号。

## 下载并校验

ZIP 下载完成后计算散列并解压：

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-demo-cfsr-20090101-v1.zip -Algorithm SHA256
    Expand-Archive `
      .\trajecta-demo-cfsr-20090101-v1.zip `
      -DestinationPath .\demo-data
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-demo-cfsr-20090101-v1.zip
    unzip trajecta-demo-cfsr-20090101-v1.zip -d demo-data
    ```

后续步骤使用的解压路径为：

```text
demo-data/trajecta-demo-cfsr-20090101-v1/data/
```

在不同机器之间复制资料时，也可以按照上表重新计算单个文件的散列。

## 放入示例项目

安装包中的项目从 `examples/domain-fill-cfsr/data/` 读取文件：

=== "Windows PowerShell"

    ```powershell
    Copy-Item `
      .\demo-data\trajecta-demo-cfsr-20090101-v1\data\* `
      .\examples\domain-fill-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    cp demo-data/trajecta-demo-cfsr-20090101-v1/data/* \
      examples/domain-fill-cfsr/data/
    ```

Release 教程使用另一个项目目录。可以修改其 Profile 中的 `data_roots.met`，让两个项目
指向同一物理资料目录；也可以复制一份文件，得到完全独立的教程目录。

## 使用 Trajecta 检查单帧

`data inspect` 只读取源资料元数据，不会启动粒子运行：

```text
trajecta --format json data inspect examples/domain-fill-cfsr/data/pgbl00.gdas.2009010100.grb2
```

该文件会报告以下主要属性：

| 属性 | 值 |
|---|---|
| 容器 | GRIB2 |
| Dataset family | `cfsr_pgbl_pressure` |
| 水平网格 | 144 × 73，经度周期 |
| 垂直坐标 | 37 个压力层 |
| 气压范围 | 1 至 1,000 hPa |
| Profile 帧间隔 | 21,600 秒 |

项目 finalize 时，命令返回的网格签名和层次签名会进入 DatasetLock。

## 示例读取的字段

CFSR 源文件包含许多消息。Dataset profile 选择 Trajecta 使用的字段，并将源单位和层次
类型映射到统一字段模型。

| 用途 | 选取的字段 |
|---|---|
| 三维输送 | 东西风、南北风、压力垂直速度、气温、比湿和位势高度 |
| 地表状态 | 地表气压和地表位势 |
| 近地输送 | 10 m 风、2 m 气温、2 m 比湿、粗糙度、边界层高度、热通量和摩擦速度 |
| Domain-fill 初始化 | 风场、气温、比湿、位势高度、地表气压和地形 |

快速入门会使用 domain-fill 初始化与输送字段。近地面和边界层字段也保留在同一 profile
中，较长的案例可以直接使用相应物理过程。工作流请求 diagnostics capability 时，
potential vorticity 会从压力层字段派生。

## 来源与引用

四个文件均按源字节取自
[NOAA NCEI CFSR 六小时低分辨率目录](https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/2009/200901/20090101/)。
[CFSR 元数据记录](https://www.ncei.noaa.gov/access/metadata/landing-page/bin/iso?id=gov.noaa.ncdc:C00765)
介绍低分辨率 GRBLOW collection 及其使用条件；NOAA 的
[开放数据页面](https://www.noaa.gov/information-technology/open-data-dissemination)
给出更广泛的访问政策。上述来源页面于 2026 年 8 月 2 日核查。

使用这些文件开展科研工作时，可引用：

Saha et al. (2010)，
[*The National Centers for Environmental Prediction Climate Forecast System Reanalysis*](https://doi.org/10.1175/2010BAMS3001.1)。

资产内的来源说明把数据集名称、提供机构、源地址、引用和访问日期与四个原始文件一同
保存。

## 在其他项目中使用

RunProfile 可以把逻辑数据集绑定到任何含有这些文件的本地目录。绑定内容包括资料根、
reader 和 DatasetLock 路径；Case 仍只引用逻辑数据集名称。多个项目因此可以共享同一
只读资料集合，同时各自维护 lock 与结果目录。

使用其他日期或更长时段时，先生成项目 data-plan，再把它传给
`tools/fetch_trajecta_data.py`。助手根据 Case coverage 整理 provider 请求和本地目标路径。
所需帧到位后，`project finalize` 会检查文件并创建运行使用的 lock。
