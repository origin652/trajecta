---
title: 取消、重跑、隐藏与清理预览
description: 停止排队或运行中的任务，在同一任务系列中创建新执行轮次，隐藏终态任务，并查看只读清理计划。
---

# 取消、重跑、隐藏与清理预览

Trajecta 会持久保存每个已接受任务的执行历史。生命周期命令可以改变队列状态，或在同一任务系列
中创建新的执行轮次。旧结果目录会继续保留，便于将中断运行与之后成功的运行放在一起检查。

开始操作前，先保存当前快照：

```text
trajecta --format json job status JOB_ID
```

本页的 `JOB_ID` 指任务系列 ID。快照会显示当前 `run_id`、执行轮次编号、状态和结果目录。

## 安全取消

常规取消采用协作方式：

```text
trajecta job cancel JOB_ID
```

不同状态下的行为如下：

| 当前状态 | 安全取消的处理 |
| --- | --- |
| `queued` | 从待派发队列中移除，状态变为 `cancelled` |
| `starting` 或 `running` | 记录 `cancelling`，工作进程在数值宏步边界响应 |
| `cancelling` | 返回当前取消状态 |
| 已到达终态 | 返回已有终态快照 |

工作进程到达安全点后停止推进粒子，收尾已生成的部分结果，对输出数据库执行检查点，并进入
`cancelled`。实际等待时间取决于完成和关闭当前数值宏步所需的时间。

另开终端观察状态：

```text
trajecta job events JOB_ID --follow
trajecta job wait JOB_ID
```

`job wait` 在 `cancelled` 终态返回退出码 `1`。结果目录仍可由 `result inspect` 读取，也可用于
分析取消前已经写入的内容。

## 强制停止无响应的工作进程

需要立即停止工作进程时：

```text
trajecta job cancel JOB_ID --force
```

守护进程会终止自己拥有的工作进程，并将执行轮次记为 `interrupted`。已经写入的文件原样保留，
其中可能包括尚未执行正常检查点的 SQLite WAL 和临时输出。先查看：

```text
trajecta result inspect JOB_ID
trajecta --format json job events JOB_ID --since 0
```

安全取消长时间停在 `cancelling`，且已经超过一个正常宏步所需时间时，也可以考虑强制停止。
[恢复指南](../operations/recovery.md)说明如何检查失联工作进程、SQLite/WAL 状态和中断运行清单。

!!! warning "强制停止可能留下未收尾文件"

    保留整个执行轮次目录。需要重新计算时，创建新执行轮次，不要继续写入旧目录。

## 创建重跑执行轮次

重跑保持任务系列不变，同时创建新的运行 ID：

```text
trajecta --format json job rerun JOB_ID
```

新回执具有：

- 与旧运行相同的 `job_series_id`；
- 新的 UUIDv7 `run_id`；
- 加一后的 `attempt`；
- 初始状态 `queued`。

重跑会复制任务系列中原先规范化的输入选择和调度资源请求。需要修改案例、运行配置、资料锁或
资源请求时，应按新配置提交一个新的任务系列。

若当前执行轮次仍然活跃，`job rerun` 会先请求安全取消，等待它进入终态，再创建下一轮。这样
同一任务系列不会同时拥有两个活跃执行轮次。

新一轮仍用同一个任务系列 ID 跟踪：

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --follow
```

事件和快照都包含 `run_id` 与 `attempt`，读取程序可以据此分开各轮历史。

## 验证重跑结果

新执行轮次完成后运行：

```text
trajecta result verify JOB_ID --full
```

完整验证会检查生命周期覆盖、粒子质量、质量账本、SQLite 行数和规范输出摘要。通过后，本机任务
数据库会记录当前运行的规范化输出摘要；同一任务系列中更早的终态执行轮次可以据此标记为已被替代。

只有后续 `complete` 执行轮次完成完整验证后，旧目录才可能出现在清理预览的候选项中。单纯存在
一个较新的执行轮次并不足以形成清理候选。

## 从日常列表中隐藏任务

`job forget` 可以让一个终态任务系列不再出现在日常 `job list` 中：

```text
trajecta job forget JOB_ID
```

该命令只改变列表可见性。任务数据库行、事件、执行轮次记录、结果目录和完整验证历史都会保留；
仍可通过 ID 或结果路径直接查询。活跃任务需要先进入终态。

## 查看清理预览

`0.1.0-alpha.1` 的清理命令只计算计划：

```text
trajecta --format json job prune
```

响应采用 `trajecta.prune-plan/v1`，并始终包含：

```json
{
  "mode": "dry_run",
  "delete_enabled": false
}
```

候选项按任务系列、执行轮次和运行 ID 排序。每项会给出：

| 字段内容 | 说明 |
| --- | --- |
| 目录路径 | 当前观察到的执行轮次目录 |
| 递归字节数 | 该目录估算占用空间 |
| 候选原因 | 为什么这一轮被较新运行替代 |
| 保护标记 | 当前是否仍需保留 |
| 替代运行 ID | 后续通过完整验证的运行 |

此命令不会删除文件，也不会更改任务数据库、事件、报告或溯源信息。本版本没有应用清理计划的
参数。可以保存 JSON 响应用于存储估算，再按照本地归档流程处理目录。

!!! tip "`job prune` 只生成清单"

    无论执行多少次，它都不会释放磁盘空间。

## 守护进程重启后

守护进程启动时会读取持久化状态和工作进程标识。`complete`、`failed`、`cancelled` 和
`interrupted` 等终态执行轮次继续保持终态，不会重新派发。排队任务在资源可用后继续准入。

若某个活跃执行轮次的工作进程已经消失，恢复过程会将它协调为 `interrupted` 并保留结果目录。
需要再次运行时，显式使用 `job rerun`；重启守护进程本身不会创建新执行轮次。
