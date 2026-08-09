---
title: 先配置 Trajecta 项目，后准备资料
description: 分阶段准备 Case 与 Profile、生成 data-plan、获取气象资料并 finalize 项目。
---

# 先配置项目，后准备资料

气象文件尚未下载时，Trajecta 项目已经可以进入审阅。Case 确定模拟时段和空间域，
RunProfile 选择资料家族与本地资料根目录。`project data-plan` 根据这两类文档计算所需覆盖，
整个过程不连接资料提供方。

资料在另一台机器上准备、ERA5 请求仍在 provider 队列中，或项目配置需要先行审阅时，都
可以采用这一流程。

## 1. 创建项目骨架

初始化空项目目录，并查看生成的索引：

```text
trajecta project init PROJECT --name "Moisture study"
trajecta --project PROJECT project show
```

将 Case 和 RunProfile 文件放在项目目录下。随后使用文本编辑器或 `project set`，把它们相对
于项目根目录的路径加入 `trajecta-project.yml`。[文档参考](../reference/documents.md)列出了
索引字段；仓库 `examples/` 目录中的完整项目也可以作为起点。

一个项目可以包含多个 Case 和多个 Profile。每个具名 Profile 选择一个已进入索引的 Case。
同一项研究可以由此容纳多组实验，每次运行仍对应明确的 Case/Profile 组合。

## 2. 在资料缺席时校验文档

先逐个校验文档，再检查项目索引：

```text
trajecta case validate PROJECT/cases/moisture.yml
trajecta case resolve PROJECT/cases/moisture.yml
trajecta case validate PROJECT/profiles/cfsr.yml
trajecta --project PROJECT project validate
trajecta --project PROJECT project status
```

逻辑配置完整后，项目通常会处于 `configured`。Profile 的必填值尚未齐全时保持 `draft`。
字段名、类型、引用或 population 设置有误时，命令会在资料准备前给出相应错误。

`case resolve` 展开组件引用并输出规范化内容。Case 由共享的 domain、time 或 population
片段组合时，可以通过这一视图检查展开结果。

## 3. 生成确定性的 data-plan

把计划写入项目目录：

```text
trajecta --project PROJECT project data-plan --output data-plan.json
```

计划为每项所选资料记录以下内容：

- 从 Case 推导的覆盖时段；
- 所需气象能力；
- dataset profile 与逻辑 dataset ID；
- 项目相对的资料根目录和 lock 路径；
- 用于识别过期计划的项目身份。

Case、Profile 或资料映射发生变化后重新生成计划。下载助手会对照当前项目与传入计划；两者
身份不同，执行会在下载前停止。

## 4. 预览 provider request

省略 `--execute` 运行助手：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

预览输出中的模式为 `dry-run`。它会展开插值 anchor、provider request、目标路径和各本地
文件的当前状态，不发起网络请求，也不编辑项目。

助手支持以下正式资料家族：

| 资料家族 | Provider 路径 | 本地准备方式 |
| --- | --- | --- |
| CFSR pressure level | NOAA NCEI HTTPS archive | GRIB2 保存在声明的资料根目录 |
| ERA5 pressure level | Copernicus Climate Data Store | Provider 文件转换为 reader 可直接读取的 NetCDF 布局 |
| ERA5 hybrid level | Copernicus Climate Data Store | 模式层与配套字段整理为 reader 可直接读取的 NetCDF 文件 |

ERA5 流程使用仓库中固定的资料准备依赖：

```text
python -m pip install -r requirements-data.txt
```

CDS 认证沿用官方 CDS client 配置。凭据保存在该配置或 provider 支持的环境变量中，助手不会
把它复制到项目或 fetch manifest。

## 5. 获取或转移文件

审阅请求列表后执行：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json --execute
```

所有目标路径都在项目声明的资料根目录内解析。已有 CFSR 文件与冻结身份一致时会直接复用；
文件冲突会形成错误，不会覆盖现有内容。ERA5 准备过程在所选资料根目录下使用带 owner 标记
的工作目录，完整文件随后移入 reader-ready 目录。

资料由另一套系统下载时，可把完成的文件复制到同一个资料根目录。项目此时仍未 finalize，
因此未完成的传输可以保留，待其余时次到达后继续。

## 6. 抽查源文件

`data inspect` 读取一个 GRIB 或 NetCDF 文件的元数据，不启动粒子模拟：

```text
trajecta --format json data inspect PROJECT/data/pgbl00.gdas.2009010100.grb2
```

每个资料家族或准备批次可以抽查一个文件。输出包含容器、网格、垂直坐标、时间、字段和
reader capability 信息；finalize 会继续检查这些信息。

## 7. Finalize 项目

计划中的文件到位后运行：

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

Finalize 解析所选 Case/Profile 组合，扫描实际资料根目录，检查时间覆盖与 capability，再
原子写入 DatasetLock。所有映射一致后，项目状态变为 `finalized`。

Fetch manifest 和 DatasetLock 的用途不同。前者记录资料助手取得的内容；后者是 Trajecta
在提交任务时读取、并写入运行 provenance 的资料清单。在这一流程中，DatasetLock 由
`project finalize` 创建。

!!! tip "下载完成后还要 finalize"

    最后一份文件到位后运行 `project finalize`。资料助手不会修改 DatasetLock。

## 资料准备中断后继续

再次以预览模式运行助手。文件状态表会区分 `present`、`missing` 和 `conflict`：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan PROJECT/data-plan.json
```

大小和 SHA-256 一致的 CFSR 文件会保留，随后只需执行缺少的请求。出现 conflict 时，对照
provider inventory 检查本地文件，再把所需文件放入干净的目标路径。

Case 的时段或空间域发生变化后，先生成新的 data-plan。Finalize 同样会报告覆盖缺口；在
准备更大的 provider request 期间，现有项目可以继续保持 `configured`。

## 多个项目共享一套资料

多个项目可以指向同一套只读气象资料。各项目保留自己的相对映射与 DatasetLock，文件字节
则可通过复制或挂载到各项目根目录下的方式共享。结果目录和 fetch 工作目录适合与共享只读
资料分开存放。
