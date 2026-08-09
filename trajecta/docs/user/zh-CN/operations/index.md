---
title: Trajecta 运维手册
description: 管理 Trajecta 本地队列与资源池，查看 worker 状态，并在 daemon 重启后继续使用任务记录和结果。
---

# 运维手册

Trajecta 的队列和 worker 都运行在本机。CLI 提交的命令先进入持久化任务目录，等待 CPU 与
内存资源，随后交给独立 worker 进程。Daemon 接收任务后，即使提交命令所在的终端关闭，
任务仍会留在队列中。

本章关注科学运行周围的机器管理工作，包括资源接收、队列观察、daemon 重启、worker
中断、存储维护和结果并发读取。Case 与 RunProfile 的配置方式见[概念](../concepts/index.md)
和[操作指南](../how-to/index.md)。

## 涉及的进程与文件

本机安装包含三类持久化状态：

| 项目 | 选择方式 | 保存内容 |
| --- | --- | --- |
| 机器配置 | `--config`、`TRAJECTA_CONFIG` 或平台默认路径 | 资源池、daemon endpoint、任务目录路径、监测间隔和本机 Profile 模板 |
| 任务目录 | 由机器配置选择 | Job series、attempt、状态转换、事件、worker lease 和验证状态 |
| 结果目录 | 由 Project 与 RunProfile 决定 | Run manifest、`particles.sqlite`、provenance bundle、报告及中断后保留的文件 |

Daemon 负责本机协调。Windows 使用 named pipe，Linux 使用 Unix socket。数值计算位于子
worker 进程中，因此模拟运行期间仍可查询队列。Daemon 空闲退出后，任务目录与结果目录
继续保留在磁盘上。

多个终端管理同一个队列时，应始终选择同一份配置：

```text
trajecta --config workstation.toml job list
trajecta --config workstation.toml job status JOB_ID
trajecta --config workstation.toml job events JOB_ID --follow
```

若命令选择了另一份配置，它可能会连接到同一台计算机上的另一个 daemon 和任务目录。

## Attempt 生命周期

首次提交会创建 job series 及其第一个 attempt。Rerun 保持 series 不变，同时生成新的
run ID 和 attempt number。

| 状态 | 运维含义 | 常见后续状态 |
| --- | --- | --- |
| `queued` | 请求已持久化，正在等待资源 | `starting` 或 `cancelled` |
| `starting` | 资源已预留，worker 正在启动 | `running`、`failed` 或 `interrupted` |
| `running` | Worker 已接管结果目录并推进模拟 | `complete`、`completed_with_particle_errors`、`cancelling`、`failed` 或 `interrupted` |
| `cancelling` | 安全取消正在等待数值边界 | `cancelled` 或 `interrupted` |
| `complete` | Run 已完整结束，没有异常粒子终止 | 终态 |
| `completed_with_particle_errors` | 输出已完成，同时存在异常粒子终止 | 终态 |
| `cancelled` | 协作式取消已完成输出收尾 | 终态 |
| `failed` | 启动、执行或输出收尾返回错误 | 终态 |
| `interrupted` | Worker 在正常收尾前消失或被强制停止 | 终态 |

当前快照可通过以下命令查看：

```text
trajecta --format json job status JOB_ID
```

事件记录展示快照之前发生的状态变化：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

读取事件不会消费记录。多个终端或监测程序可以各自跟随同一个队列。

## 资源接收

机器配置定义共享容量：

```text
resources.cpu_slots
resources.memory_pool_mib
resources.memory_reserve_mib
```

RunProfile 向 scheduler 提交两项请求：

| RunProfile 字段 | Scheduler 请求 |
| --- | --- |
| `execution.worker_threads` | 数值相同的 CPU slot |
| `execution.memory_budget_bytes` | 向上取整到完整 MiB 的内存 |

可调度内存为 `memory_pool_mib - memory_reserve_mib`。CPU 和内存请求同时放得进当前剩余容量
时，attempt 才会启动。资源预留用于限制 Trajecta 接收的总工作量；操作系统仍会受到其他
应用内存变化的影响。

### 队列顺序与 backfill

Scheduler 以先入先出顺序开始检查。最早的 attempt 暂时放不进剩余容量时，后面能够放入的
小任务可以先启动。Safe backfill 会利用空闲资源，同时继续保留队首任务的位置。

队首最多被越过三次。达到第三次后，后续任务会等待资源为队首释放。持续提交小任务时，
较大的请求仍可获得启动机会。

### 外部内存压力

Daemon 会比较主机可用内存与配置的 reserve。可用内存低于 reserve 时，新任务暂停派发。
运行中的 worker 保持已有资源预留；只要操作系统或操作者没有终止它，它会继续运行。

受到影响的 queued attempt 会在该次压力期间收到一条
`daemon.external_memory_pressure` warning。主机内存恢复后，队列继续派发。
[关机与内存压力](shutdown-memory.md)介绍如何区分这种暂停和操作系统终止 worker。

## Daemon 启动、空闲退出与重启

运行类命令会连接所选配置中的本机 endpoint。没有 daemon 监听时，CLI 会启动一个实例，
并等待它就绪。Daemon 持有 instance ID、PID 和进程启动 token，避免复用的 PID 被识别为
原有 worker owner。

队列没有活跃任务时，`daemon.idle_shutdown_seconds` 决定 daemon 保持运行的时长。空闲退出
只关闭进程和 endpoint，不会移除任务行、事件、结果目录或验证记录。值设为 `0` 后，daemon
会保持可用，直至机器或该进程停止。

Daemon 启动时会逐项协调持久化 attempt：

| Catalog 与 worker 状态 | 协调动作 |
| --- | --- |
| Worker identity 与 lease 仍然有效 | 重新附着并写入 `worker.reattached` |
| 已有身份匹配的有效终态 manifest | 协调为终态并写入 `worker.terminal_reconciled` |
| 活跃 catalog row 找不到对应 worker 或终态 manifest | 通过 `worker.lost` 和 `run.interrupted.worker_lost` 将 attempt 标记为 interrupted |
| Attempt 仍为 queued | 保持 queued，等待正常接收 |
| Attempt 已为终态 | 保持原状态，不创建新 attempt |

机器重启或进程异常退出后的命令顺序见[恢复流程](recovery.md)。

## 观察运行中的队列

日常检查可以结合三个视图：

```text
trajecta job list
trajecta --format json job status JOB_ID
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

`job list` 为每个可见 series 显示一个当前条目。`job status` 还会给出队列位置、资源请求、
run identity、时间戳和输出路径。事件流记录进度、资源采样、diagnostic 与产物转换。

长期运行的本地队列可以保存 `data.sequence` 中的持久化事件序号。监测程序重启后，使用
`--since SEQUENCE` 接续。JSONL 外层 sequence 只统计本次流中的行，不能作为接续游标。

## 运行期间读取结果

结果命令使用只读快照。Run 活跃时，SQLite reader 只读取当前 indexed high-water 以内的
数据，不参与 writer 的 checkpoint 与生命周期收尾。

运行中可以使用：

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 42
```

两次快照之间的行数可能增加。进入终态后，结果还会具备最终 manifest、完成收尾的
provenance，以及已经 checkpoint 或不存在的 WAL。此时再运行完整验证：

```text
trajecta result verify JOB_ID --full
```

直接读取 SQLite 时，可参照[结果参考](../reference/results-sqlite.md)中的只读结构和快照说明。

## 日常运行检查

开始一组长任务前：

1. 选择运行时使用的机器配置，并执行 `config validate`。
2. 对 finalized project 执行 `doctor --deep`。
3. 检查任务目录、资料目录和输出目录所在磁盘的剩余空间。
4. 将各 Profile 的请求与 CPU pool、可调度内存进行比较。
5. 以 detached 方式提交任务，并保存完整 JSON receipt。

任务完成后：

1. 使用 `job status` 确认终态。
2. 对准备保留或分析的结果运行 `result verify --full`。
3. 需要便于阅读的摘要时生成 `run-report.md`。
4. 归档或复制时，以完整终态结果目录为单位处理。
5. 需要估算可回收空间时查看 `job prune`；当前版本仅返回 dry-run plan。

相关流程：

- [关机与内存压力](shutdown-memory.md)
- [存储、SQLite 与 WAL](storage-sqlite.md)
- [恢复与中断 attempt](recovery.md)
- [按症状或 code 排查故障](troubleshooting.md)
