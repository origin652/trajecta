---
title: 跟踪 Trajecta 任务事件
description: 使用全局游标读取持久化队列事件，过滤单个任务，断线续读并消费 JSONL 流。
---

# 跟踪任务事件

Trajecta 将状态转换、进度、资源采样、warning、error 和 artifact 通知保存为 job event。多个
reader 共享同一份持久化日志，读取操作不会消费事件。终端、监控进程和自动化 client 可以
各自跟踪同一个队列。

## 事件身份与顺序

每条事件都有全局 `sequence`。首个 sequence 为 `1`，随后在整个 catalog 中递增，不会为
不同 job 重新计数。`--since N` 返回全局 sequence 严格大于 `N` 的事件。

事件还包含以下身份和状态字段：

| 字段 | 用途 |
| --- | --- |
| `job_series_id` | 标识跨 attempt 的逻辑任务 |
| `run_id` | 标识发出事件的确切 attempt |
| `attempt` | series 内从 1 开始的 attempt 序号 |
| `emitted_at` | 事件持久化时的 UTC 时间 |
| `kind` | 事件类别 |
| `state` | 事件发生后观察到的持久化状态 |

进度、资源、diagnostic 和 artifact 字段只在对应类别需要时出现。

## 一次读取当前事件

读取当前已有的全部事件：

```text
trajecta job events --since 0
```

在 `events` 后加入逻辑 job series ID，可以只读一个任务：

```text
trajecta job events JOB_ID --since 0
```

一次请求最多读取 10,000 条事件。Catalog 较繁忙时，把当前返回的最后一个全局 sequence
作为下一次游标。

JSON 模式会把当前批次放在一个 CLI envelope 中：

```text
trajecta --format json job events JOB_ID --since 120
```

短时脚本可以读取一批、完成处理、保存游标，随后退出。

## 持续跟踪一个任务

加入 `--follow` 后，命令会继续轮询：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

当前 attempt 进入终态，且截至该状态的持久化事件全部输出后，流结束。最后一条 JSONL
记录的 `kind` 为 `"summary"`。只有任务状态为 `complete` 时，`summary.run_success` 才为
true。

Human 输出也可以持续跟踪：

```text
trajecta job events JOB_ID --follow
```

每行依次显示持久化事件 sequence、观察到的状态、事件类别，以及可选消息。

## 跟踪整个队列

省略 job ID 后，命令接收所有 series 的事件：

```text
trajecta --format jsonl job events --since SEQUENCE --follow
```

整个队列没有单一任务终态，因此该流会保持打开，直到 client 被停止或读取发生错误。它适合
本机 dashboard、日志适配器和队列状态收集器。

游标是全局值，所以跟踪整个队列只需保存一个数字。单任务 reader 保存的仍是最后一条事件
中的全局 sequence。

## 消费 JSONL 记录

每一行都是独立的 `trajecta.cli-stream-item/v1` JSON 对象。`kind` 决定 payload：

| `kind` | Payload |
| --- | --- |
| `data` | `data` 字段中的 `trajecta.job-event/v1` 记录 |
| `diagnostic` | `diagnostic` 字段中的结构化读取错误 |
| `summary` | `summary` 字段中的流结束状态 |

外层 stream-item 的 `sequence` 只统计当前 CLI 流的行。事件内部的 `data.sequence` 才是持久化
全局游标，断线续读使用后者。

一个简洁的处理循环可以按以下顺序实现：

1. 读取一行并解析 stream item。
2. `kind: "data"` 时处理 `data`，下游操作成功后保存 `data.sequence`。
3. `kind: "diagnostic"` 时记录 diagnostic code 和 message。
4. `kind: "summary"` 时关闭流，并读取 `summary.ok`；字段存在时一并读取
   `summary.run_success`。

在下游操作成功后保存游标，可以在 client 崩溃后实现 at-least-once 交付。先保存游标对应
at-most-once。实际顺序可根据下游操作的幂等性选择。

## Client 中断后续读

假设最后一条完整处理的事件全局 sequence 为 `4281`：

```text
trajecta --format jsonl job events JOB_ID --since 4281 --follow
```

下一批从大于该数字的第一条匹配事件开始。使用更早的游标会重复读取持久化记录；使用更晚
的游标会跳过中间记录。Trajecta 不为各 reader 单独维护游标。

!!! tip "处理完成后再保存游标"

    Client 崩溃后可能重复一条事件，但不会跳过尚未完成下游处理的事件。

## 事件类别

| 类别 | 常见内容 |
| --- | --- |
| `state_transition` | 队列接收、worker 启动、running、取消或终态 |
| `progress` | 已完成 macro-step、当前模拟时间、活动粒子数和终止计数 |
| `resource` | Wall time，以及平台可提供的 CPU time 和 RSS 观测 |
| `warning` | 外部内存压力等可恢复运行条件 |
| `error` | Fatal 或 terminal diagnostic code 与消息 |
| `artifact` | 可审计输出路径的创建或完成 |

进度与资源事件以 `monitoring.sample_interval_ms` 为基础，并受运行时的有界发送节奏控制。它们
适合状态展示和运维历史；结果数据保存在运行目录中。

## 将当前快照与事件配合使用

事件记录持久化历史中的一个转换点。需要最新队列位置、结果路径或终态时间时，查询
`job status`：

```text
trajecta --format json job status JOB_ID
```

任务结束后，可以把状态中的 output directory 或 job ID 交给[结果命令](results.md)。
