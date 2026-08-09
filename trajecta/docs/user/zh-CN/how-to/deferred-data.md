---
title: 先配置项目，后准备资料
description: 分阶段编写案例和运行配置、生成资料计划、准备气象文件，并在资料到齐后完成项目定稿。
---

# 先配置项目，后准备资料

Trajecta 项目可以在气象资料下载前完成配置和审阅。案例（`Case`）给出模拟时段与空间区域，
运行配置（`RunProfile`）选择资料系列和本地目录。`project data-plan` 根据这些文档计算覆盖
要求，不需要连接数据服务。

以下情况很适合使用这一流程：

- 气象资料由另一台计算机准备；
- ERA5 请求需要在数据服务方排队；
- 网络暂时不可用，但项目配置可以先审阅；
- 人和自动化工具需要分几次补齐项目字段。

## 1. 创建项目骨架

初始化空项目并查看生成的索引：

```text
trajecta project init PROJECT --name "Moisture study"
trajecta --project PROJECT project show
```

把案例和运行配置文件放在项目目录内，再通过文本编辑器或 `project set` 将相对路径登记到
`trajecta-project.yaml`。字段说明见[项目文档参考](../reference/documents.md)，仓库
`examples/` 下的四个项目可以直接作为起点。

一个项目可以包含多个案例和多个运行配置。每项运行配置都选择一个已登记案例，因此每次运行
都有明确的“案例—运行配置”组合。

## 2. 在没有气象文件时校验文档

先检查单个文档，再检查项目索引：

```text
trajecta case validate PROJECT/cases/moisture.yml
trajecta case resolve PROJECT/cases/moisture.yml
trajecta case validate PROJECT/profiles/cfsr.yml
trajecta --project PROJECT project validate
trajecta --project PROJECT project status
```

逻辑配置齐全时，项目通常显示 `configured`。运行配置缺少必填项时会显示 `draft`。字段名称错误、
值类型不符、引用不存在或粒子群设置不合理时，命令会返回可修复的错误。

`case resolve` 会展开引用的组成部分，并输出规范化文档。案例由共享的区域、时间或粒子群片段
组合时，可以用它确认最终解析结果。

## 3. 生成资料计划

把计划写入项目目录：

```text
trajecta --project PROJECT project data-plan --output data-plan.json
```

计划会为每组案例与运行配置写出：

- 从案例推导的物理时间覆盖；
- 粒子群、边界和数值过程所需的气象能力；
- 内置资料配置和逻辑资料 ID；
- 相对于项目根目录的资料目录与资料锁路径；
- 用于识别过期计划的项目 SHA-256。

修改案例、运行配置或资料映射后，应重新生成计划。下载助手会把计划中的 `project_sha256` 与当前项目
比较；两者不同便停止执行，避免下载到错误时段或目录。

## 4. 预览数据服务请求

先省略 `--execute`：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

预览结果标记为 `dry-run`，并列出插值锚点、服务方请求、目标路径和每个本地文件的状态。该模式
不会发出网络请求，也不会修改项目。

资料助手支持以下系列：

| 资料系列 | 来源 | 本地准备方式 |
| --- | --- | --- |
| CFSR 气压层 | NOAA NCEI HTTPS 归档 | 将原始 GRIB2 保存在声明的资料目录 |
| ERA5 气压层 | 哥白尼气候数据存储 | 将服务方文件准备成读取器可用的 NetCDF 布局 |
| ERA5 混合模式层 | 哥白尼气候数据存储 | 将模式层变量和地表配套资料组合为可用 NetCDF |

ERA5 资料工具需要仓库中固定的 Python 依赖：

```text
python -m pip install -r requirements-data.txt
```

CDS 认证沿用官方客户端配置。凭据保存在官方配置文件或服务方支持的环境变量中，不会复制到
项目文件和下载清单。

## 5. 下载或转移文件

查看请求列表后开始执行：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json --execute
```

所有目标路径都解析到项目声明的资料目录内。大小和 SHA-256 符合计划的现有 CFSR 文件会被复用；目标位置
存在内容不同的文件时，助手会报告冲突并保留原文件。ERA5 准备过程使用资料目录下的专用工作区，
只有完整文件才会移入读取器使用的 `ready/` 目录。

若资料由其他系统下载，可以把完成的文件复制到同一个已声明资料目录。项目此时尚未定稿，
所以文件可以分批到达，已有部分会保留到其余时次准备完成。

## 6. 抽查源文件

`data inspect` 可以读取一个 GRIB 或 NetCDF 文件的元数据，不会启动模拟：

```text
trajecta --format json data inspect PROJECT/data/pgbl00.gdas.2009010100.grb2
```

每个资料系列或准备批次至少抽查一个文件。输出会显示容器、网格、垂直坐标、有效时次、字段和
读取能力。项目定稿时，这些信息会在完整文件集合上统一检查。

## 7. 完成项目定稿

计划中的文件到齐后运行：

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

项目定稿会解析所选案例和运行配置，扫描实际资料目录，检查时间覆盖与能力，然后原子写入资料锁。
所有资料映射都一致时，项目状态进入 `finalized`。

下载清单和资料锁的用途不同：

| 文件 | 用途 |
| --- | --- |
| `TRAJECTA_FETCH_MANIFEST.json` | 记录资料助手请求、下载和准备的文件 |
| 资料锁（`DatasetLock`） | 固定 Trajecta 提交运行时采用的资料清单，并写入结果溯源信息 |

资料助手不会创建或覆盖资料锁；本流程只由 `project finalize` 生成资料锁。

!!! tip "最后一个文件到达后再定稿"

    下载完成并不改变项目状态。运行 `project finalize` 后，再用 `doctor --deep` 检查准备结果。

## 资料准备中断后继续

再次用预览模式查看状态：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

文件表会区分：

| 状态 | 含义 |
| --- | --- |
| `present` | 本地文件的大小和 SHA-256 与计划一致，可直接复用 |
| `missing` | 目标尚未完成，可以继续下载 |
| `conflict` | 目标位置已有另一份内容，需要人工核对 |

匹配大小和 SHA-256 的 CFSR 文件会保留。解决冲突时，先对照服务方清单确认应采用的文件，再将
目标内容整理到干净路径。案例时段或区域变化后，应生成新资料计划再继续。

项目定稿也会报告覆盖缺口。较大的资料请求仍在服务方处理时，项目可以继续保持 `configured`，无需删除
已经到达的文件。

## 多个项目共用资料

多个项目可以指向同一份只读气象资料。每个项目仍保留自己的相对映射和资料锁。共享时，可以
复制资料目录，也可以把同一只读目录挂载到各项目声明的位置。

结果目录和下载工作目录应与共享只读集合分开。这样，一个项目的准备过程或运行产物不会改变
其他项目已经锁定的资料。
