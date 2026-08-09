---
title: 前台运行与任务队列
description: 以前台或后台方式提交 Trajecta 任务，了解资源准入，查看队列状态并等待运行结束。
---

# 前台运行与任务队列

所有正式运行都会进入同一个持久化本地队列。前台模式让提交命令一直等待，直到当前执行轮次到达
终态；后台模式在队列接受任务后立即返回回执。两种模式使用相同的工作进程、数值引擎和结果
目录，区别只在提交端是否继续等待。

## 提交前检查

使用项目模式时，先确认运行配置所属项目已经定稿：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT doctor --deep
```

`run` 可以接受项目及其中登记的运行配置，也可以直接接受案例和运行配置文件：

=== "项目模式"

    ```text
    trajecta --project PROJECT run --profile PROFILE
    ```

=== "直接文档模式"

    ```text
    trajecta run --case cases/study.yml --run-profile profiles/local.yml
    ```

经常重复运行同一研究时，项目模式更方便。项目索引会固定运行配置选择的案例、资料映射、资料锁
和结果目录。直接文档模式仍使用同一任务生命周期，只是每次显式指定文档组合。

## 前台运行

默认行为是前台等待：

```text
trajecta --project PROJECT run --profile PROFILE
```

任务持久写入队列后，命令先显示回执，然后持续跟踪当前执行轮次。只有终态为 `complete` 时，
命令退出码才是 `0`。`failed`、`cancelled`、`interrupted` 或
`completed_with_particle_errors` 返回退出码 `1`。

关闭终端只会断开前台客户端。已经被队列接受的任务继续由守护进程管理。使用回执中的任务系列
ID 可以从另一个终端重新连接：

```text
trajecta job status JOB_ID
trajecta job wait JOB_ID
```

!!! tip "关闭前台终端不会取消任务"

    需要停止计算时，请使用 `job cancel`。直接关闭提交终端后，工作进程仍会继续运行。

## 后台提交

加入 `--detach` 后，命令会在任务写入持久队列后返回：

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

回执包含三个主要标识：

| 字段 | 含义 |
| --- | --- |
| `job_series_id` | 逻辑任务的稳定 ID，后续重跑仍属于同一系列 |
| `run_id` | 当前这一执行轮次的唯一 ID |
| `attempt` | 该任务系列内从 1 开始的执行轮次编号 |

自动化程序适合保存完整回执。日常查询和重跑一般使用 `job_series_id`；结果命令还可以接受
`run_id` 或具体结果目录。

## 守护进程与本机配置

本机配置决定守护进程端点和 SQLite 任务数据库。首个运行时命令发现端点不可用时，会自动启动
本地守护进程。Windows 使用本机具名管道，Linux 使用 Unix 域套接字，端点不会监听网络接口。

队列空闲达到 `daemon.idle_shutdown_seconds` 后，守护进程可以自行退出。后续命令会重新启动
它，并打开原有任务数据库。将该值设为 `0`，守护进程会持续运行到操作系统停止它。

凡是通过任务数据库查询 `run`、`job` 或 `result`，都应选择同一份本机配置：

```text
trajecta --config configs/workstation.toml job list
trajecta --config configs/workstation.toml job status JOB_ID
```

若查询不到刚提交的任务，先比较两个终端的 `trajecta config path` 输出。

## 资源准入

运行配置中的 `worker_threads` 和 `memory_budget_bytes` 会转换为调度请求：

- CPU 槽位数等于工作线程数；
- 内存预算按 MiB 向上取整；
- 所有活跃执行轮次共享本机配置中的 CPU 和可调度内存池。

可调度内存按下式计算：

```text
resources.memory_pool_mib - resources.memory_reserve_mib
```

CPU 和内存同时有足够余量时，任务才能从 `queued` 进入 `starting`。暂时无法准入的任务会留在
持久队列中。调度器以先入先出为基础，并允许能填补空闲资源的小任务有限度地先启动。

当操作系统当前可用内存低于预留量时，守护进程会暂停派发新任务，并记录外部内存压力警告。
已经运行的工作进程保持其资源预留，待主机内存恢复后，排队任务会继续准入。

## 查看队列状态

列出每个可见任务系列的最新执行轮次：

```text
trajecta job list
```

当前命令最多返回 1,000 项。查看指定任务：

```text
trajecta --format json job status JOB_ID
```

常用字段如下：

| 字段 | 出现时机 |
| --- | --- |
| `state` | 始终存在 |
| `queue_position` | 任务仍在队列中时 |
| `started_at` | 工作进程开始启动后 |
| `finished_at` | 到达终态后 |
| `output_directory` | 执行轮次目录分配完成后 |
| `resources` | 始终存在，记录本次准入请求 |

常规状态顺序为：

```text
queued → starting → running → complete
```

取消、异常粒子和运行错误会进入其他终态。[运维手册](../operations/index.md)列出了完整状态及
守护进程或工作进程中断后的处理方式。

## 从另一个终端等待

`job wait` 会连接到现有任务系列，在当前执行轮次进入终态后退出：

```text
trajecta job wait JOB_ID
```

它适合后台提交后等待结果，也适合重新连接到仍在运行的任务。需要持续读取进度和资源状态时，
使用[任务事件](events.md)。

## 提交多个运行配置

每个运行配置单独使用 `--detach` 提交：

```text
trajecta --project PROJECT run --profile forward-wet-season --detach
trajecta --project PROJECT run --profile backward-wet-season --detach
trajecta --project PROJECT run --profile sensitivity-low-mixing --detach
trajecta job list
```

守护进程会启动资源池能够容纳的工作进程，其余执行轮次保持 `queued`。已经完成的执行轮次是
任务数据库中的终态记录。守护进程重启后不会再次提交或运行它们。

## 脚本中保存提交结果

一个请求对应一个响应时，使用 JSON：

```text
trajecta --format json --project PROJECT run --profile PROFILE --detach
```

响应对象包含 `ok`、完整命令路径、`data` 中的提交回执，以及存在时的诊断。机器模式下，应用
响应作为单个 JSON 写入标准输出。命令用法错误保留退出码 `2`，运行或产品错误使用退出码 `1`。
