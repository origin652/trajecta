---
title: 关机与内存压力
description: 为长时间 Trajecta 任务安排关机，处理外部内存压力，并恢复被操作系统终止的 worker。
---

# 关机与内存压力

长时间轨迹任务会与操作系统、文件缓存、交互式应用和其他科学程序共享机器。Trajecta 通过
scheduler reservation 限制自身接收的工作量，并在启动新 worker 前单独检查主机当前可用
内存。

本页区分三种情况：

| 情况 | 队列变化 | 活跃 worker 的变化 |
| --- | --- | --- |
| 主机可用内存低于配置 reserve | 暂停新任务派发 | Worker 继续运行 |
| 请求安全取消 | Queued 任务直接取消；运行中的 worker 在 macro-step 边界停止 | Worker 收尾部分结果后退出 |
| 机器或 worker 进程突然停止 | Daemon 恢复前不再派发 | 活跃 attempt 会重新附着、根据终态 manifest 完成协调，或进入 interrupted |

## 为工作站上的长任务做准备

先查看资源池：

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get resources.memory_reserve_mib
```

再检查所选 Profile：

```text
trajecta --project PROJECT project show
trajecta --project PROJECT project get profiles.PROFILE
```

`execution.memory_budget_bytes` 是一个 worker 的请求。估算队列能够接收的最大工作量时，可
将它乘以可能同时活跃的 worker 数量。`memory_reserve_mib` 还要为 daemon、SQLite page
cache、气象 reader、操作系统和其他应用留下空间。

共享工作站可以在提交批次前减少 `resources.cpu_slots`，也可以提高 reserve。机器配置的
变化影响后续接收，不会改写现有 series 已保存的资源请求。

运行前再执行：

```text
trajecta --project PROJECT doctor --deep
trajecta job list
```

第一条命令检查所选文件系统、资料 reader、已有 lock 和临时 SQLite WAL 周期。第二条命令
显示当前已经占用本机资源池的任务。

## 运行期间观察内存

另开一个终端跟随任务：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

Resource event 包含 worker 与 daemon 可取得的运行观测。当前 scheduler reservation 可通过
以下命令查看：

```text
trajecta --format json job status JOB_ID
```

Daemon catalog 记录 Trajecta 的请求，不包含全部无关进程，因此主机总内存还可结合操作系统
工具查看。Windows 可使用任务管理器或资源监视器；Ubuntu 24.04 可使用 `free -h` 和 `ps`。

## 处理外部内存压力

对应 warning code 为：

```text
daemon.external_memory_pressure
```

它表示主机当前可用内存已经低于 `resources.memory_reserve_mib`。Attempt 会保持 queued，
同一次压力期间只写一条 warning，不会在每轮 scheduler 检查中重复刷屏。

可以按以下顺序处理：

1. 查看哪些 Trajecta 以外的程序改变了主机可用内存。
2. 活跃 Trajecta worker 的内存保持稳定时，可以让它完成当前任务。
3. 关闭或推迟其他占用大量内存的工作。
4. 跟随 `job events`，等待派发恢复。
5. 相同工作量反复触发压力时，在下一批任务前调整资源池或 Profile 请求。

需要让运行中的任务释放 reservation 时，可以请求安全取消：

```text
trajecta job cancel JOB_ID
trajecta job wait JOB_ID
```

Worker 会到达 macro-step 边界，完成部分结果收尾，然后进入 `cancelled`。等待时间包含当前
macro step 和输出收尾所需时间。

!!! tip "安全取消不会暂停模拟"

    后续 rerun 会从 resolved input 创建新 attempt，取消时的粒子状态不能作为续跑 checkpoint。

## 处理操作系统 OOM 终止

操作系统终止 worker 后，daemon 无法收到正常收尾。Catalog 会先记录 worker 丢失，再将
attempt 记为 interrupted。常见 event code 为：

```text
worker.lost
run.interrupted.worker_lost
```

读取最终 catalog 状态，并保留完整 attempt 目录：

```text
trajecta --format json job status JOB_ID
trajecta --format json job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

Interrupted 目录中可能存在 running manifest、部分 SQLite、WAL、worker 输出和临时
provenance 文件。这些文件对应旧 attempt 已经到达的位置。Rerun 会获得新的 run ID，并
使用单独目录。

再次运行前，可将观测到的峰值用量与 Profile 内存预算进行比较。常见调整包括：

- 减少同时接收的任务数量；
- thread-local working memory 较大时，降低 `execution.worker_threads`；
- 为 admission 设置符合实际的 `execution.memory_budget_bytes`；
- 共享工作站提高 `resources.memory_reserve_mib`；
- 将批次移到物理内存余量更大的机器。

Worker 被终止时若正在写 SQLite 或 provenance，可在提交新 attempt 前运行
`doctor --deep`，检查当前文件系统与 SQLite runtime。

## 在计划关机前停止

运行中的任务可以请求安全取消，并等待 `cancelled`：

```text
trajecta job cancel JOB_ID
trajecta job events JOB_ID --follow
```

存在多个任务时，先查看 `job list`，再逐个取消活跃 series，并等待各 current attempt。
Queued attempt 也可以直接取消，不会启动 worker。

全部 current attempt 进入终态后，机器上就没有活跃 result writer。下次需要继续计算时，
显式创建 rerun：

```text
trajecta job rerun JOB_ID
```

原有结果目录仍归属于 cancelled attempt。

## 在意外关机后恢复

开机后选择原机器配置和项目：

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
```

需要 daemon 的第一条运行类命令会启动本地实例。启动协调可能重新附着仍然存活的 worker，
也可能接收已经完成的终态 manifest，或将消失的 worker 标记为 interrupted。状态稳定后
再运行 `job rerun`。

关机后的完整检查顺序见[恢复流程](recovery.md)。
