---
title: 关机与内存压力
description: 为长时间 Trajecta 运行准备工作站，处理外部内存压力，并恢复被操作系统终止的工作进程。
---

# 关机与内存压力

长时间轨迹计算会与操作系统文件缓存、交互程序和其他科研软件共享内存。Trajecta 通过调度资源
预留限制自身同时准入的任务数量，并在启动新工作进程前单独检查主机当前可用内存。

本页区分三种情况：

| 情况 | 对队列的影响 | 对活跃工作进程的影响 |
| --- | --- | --- |
| 主机可用内存低于配置预留 | 暂停派发新任务 | 继续运行 |
| 请求安全取消 | 排队任务直接取消；运行任务等待宏步边界 | 收尾部分结果后退出 |
| 计算机或工作进程突然停止 | 守护进程恢复前不再派发 | 重连、按终态清单协调，或标记中断 |

## 长任务前准备工作站

查看本机资源池：

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get resources.memory_reserve_mib
```

再查看所选运行配置：

```text
trajecta --project PROJECT project show
trajecta --project PROJECT project get profile.PROFILE
```

`execution.memory_budget_bytes` 是一个工作进程的请求。估算队列满负载时的内存需求，可以将它
乘以可能同时活跃的工作进程数。`memory_reserve_mib` 还要为守护进程、SQLite 页缓存、气象读取器、
操作系统和其他应用留出空间。

共享工作站可以在提交批次前降低 `resources.cpu_slots`，或提高内存预留量。更改本机配置只影响
之后的资源准入，不会改写已有任务系列保存的资源请求。

最后运行：

```text
trajecta --project PROJECT doctor --deep
trajecta job list
```

第一条命令检查所选文件系统、气象读取器、现有资料锁和临时 SQLite/WAL 流程；第二条命令显示
当前已经占用本机资源池的任务。

## 运行期间观察内存

在另一个终端跟踪事件：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

资源事件会包含工作进程和守护进程能够采集到的资源信息。当前调度预留可通过以下命令查看：

```text
trajecta --format json job status JOB_ID
```

任务数据库只记录 Trajecta 的请求和采样。查看整机内存时，Windows 可使用任务管理器或资源
监视器；Ubuntu 24.04 可使用 `free -h` 和 `ps`。

## 处理外部内存压力

对应警告码为：

```text
daemon.external_memory_pressure
```

它表示主机当前可用内存低于 `resources.memory_reserve_mib`。执行轮次保持 `queued`，一次压力
期间只记录一条警告，避免每次调度轮询重复写入。

可以按以下顺序处理：

1. 查看哪些非 Trajecta 程序最近增加了内存占用。
2. 活跃工作进程内存稳定时，让它完成当前运行。
3. 关闭或推迟无关的大内存任务。
4. 跟踪 `job events`，确认队列何时恢复派发。
5. 同类负载反复触发压力时，在下一批任务前调整资源池或运行配置请求。

需要释放活跃任务的资源时，可安全取消：

```text
trajecta job cancel JOB_ID
trajecta job wait JOB_ID
```

工作进程到达宏步边界，收尾部分结果并进入 `cancelled`。等待时间包括当前宏步和输出收尾。

!!! tip "安全取消不会生成续算检查点"

    后续 `job rerun` 会使用解析后的输入创建新执行轮次，从起始状态重新计算。

## 操作系统因内存不足终止工作进程

操作系统终止工作进程后，守护进程无法收到正常收尾。任务事件通常依次出现：

```text
worker.lost
run.interrupted.worker_lost
```

读取最终状态，并保留完整执行轮次目录：

```text
trajecta --format json job status JOB_ID
trajecta --format json job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

中断目录可能包含 `running` 运行清单、部分 SQLite、WAL、工作进程输出和临时溯源文件。检查这些
文件可以了解旧执行轮次停止时的状态；重跑会获得新运行 ID 和独立目录。

重新运行前，将观察到的峰值内存与运行配置预算比较。常见调整包括：

- 减少同时准入的任务数；
- 线程本地工作内存较多时，降低 `execution.worker_threads`；
- 让 `execution.memory_budget_bytes` 反映实际负载；
- 共享工作站提高 `resources.memory_reserve_mib`；
- 将批次移到物理内存余量更大的计算机。

若工作进程终止时正在写 SQLite 或溯源信息，提交下一轮前运行 `doctor --deep`，检查当前文件系统
和 SQLite 运行环境。

## 计划关机前停止任务

对运行任务请求安全取消，并等待 `cancelled`：

```text
trajecta job cancel JOB_ID
trajecta job events JOB_ID --follow
```

有多个任务时，先查看 `job list`，逐个取消活跃任务并等待其当前执行轮次。排队任务可以在工作
进程启动前直接取消。

所有当前执行轮次进入终态后，结果目录不再有活跃写入端。开机后需要重新计算时，显式执行：

```text
trajecta job rerun JOB_ID
```

被取消的旧结果目录继续属于原执行轮次。

## 意外关机后恢复

开机后使用提交时相同的本机配置和项目：

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
```

首个任务命令会在需要时启动本地守护进程。启动协调可能重连仍存活的工作进程，根据已经写好的
终态运行清单完成状态协调，或把失联工作进程标记为中断。等待状态稳定后再执行 `job rerun`。

完整操作顺序见[恢复中断执行轮次](recovery.md)。
