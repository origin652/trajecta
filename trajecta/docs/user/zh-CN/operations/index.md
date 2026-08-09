---
title: Trajecta 运维手册
description: 管理本地任务队列与资源池，监测工作进程，并在守护进程重启后继续使用任务记录和结果。
---

# 运维手册

Trajecta 的任务队列和工作进程都运行在本机。CLI 提交的任务先写入持久化任务数据库，等待 CPU
与内存资源，然后由独立工作进程执行。守护进程接受任务后，关闭提交终端不会移除任务。

本章介绍运行科学计算时涉及的本机管理工作，包括资源准入、队列监测、守护进程重启、工作进程中断、
存储维护和结果并发读取。案例与运行配置的设置方法见[概念](../concepts/index.md)和
[操作指南](../how-to/index.md)。

## 进程与持久文件

本机安装会保存三类持久数据：

| 项目 | 由什么选择 | 保存内容 |
| --- | --- | --- |
| 本机配置 | `--config`、`TRAJECTA_CONFIG` 或平台默认路径 | 资源池、守护进程端点、任务数据库路径、采样间隔和本机运行模板 |
| 任务数据库 | 所选本机配置 | 任务系列、执行轮次、状态变化、事件、工作进程租约和验证状态 |
| 结果目录 | 项目和运行配置 | 运行清单、`particles.sqlite`、溯源信息、报告及中断文件 |

守护进程负责本机协调。Windows 使用具名管道，Linux 使用 Unix 域套接字。数值计算在子工作进程
中进行，因此模拟运行时仍可正常查询队列。守护进程因空闲退出后，任务数据库和结果目录继续保留
在磁盘上。

多个终端管理同一队列时，应选择同一配置：

```text
trajecta --config workstation.toml job list
trajecta --config workstation.toml job status JOB_ID
trajecta --config workstation.toml job events JOB_ID --follow
```

指向另一配置的命令可能连接到同一计算机上的另一个守护进程和任务数据库。

## 执行轮次生命周期

首次提交创建任务系列和第一轮执行。重跑保留任务系列，创建新的运行 ID 和轮次编号。

| 状态 | 运维含义 | 常见后续状态 |
| --- | --- | --- |
| `queued` | 请求已持久化，正在等待资源准入 | `starting` 或 `cancelled` |
| `starting` | 资源已经预留，工作进程正在启动 | `running`、`failed` 或 `interrupted` |
| `running` | 工作进程拥有结果目录并推进模拟 | `complete`、`completed_with_particle_errors`、`cancelling`、`failed` 或 `interrupted` |
| `cancelling` | 安全取消已记录，等待数值宏步边界 | `cancelled` 或 `interrupted` |
| `complete` | 运行完整结束，异常粒子终止数为零 | 终态 |
| `completed_with_particle_errors` | 输出已完成，但存在异常粒子终止 | 终态 |
| `cancelled` | 协作式取消已完成部分结果收尾 | 终态 |
| `failed` | 启动、执行或输出收尾返回错误 | 终态 |
| `interrupted` | 工作进程在正常收尾前消失或被强制停止 | 终态 |

查看当前快照：

```text
trajecta --format json job status JOB_ID
```

查看形成该快照的状态变化：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

读取事件不会消费记录，多个终端或监测程序可以独立跟踪同一队列。

## 资源准入

本机配置定义共享容量：

```text
resources.cpu_slots
resources.memory_pool_mib
resources.memory_reserve_mib
```

运行配置向调度器提交：

| 运行配置字段 | 调度请求 |
| --- | --- |
| `execution.worker_threads` | 相同数量的 CPU 槽位 |
| `execution.memory_budget_bytes` | 向上取整到完整 MiB 的内存 |

可调度内存为 `memory_pool_mib - memory_reserve_mib`。CPU 和内存请求都能放入当前剩余容量时，
执行轮次才会启动。资源预留限制 Trajecta 自己同时接受的工作量；主机上其他程序的内存变化由
操作系统共同反映。

### 队列顺序与空闲资源回填

调度器以先入先出为基础。最早的执行轮次暂时放不进剩余资源时，后面能够容纳的小任务可以先启动，
利用闲置 CPU 或内存。

队首任务最多被越过三次。达到上限后，后续任务等待资源为队首释放。这样，即使不断有小任务加入，
较大的早期请求仍能获得启动机会。

### 外部内存压力

守护进程比较主机可用内存与配置的预留量。可用内存低于预留量时，新任务暂停派发；运行中的
工作进程保留已有资源预留，并继续运行，除非操作系统或操作者停止它。

受影响的排队执行轮次在一次压力期间收到一条
`daemon.external_memory_pressure` 警告。主机内存恢复后，队列继续派发。
[关机与内存压力](shutdown-memory.md)说明如何区分队列暂停和工作进程被操作系统终止。

## 守护进程启动、空闲退出与重启

运行时命令连接到所选本机端点。没有守护进程监听时，CLI 会启动一个守护进程并等待就绪。守护
进程的标识由实例 ID、进程 ID 和进程启动标记组成，用来避免操作系统复用 PID 后误认进程。

队列没有活跃工作时，`daemon.idle_shutdown_seconds` 决定守护进程保持多久。空闲退出只关闭
进程和端点，不会删除任务数据库行、事件、结果目录或验证记录。将其设为 `0` 可保持守护进程
持续可用。

重启时，守护进程按持久状态协调每个执行轮次：

| 任务数据库与工作进程状态 | 协调动作 |
| --- | --- |
| 工作进程标识和租约仍有效 | 重新连接，并记录 `worker.reattached` |
| 已有匹配的有效终态运行清单 | 协调终态，并记录 `worker.terminal_reconciled` |
| 活跃记录没有匹配工作进程或终态清单 | 通过 `worker.lost` 和 `run.interrupted.worker_lost` 标记中断 |
| 执行轮次仍为 `queued` | 保持排队，继续正常资源准入 |
| 执行轮次已经终态 | 保持终态，不创建新轮次 |

[恢复操作手册](recovery.md)给出重启、断电或进程失联后的命令顺序。

## 监测活跃队列

日常可以结合三种视图：

```text
trajecta job list
trajecta --format json job status JOB_ID
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

`job list` 为每个可见任务系列显示当前轮次。`job status` 补充队列位置、资源请求、运行 ID、
时间戳和结果路径。事件流记录进度、资源采样、诊断和产物变化。

长时间监测时，保存事件中的 `data.sequence`。监测客户端重启后，以
`--since SEQUENCE` 续读。外层 JSONL `sequence` 只统计本次命令流中的行，不能用于持久续读。

## 运行期间读取结果

结果命令使用只读快照。工作进程仍在写入时，SQLite 读取器只观察当前已建立索引的高水位范围，
不会接管写入端的检查点或生命周期收尾。

可用的实时检查：

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 42
```

不同快照之间的数量可能增长。到达终态后，结果会增加最终运行清单和完整溯源信息，WAL 也会完成
检查点并截断或消失。完整验证应在终态后运行：

```text
trajecta result verify JOB_ID --full
```

直接读取 SQLite 时，遵循[SQLite 结果参考](../reference/results-sqlite.md)中的只读连接和顺序要求。

## 长任务前后的检查

提交长任务前：

1. 确认 `config path` 指向预期本机配置，并运行 `config validate`。
2. 对已经定稿的项目运行 `doctor --deep`。
3. 检查任务数据库、资料目录和结果目录所在文件系统的可用空间。
4. 比较各运行配置请求与 CPU 池、可调度内存。
5. 后台提交任务，并保存完整 JSON 回执。

任务完成后：

1. 用 `job status` 确认终态。
2. 对将要保留或分析的结果运行 `result verify --full`。
3. 需要人类可读摘要时生成 `run-report.md`。
4. 将完整终态结果目录作为一个单元复制或归档。
5. 用 `job prune` 估算被替代执行轮次的空间；当前版本只返回只读计划。

相关操作：

- [关机与内存压力](shutdown-memory.md)
- [存储、SQLite 与 WAL](storage-sqlite.md)
- [恢复中断执行轮次](recovery.md)
- [按症状与诊断码排查](troubleshooting.md)
