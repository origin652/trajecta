---
title: 检查和读取结果
description: 定位某次运行，查看生命周期与产物，执行快速或完整验证，流式读取轨迹，并生成运行报告。
---

# 检查和读取结果

每个结果目录只对应一次具体执行。目录中包含运行清单、粒子状态 SQLite 数据库、溯源信息
包、解析后的源文档，以及中断或失败时留下的诊断文件。日常读取先使用结果命令；需要跨粒子聚合
或专门查询时，再以只读方式打开 SQLite。

## 选择结果

以下三种值都可作为 `RESULT`：

| 形式 | 示例 | 解析方式 |
| --- | --- | --- |
| 任务系列 ID | `0198...` | 所选本机任务数据库中的当前执行轮次 |
| 运行 ID | `0198...` | 所选本机任务数据库中的具体执行轮次 |
| 目录路径 | `PROJECT/runs/.../attempt-1` | 磁盘上的具体目录 |

通过 ID 查询时，本机配置决定使用哪一份任务数据库。结果复制到另一台计算机后，可以直接使用
目录路径：

```text
trajecta --config configs/workstation.toml result inspect JOB_ID
trajecta result inspect archived-runs/study/attempt-2
```

## 查看生命周期、数量和产物

先使用人类可读输出：

```text
trajecta result inspect RESULT
```

需要完整结构时使用 JSON：

```text
trajecta --format json result inspect RESULT
```

`trajecta.result-inspection/v1` 按以下部分组织：

| 部分 | 内容 |
| --- | --- |
| `identity` | 任务系列、运行 ID、执行轮次和案例名称 |
| `lifecycle` | 状态、运行成功标记、开始与结束时间、失败详情 |
| `software`、`inputs`、`numerical` | 软件版本、源资料散列和数值设置 |
| `execution` | 资源设置和运行计数 |
| `particles` | SQLite 中的粒子、状态、事件、质量和终止数量 |
| `quality` | 风、气压和气温的有效性与质量分类 |
| `mass_ledger` | 质量账本行数和观察到的最大不平衡 |
| `artifacts` | 运行清单、SQLite、WAL、溯源信息、报告和诊断文件路径及大小 |
| `catalog` | 列表可见性、完整验证记录和执行轮次替代关系 |

对于 `complete`、`completed_with_particle_errors` 和 `cancelled`，检查命令会读取运行清单声明
的终态 SQLite 与溯源产物。`failed` 或 `interrupted` 只要运行清单可读，也可以返回部分信息。
SQLite 存在但暂时无法读取时，JSON 会保留来自运行清单的字段，将 SQLite 派生部分设为 `null`，
并给出一条结构化诊断。

## 理解运行状态

| 状态 | 含义 |
| --- | --- |
| `complete` | 运行完成，异常粒子终止数为零 |
| `completed_with_particle_errors` | 输出完成，但有一个或多个粒子异常终止 |
| `cancelled` | 安全取消完成，并收尾了部分结果 |
| `failed` | 运行级错误经过受控收尾并写入终态清单 |
| `interrupted` | 工作进程失联或被强制停止，目录保留当前文件 |
| `running` | 执行轮次仍在工作进程控制下 |

下游流程只接收成功科学运行时，可在机器输出中检查 `lifecycle.run_success`。对取消和中断目录，
`result inspect` 仍有助于了解运行到哪个时刻、写入了多少状态，以及有哪些文件可供恢复分析。

## 快速验证

快速验证检查终态产物的文件散列和结构：

```text
trajecta result verify RESULT
```

命令会：

1. 解析并校验 `run-manifest.json`；
2. 检查终态是否适合验证；
3. 检查 WAL 终态和公开 SQLite 行数；
4. 重新计算文件、规范 SQL 和规范输出摘要；
5. 校验溯源信息包及其规范化内容。

响应采用 `trajecta.result-verification/v1`，包含运行清单、SQLite、SQL、溯源内容和规范输出的
SHA-256。复制或归档结果时，可以把这份紧凑输出与传输记录放在一起。

## 完整验证

加入 `--full` 后执行逐行科学与生命周期检查：

```text
trajecta result verify RESULT --full
```

完整模式会检查粒子和样本顺序、输出事件覆盖、坐标与状态有限性、气象质量声明、终止一致性和
质量账本容差。`full` 对象给出实际检查的粒子数、样本数、终止记录数、输出事件数和质量账本行数。

通过任务数据库定位的 `complete` 执行轮次完成完整验证后，Trajecta 会在本机任务数据库中记录
其规范输出摘要。该记录用于执行轮次替代关系和只读清理计划。

正在运行、失败或中断的目录仍可检查，但不会形成成功的完整验证记录。

## 读取指定粒子的轨迹

可以提供一个或多个稳定粒子 ID：

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

CLI 会对 ID 排序并拒绝重复值。输出开始前，命令会先确认所有请求的粒子都存在；其中任何一个
缺失时，不会输出一半的轨迹。

读取全部粒子：

```text
trajecta result trajectory RESULT --all
```

大量记录按粒子和样本顺序流式读取。管道程序通常使用 JSONL：

```text
trajecta --format jsonl result trajectory RESULT --all
```

第一条数据是 `trajecta.trajectory-stream/v1` 头部，说明运行 ID、状态和选择范围。随后每个
粒子先输出一条 `record_kind: "particle"`，再输出按时间排序的
`record_kind: "state"`；最后一项为流汇总。

粒子记录包含来源、出生时刻、干空气质量、敏感度权重和携带物质质量。状态记录包含物理时刻、
位置、事件与样本序号以及粒子状态，还会给出选定气象字段、质量标记和溯源 ID。若粒子在该样本
终止，末尾会附上终止详情。

普通 JSON 模式会先把记录流写入临时缓冲文件，再形成一个完整 CLI 响应，因此无需把全部轨迹
放进内存。持续读取大结果时，JSONL 可以让下游程序逐行处理。

## 生成 Markdown 运行报告

在结果目录内生成标准报告：

```text
trajecta run report --result RESULT
```

输出路径固定为 `RESULT/run-report.md`。报告先列出运行 ID、生命周期、输入和执行设置，随后
汇总粒子与质量，并给出验证、产物和中断文件索引。写入采用同目录临时文件和原子替换；输入
未改变时，重复生成会得到相同内容。

运行报告是方便阅读的派生视图，不参与规范科学输出摘要，因此可以在归档后重新生成。

## 移动或归档结果

复制整个执行轮次目录；若目录中存在零字节 WAL，也一并保留。传输完成后直接对目录运行：

```text
trajecta result verify archived-runs/attempt-1
trajecta result verify archived-runs/attempt-1 --full
```

复制后的目录若未登记在当前本机任务数据库中，`catalog` 字段会为空；运行清单和各产物散列仍可
从目录本身读取。

## SQLite 高级分析

[SQLite 参考](../reference/results-sqlite.md)列出公开表、主键和常用连接方式。执行轮次进入终态
后，以只读连接打开 `particles.sqlite`。查看个别粒子历史时先用 `result trajectory`；跨粒子
聚合或特殊统计再使用 SQL。

派生表、NetCDF、图件和其他分析产品应写入单独的用户指定目录。结果目录作为源产品保留，避免
后续分析文件改变归档内容和目录摘要。
