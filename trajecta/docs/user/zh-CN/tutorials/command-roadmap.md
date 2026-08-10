---
title: 常用命令路线图
description: 按本机准备、项目定稿、任务运行和结果读取的顺序，查找 Trajecta 日常工作中常用的命令。
---

# 常用命令路线图

这份路线图按照一次研究任务的实际顺序整理命令。第一次使用时可以从上到下执行；项目进入
日常运行后，可直接跳到相应阶段。示例中的 `PROJECT` 表示项目目录，`PROFILE` 表示项目索引中
登记的运行配置名称，`JOB_ID` 由运行命令返回，`RESULT` 可以是任务系列 ID、运行 ID 或结果目录。

## 按任务查找

| 当前要做的事 | 常用命令 | 继续阅读 |
| --- | --- | --- |
| 创建并检查本机配置 | `config init`、`config validate`、`doctor` | [配置与环境检查](../getting-started/configuration.md) |
| 查看项目是否可以继续 | `project status`、`project show`、`project validate` | [渐进配置](../how-to/progressive-configuration.md) |
| 规划和检查气象资料 | `project data-plan`、`data inspect`、`project finalize` | [先配置项目，后准备资料](../how-to/deferred-data.md) |
| 提交前台或后台任务 | `run`、`job events`、`job wait` | [前台运行与任务队列](../how-to/run-queue.md) |
| 查看或调整队列中的任务 | `job list`、`job status`、`job cancel`、`job rerun` | [取消与重跑](../how-to/cancel-rerun-prune.md) |
| 检查运行结果 | `result inspect`、`result verify`、`result trajectory`、`run report` | [结果读取](../how-to/results.md) |

## 准备本机环境

首次运行时创建本机配置，然后检查字段和运行环境：

```text
trajecta config init
trajecta config validate
trajecta doctor
```

本机配置保存任务数据库位置、资料根目录和资源池设置。`config validate` 只检查配置文档；
`doctor` 会继续检查当前选择的配置及其引用路径。项目和资料锁准备好后，可以增加深入检查：

```text
trajecta --project PROJECT doctor --deep
```

!!! tip "使用单独的本机配置"

    需要在同一台计算机上区分测试环境和正式环境时，为命令增加 `--config PATH`。任务数据库、
    资料目录和资源池会随所选配置一起切换。

## 准备项目与气象资料

已有项目通常从状态检查开始。资料尚未下载时，项目仍可完成文档校验并生成资料计划：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project validate
trajecta --project PROJECT project data-plan --output data-plan.json
```

资料计划列出需要的资料系列、时间范围、本地目标目录和资料锁位置。下载助手默认只展示请求；
确认目标目录和时段后，再增加 `--execute`：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json --execute
```

资料到齐后，可以先检查单个文件，再定稿整个项目：

```text
trajecta data inspect FILE
trajecta --project PROJECT project finalize
trajecta --project PROJECT doctor --deep
```

`project finalize` 会根据案例覆盖范围和运行配置检查本地资料，并绑定相应的资料锁。后续修改案例
时，应重新生成资料计划，再进行项目定稿。

## 选择前台或后台运行

前台运行会在当前终端等待执行轮次结束，适合教程和短任务：

```text
trajecta --project PROJECT run --profile PROFILE
```

后台运行在任务进入持久化队列后返回，适合长任务和多任务队列：

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

JSON 响应中的 `job_id` 用于后续状态查询。关闭提交任务的终端不会删除已经进入队列的任务。

!!! tip "先保留任务 ID"

    后台提交时可将 JSON 输出保存到研究日志。后续查询、取消和重跑均以同一个任务系列 ID 为入口。

## 查看和管理任务

以下命令覆盖日常队列操作：

```text
trajecta job list
trajecta job status JOB_ID
trajecta --format jsonl job events JOB_ID --since 0 --follow
trajecta job wait JOB_ID
```

`job status` 返回当前快照，`job events` 按持久化序号读取状态变化，`job wait` 等待当前执行轮次
进入终态。需要停止或重新执行时使用：

```text
trajecta job cancel JOB_ID
trajecta job cancel JOB_ID --force
trajecta job rerun JOB_ID
```

普通取消会给工作进程留出收尾时间。强制停止适用于工作进程无法响应的情况；执行目录会保留，
便于查看中断前已经写入的记录。`job rerun` 在原任务系列中创建新的执行轮次，已经完成的轮次
保持原样。

## 读取并验证结果

任务进入成功终态后，先查看摘要，再执行完整检查：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
```

按粒子 ID 读取轨迹，或为结果目录生成运行报告：

```text
trajecta --format jsonl result trajectory RESULT --particle-id ID
trajecta run report --result RESULT
```

`result trajectory` 可以写出一个或多个粒子的时间序列。需要读取全部粒子时可使用 `--all`，
数据量较大时适合采用 JSONL 输出并通过管道逐行处理。

## 查找其他命令

[完整命令索引](command-index.md)按六类任务列出当前版本的全部命令。需要确认全局参数、退出码、
机器输出格式或二进制原始帮助时，可继续查看[CLI 命令树](../reference/cli.md)。
