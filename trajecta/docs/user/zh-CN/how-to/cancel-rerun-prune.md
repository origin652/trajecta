---
title: 取消、重跑、隐藏和审阅 Trajecta 清理计划
description: 停止 queued 或 running attempt，在同一 job series 中创建新 attempt，隐藏终态任务并查看 dry-run prune 计划。
---

# 取消、重跑、隐藏与清理计划

Trajecta 为每个已接收的 attempt 保存持久化历史。生命周期命令可以改变队列状态，或创建新的
attempt；较早的结果目录继续保留，便于将中断运行与后续成功运行放在一起检查。

操作前先记录当前快照：

```text
trajecta --format json job status JOB_ID
```

本页的 `JOB_ID` 指 job series ID。快照中包含当前 `run_id`、attempt 序号、状态和输出目录。

## 请求安全取消

常规 cancel 采用协作式停止：

```text
trajecta job cancel JOB_ID
```

不同当前状态对应以下行为：

| 当前状态 | 安全取消行为 |
| --- | --- |
| `queued` | 从待 dispatch 队列移出，并记录 `cancelled` |
| `starting` 或 `running` | 记录 `cancelling`，worker 在 macro-step 边界读取请求 |
| `cancelling` | 返回当前取消状态 |
| 任一终态 | 返回已有终态快照 |

到达数值安全点后，worker 停止推进粒子，完成当前已有的部分结果，对输出数据库执行
checkpoint，随后进入 `cancelled`。因此，停止时间主要取决于到达并收尾当前 macro-step 所需
的工作量。

可以在另一个终端观察转换：

```text
trajecta job events JOB_ID --follow
trajecta job wait JOB_ID
```

`job wait` 对 cancelled 终态返回退出码 `1`。结果目录仍可交给 `result inspect` 和 forensic
检查。

## 强制停止无响应的 worker

需要立即停止 worker 时使用：

```text
trajecta job cancel JOB_ID --force
```

Daemon 终止自己管理的 worker 进程，并记录 `interrupted`。已经写出的文件留在 attempt 目录，
其中可能包含尚未完成常规 checkpoint 的 SQLite WAL 或临时输出。该目录按部分 forensic
artifact 处理，可先运行：

```text
trajecta result inspect JOB_ID
trajecta --format json job events JOB_ID --since 0
```

安全取消停留在 `cancelling`，并超过预期 macro-step 时长后，也可以采用 force cancel。
[恢复指南](../operations/recovery.md)列出了 worker 失联、SQLite/WAL 状态和 interrupted
manifest 的检查方法。

## 创建 rerun attempt

Rerun 保留逻辑 job series，并创建新的运行身份：

```text
trajecta --format json job rerun JOB_ID
```

新的 receipt 具有以下特点：

- `job_series_id` 与原 series 相同；
- `run_id` 是新的 UUID-v7；
- attempt 序号增加 1；
- 初始状态为 `queued`。

Series 中原有的规范化输入选择和 scheduler resource request 会复制到新 attempt。Case、
Profile、dataset lock 或资源请求需要调整时，修改项目并提交一项新的 run。

当前 attempt 仍在活动状态时，`job rerun` 会先请求安全取消并等待终态，再创建下一项 queued
attempt。由此避免同一逻辑 series 同时拥有两个活动 attempt。

新 attempt 继续使用同一个 job series ID 跟踪：

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --follow
```

事件和快照都带有 `run_id` 与 `attempt`，reader 可以据此分开新旧历史。

## 为成功 attempt 执行 full verify

新 attempt 完成后运行完整结果校验：

```text
trajecta result verify JOB_ID --full
```

Full verifier 检查生命周期覆盖、粒子质量、质量账本、SQLite 行数和 canonical output digest。
校验成功后，本机 catalog 会保存记录，并可将同一 series 中较早的终态 attempt 标为由该次
verified complete 运行取代。

这条记录决定旧 attempt 能否成为 prune plan 中的候选项。仅有一个更新的 attempt，或新
attempt 尚未通过 full verify，都不会使旧目录进入可清理候选。

## 从日常列表隐藏终态 series

`job forget` 将终态 series 从常规 `job list` 中隐藏：

```text
trajecta job forget JOB_ID
```

该操作只改变列表可见性。Catalog 行、事件、attempt 身份、结果目录和 verification 历史都会
保留，直接查询和结果路径仍可使用。

探索性 series 已审阅完毕，不再需要占据日常队列视图时，可以使用 forget。活动 series 先
进入终态，随后才能隐藏。

## 查看 dry-run prune plan

`0.1.0-alpha.1` 中的 prune 只计算计划：

```text
trajecta --format json job prune
```

响应使用 `trajecta.prune-plan/v1`，并始终包含：

```json
{
  "mode": "dry_run",
  "delete_enabled": false
}
```

候选项按 series、attempt 和 run identity 排序。一个看起来可以清理的条目，需要同一 series
中存在较新的 `complete` attempt，且该次运行已经通过 full verify。每项记录实际路径、递归
字节数、原因、保护标记和 superseding run ID。

该命令不会删除文件、SQLite 行、事件、报告或 provenance。当前版本没有 apply 或 delete
选项。估算存储量时可以保存 JSON 计划；手工归档则沿用所在计算环境的保留流程。

!!! tip "当前 prune 只生成报告"

    输出路径用于人工审阅。执行命令不会释放磁盘空间。

## Daemon 重启后的处理

Daemon 启动时读取持久化 attempt 状态和所管理的 worker 身份。`complete`、`failed`、
`cancelled` 与 `interrupted` 等终态保持不变，不会再次 dispatch。Queued 任务在资源可用后
继续接收。活动状态中的 worker 已经消失时，对应 attempt 会协调为 interrupted，并保留输出
目录。

需要新的 attempt 时运行 `job rerun`。Daemon 重启本身不会创建 attempt。
