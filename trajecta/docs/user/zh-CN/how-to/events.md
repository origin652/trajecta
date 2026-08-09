---
title: 跟踪任务事件
description: 使用全局游标读取持久化任务事件，筛选任务系列，安全续读，并处理 JSONL 状态流。
---

# 跟踪任务事件

Trajecta 将状态变化、进度、资源采样、警告、错误和产物通知保存为任务事件。所有读取端共享同一
份持久日志；读取一条事件不会删除它，也不会影响其他读取端。终端、监控程序和自动化脚本可以
同时跟踪同一个队列。

## 事件标识与顺序

每条事件都有全局 `sequence`。序号从 `1` 开始，在整个任务数据库中持续递增，不会因任务不同
重新计数。`--since N` 返回全局序号严格大于 `N` 的事件。

事件还包含：

| 字段 | 用途 |
| --- | --- |
| `job_series_id` | 标识跨执行轮次保持稳定的逻辑任务 |
| `run_id` | 标识产生该事件的具体执行轮次 |
| `attempt` | 任务系列内从 1 开始的执行轮次编号 |
| `emitted_at` | 事件持久写入时的 UTC 时间 |
| `kind` | 事件类别 |
| `state` | 该事件之后观察到的持久状态 |

进度、资源、诊断和产物字段只在对应事件类别中出现。

## 一次读取当前事件

读取目前已有的全部事件：

```text
trajecta job events --since 0
```

把任务系列 ID 放在 `events` 后，只读取该任务：

```text
trajecta job events JOB_ID --since 0
```

单次请求最多返回 10,000 条。队列事件很多时，记录最后一条事件的全局序号，作为下一次
`--since` 游标。

JSON 模式会把当前批次放在一个完整的 CLI 响应对象中：

```text
trajecta --format json job events JOB_ID --since 120
```

这种方式适合短脚本：读取一批、完成处理、保存游标，然后退出。

## 持续跟踪一个任务

加入 `--follow` 后持续轮询：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

当前执行轮次进入终态，并且对应的持久事件全部输出后，流会结束。最后一行的
`kind` 为 `summary`。只有任务状态为 `complete` 时，`summary.run_success` 才为 `true`。

也可以使用人类可读格式：

```text
trajecta job events JOB_ID --follow
```

每行依次显示全局事件序号、任务状态、事件类别和可选消息。

## 跟踪整个队列

省略任务 ID 后，流会包含所有任务系列：

```text
trajecta --format jsonl job events --since SEQUENCE --follow
```

整个队列没有单一任务终态，因此该流会持续打开，直到客户端停止或发生读取错误。它可用于本机
状态面板、日志转接程序或队列采集器。

游标属于整个任务数据库，一个序号即可续读所有任务。即使只关心单个任务，读取者保存的仍是
最后处理事件的全局序号。

## 读取 JSONL

每行都是独立的 `trajecta.cli-stream-item/v1` JSON 对象。外层 `kind` 决定内容：

| `kind` | 内容 |
| --- | --- |
| `data` | `data` 中的一条 `trajecta.job-event/v1` 事件 |
| `diagnostic` | `diagnostic` 中的结构化读取错误 |
| `summary` | `summary` 中的流结束状态 |

外层流项目的 `sequence` 只对当前 CLI 流中的行计数。`data.sequence` 才是持久化全局游标，
断线续读时应保存后者。

一个稳妥的处理循环可以按以下顺序实现：

1. 逐行读取并解析流项目。
2. 遇到 `kind: "data"` 时，先处理 `data`；下游操作成功后再保存 `data.sequence`。
3. 遇到 `kind: "diagnostic"` 时，记录诊断码和消息。
4. 遇到 `kind: "summary"` 时，读取 `summary.ok` 和可选的 `summary.run_success`，然后关闭流。

在下游操作成功后保存游标，客户端崩溃重启时可能重复处理最后一条事件，但不会跳过尚未完成的
操作。这种方式适合要求至少处理一次的状态同步。若下游操作天然幂等，重复事件通常更容易处理。

## 断线后续读

假设最后完整处理的事件序号为 `4281`：

```text
trajecta --format jsonl job events JOB_ID --since 4281 --follow
```

下一批从大于该序号的第一条匹配事件开始。使用更早游标会重复返回持久记录，使用过大的游标则会
跳过记录。Trajecta 不替各读取者保存游标，监控程序应自行持久化。

!!! tip "处理完成后再保存游标"

    客户端意外退出后，最多重复一小段事件；尚未完成的下游处理不会被游标越过。

## 事件类别

| 类别 | 常见内容 |
| --- | --- |
| `state_transition` | 队列准入、工作进程启动、开始运行、取消或终态 |
| `progress` | 已完成宏步、当前模拟时刻、活动粒子数和终止数量 |
| `resource` | 已运行时间，以及可用时的 CPU 时间和常驻内存采样 |
| `warning` | 外部内存压力等可恢复情况 |
| `error` | 导致失败或终态的诊断码与消息 |
| `artifact` | 结果路径的创建或产物完成写入 |

系统会尽量按照 `monitoring.sample_interval_ms` 指定的间隔发布进度和资源事件，同时限制过高的
采样频率。这些事件适合状态展示和运维记录；完整粒子结果仍保存在执行轮次目录。

## 同时读取当前快照

事件记录某个时刻发生的变化。需要最新队列位置、结果路径或终态时间时，再查询当前快照：

```text
trajecta --format json job status JOB_ID
```

任务进入终态后，可把快照中的结果目录或任务 ID 交给[结果命令](results.md)。
