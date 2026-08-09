---
title: 前台运行 Trajecta 或提交本地队列
description: 提交前台与后台任务，理解资源接收过程，查看队列状态并等待运行结束。
---

# 前台运行与任务队列

所有正式运行都进入同一个持久化本地队列。前台模式让提交命令一直等待，直到 attempt 进入
终态。后台模式在队列接收任务后返回 receipt。两种模式使用相同的数值 worker 和结果目录
结构。

## 准备提交

项目模式下，先确认所选 Profile 位于 finalized 项目中：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

`run` 可以接收项目与 Profile 的组合，也可以接收显式 Case 和 RunProfile 路径。

=== "项目模式"

    ```text
    trajecta --project PROJECT run --profile PROFILE
    ```

=== "直接文档模式"

    ```text
    trajecta run --case cases/study.yml --run-profile profiles/local.yml
    ```

项目模式适合重复运行，项目索引会固定 Profile 所选的 Case、资料映射、lock 和输出位置。
直接文档模式使用同一套队列生命周期，输入来自一对明确的文档。

## 在前台运行

默认行为是前台等待：

```text
trajecta --project PROJECT run --profile PROFILE
```

任务持久化接收后，命令输出 receipt，并跟随任务直到结束。只有 `complete` 终态返回退出码
`0`。运行失败、取消、中断或含粒子异常的终态返回 `1`。

关闭终端会断开前台 client，已经接收的 attempt 继续由 daemon 托管。使用 receipt 中的
job series ID 重新连接：

```text
trajecta job status JOB_ID
trajecta job wait JOB_ID
```

## 提交后立即返回

终端需要在接收后立即返回时加入 `--detach`：

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

Receipt 中有三个常用身份字段：

| 字段 | 含义 |
| --- | --- |
| `job_series_id` | 逻辑任务及后续 rerun 共用的稳定身份 |
| `run_id` | 当前这个 attempt 的身份 |
| `attempt` | series 内从 1 开始的 attempt 序号 |

自动化流程适合保存完整 receipt。日常 `job` 命令通常使用 `job_series_id`；结果命令可以解析
job series ID、run ID 或结果目录路径。

!!! tip "两个任务身份都保留"

    跟踪和 rerun 使用 series ID。脚本需要锁定某次 attempt 时，再使用 run ID。

## Local daemon 的选择方式

本机配置决定 daemon endpoint 及其 SQLite 任务 catalog。运行类命令首次发现 endpoint 无法
连接时，会自动启动 local daemon。Windows 使用本机 named pipe，Linux 使用本机 Unix
socket，不在网络接口上监听。

Daemon 空闲达到 `daemon.idle_shutdown_seconds` 后退出。后续命令可以再次启动它，并读取同一
份持久化 catalog。将该值设为 `0` 时，daemon 会保持运行，直到操作系统停止进程。

需要读取同一 catalog 的 run、job 和 result 命令选择同一份本机配置：

```text
trajecta --config configs/workstation.toml job list
trajecta --config configs/workstation.toml job status JOB_ID
```

## 理解资源接收

所选 RunProfile 提供 `worker_threads` 和 `memory_budget_bytes`。提交时，Trajecta 将其转换为
scheduler request：

- CPU slot 数量等于 worker thread 数量；
- 内存请求由字节预算向上取整到 MiB；
- 所有活动 attempt 共享本机配置中的 CPU 和可调度内存池。

可调度内存等于 `resources.memory_pool_mib` 减去 `resources.memory_reserve_mib`。CPU 和内存
同时可用后，任务才能离开 `queued`。在资源允许接收的任务之间，队列按 FIFO 顺序推进。

操作系统当前可用内存低于 reserve 时，daemon 暂停新的 dispatch，并记录外部内存压力
warning。正在运行的 worker 保留已有资源预留。

## 查看队列状态

列出日常可见的 job series 及其最新 attempt：

```text
trajecta job list
```

当前命令最多返回 1,000 项。查看指定 series 时使用：

```text
trajecta --format json job status JOB_ID
```

常用字段如下：

| 字段 | 出现时机 |
| --- | --- |
| `state` | 始终存在 |
| `queue_position` | attempt 仍在队列中 |
| `started_at` | worker 开始启动后 |
| `finished_at` | 进入终态后 |
| `output_directory` | attempt 目录完成分配后 |
| `resources` | 始终存在，记录本次接收的资源请求 |

常规状态顺序为：

```text
queued → starting → running → complete
```

取消和运行故障会形成其他终态路径。[运维手册](../operations/index.md)说明各状态，以及 daemon
或 worker 中断后的恢复行为。

## 从另一个终端等待

`job wait` 连接到已有 series，并在当前 attempt 进入终态时退出：

```text
trajecta job wait JOB_ID
```

它可用于 detached 提交，也可用于重新连接一台机器。需要持续进度时，可读取
[`job events`](events.md)。

## 运行多个 Profile

逐个使用 `--detach` 提交：

```text
trajecta --project PROJECT run --profile forward-wet-season --detach
trajecta --project PROJECT run --profile backward-wet-season --detach
trajecta --project PROJECT run --profile sensitivity-low-mixing --detach
trajecta job list
```

Daemon 会在共享资源池范围内接收多个 worker，其余 attempt 保持持久化 `queued`。已经完成
的 attempt 是 catalog 中的终态记录；daemon 重启不会再次提交或 dispatch 它们。

## 在脚本中保存提交输出

一次请求和一次响应可以使用 JSON：

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

Envelope 包含 `ok`、完整命令路径、位于 `data` 中的 receipt，以及可能出现的 diagnostics。
机器模式的应用响应作为一个 stdout envelope 输出。Usage error 保留退出码 `2`，运行或产品
错误使用退出码 `1`。
