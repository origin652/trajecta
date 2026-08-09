---
title: 恢复中断的 Trajecta 任务
description: 在断电、worker 消失、强制取消、磁盘故障或输出收尾中断后，协调 Trajecta 任务并创建后续 attempt。
---

# 恢复与中断 attempt

恢复过程从持久化任务 catalog 和 attempt 目录已有文件开始。Daemon 返回后会读取两处状态，
然后重新附着身份匹配的活跃 worker，协调有效终态 manifest，或将已经消失的 worker 关闭为
interrupted attempt。

重启过程不会自行创建 rerun。已完成 attempt 保持完成，queued attempt 继续等待正常派发，
interrupted attempt 则等待显式 `job rerun`。

## 初始检查

读取初始状态期间，将机器配置、项目、任务 catalog 和 attempt 目录保持在原路径。SQLite
数据库应与 `-wal`、`-shm` sidecar 放在一起。

使用提交任务时的同一 `--config` 路径运行：

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
trajecta --config workstation.toml --format json job status JOB_ID
trajecta --config workstation.toml --format json job events JOB_ID --since 0
```

第一条 job 命令会启动本地 daemon 或重新连接。启动协调更新 catalog 后，再读取最近事件。

为每个受影响 attempt 记录以下值：

| 值 | 读取位置 | 用途 |
| --- | --- | --- |
| Job-series ID | 提交 receipt 或 `job status` | 后续状态和 rerun 命令选择同一逻辑任务 |
| Run ID 与 attempt number | `job status` 和事件 | 标识受影响的具体进程与结果目录 |
| 当前状态 | `job status` | 区分活跃、终态和仍在排队的任务 |
| 输出目录 | `job status` | 定位 manifest、SQLite、log 和临时文件 |
| 最后一个持久化事件序号 | `job events` | 作为后续事件读取的游标 |

## 读取协调结果

启动后的事件记录通常会出现以下路径之一：

| 事件或状态 | 含义 | 后续动作 |
| --- | --- | --- |
| `worker.reattached` 与 `running` | 已持有的 worker 仍存活，lease 匹配 | 继续监测，不移动其输出目录 |
| `worker.terminal_reconciled` 与终态 | Worker 已经写入身份匹配的终态 manifest | Inspect 并验证终态结果 |
| `worker.lost` 随后出现 `run.interrupted.worker_lost` | 找不到身份匹配的活跃 worker 或有效终态收尾 | 检查部分目录，修正原因，再按需要 rerun |
| `queued` | Attempt 尚未开始 | 继续排队，或根据当前安排取消 |
| 已有终态且没有新 dispatch event | Catalog 原本已将 attempt 视为完成 | 保持终态；需要再次计算时显式 `job rerun` |

`worker.lease_attach_failed` 表示 daemon 无法将已经接收的 lease 附着到启动进程。Ownership
仍无法恢复时，`worker.uncontrolled_after_attach_failure` 会记录 containment failure。创建
相同输入的新 worker 前，应先检查进程列表与 attempt 目录。

## 检查 attempt 目录

先使用 product-level reader：

```text
trajecta --format json result inspect JOB_ID
```

`result inspect` 可以解析 job-series ID、run ID 或直接结果路径。Manifest 可读而 SQLite
不可用时，命令会返回 partial inspection 和 `result.inspect_sqlite_unavailable`，其中仍有
manifest identity 与 artifact inventory。

随后以只读方式查看目录：

| 文件或目录 | 需要记录的内容 |
| --- | --- |
| `run-manifest.json` | Lifecycle status、run identity、count、input identity 和已记录错误 |
| `particles.sqlite` | 是否存在、字节大小，以及 result inspection 能否打开 |
| `particles.sqlite-wal` 与 `particles.sqlite-shm` | Writer 是否在 checkpoint 和收尾前停止 |
| Provenance 目录或 bundle | Provenance closeout 已经开始还是已经完成 |
| Worker stdout 与 stderr | Native library 消息、allocation failure 或 write error |
| 临时与 forensic entry | Worker 或 daemon 在失败收尾期间保留的路径 |

!!! warning "保留原 attempt"

    Rerun 使用单独目录。旧 manifest、数据库、sidecar 和 forensic file 应成组保留。

终态为 `complete` 时运行：

```text
trajecta result verify JOB_ID --full
```

Interrupted attempt 常会缺少 full verification 所需的产物。Partial inspection 仍可用于查看
最后 manifest 状态和已经写入的 SQLite 行。

## 断电后的恢复

1. 确认项目、资料根、输出根和机器配置已经挂载到原路径。
2. 运行 `config validate` 与项目 `doctor --deep`。
3. 通过 `job list` 启动 daemon。
4. 对停电前活跃的每个 series 读取 `job status` 和全部事件。
5. 等待协调得到稳定的活跃状态或终态。
6. 检查所有受影响的 attempt 目录。
7. 对协调为 complete 的结果运行 full verification。
8. 只为需要再次计算的任务创建 rerun。

输出文件系统若在恢复后报告错误，可先复制完整的非活跃 attempt 目录，再进行后续文件系统
修复。复制时保持 SQLite 主库与 sidecar 位于一起。

## 安全取消与强制取消后的恢复

安全取消会在 worker 到达 macro-step 边界并完成输出收尾后进入 `cancelled`。此类部分 run
通常可以读取，终态 WAL 已 checkpoint 或不存在。

强制取消会立即停止已持有的 worker，并记录 `interrupted`。对应目录可能保留非空 WAL、
running manifest 或未完成的 provenance 临时文件。读取时将它视为 partial attempt，并保持
文件组合不变。

创建后续 attempt 前，先检查最终状态：

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

Rerun 保持 series identity，并创建单独 attempt：

```text
trajecta --format json job rerun JOB_ID
```

需要修改 Case、Profile、data lock 或资源请求时，可编辑项目并提交新的 run。Rerun 使用原
series 已保存的输入。

## 内存压力或 OOM 后的恢复

`daemon.external_memory_pressure` 会暂停 queued dispatch，同时让活跃 worker 继续运行。主机
内存回到 reserve 以上后，scheduler 会继续工作。单纯保持 queued 的 attempt 无需恢复动作。

操作系统 OOM 终止会表现为 worker loss 和 interruption。Rerun 前可以：

1. 读取旧 run 的 resource event 和操作系统内存记录。
2. 将 working set 与 `execution.memory_budget_bytes` 比较。
3. 减少并发接收量，提高机器 reserve，或调整 Profile request 以符合实际工作量。
4. SQLite 或 provenance 写入被中断时运行 deep doctor。

资源设置方法见[关机与内存压力](shutdown-memory.md)。

## 磁盘耗尽后的恢复

先恢复足够可用空间，不移动活跃结果目录。确认没有 worker 继续写受影响目录后：

1. 保持主数据库与 sidecar 位于一起。
2. 运行项目 `doctor --deep`，检查当前文件系统上的 create、sync、rename、WAL 和 cleanup。
3. Inspect 结果并读取 terminal event。
4. 对协调为 complete 的 run 执行验证。
5. 存储问题修正后，将 failed 或 interrupted 任务 rerun 到新的 attempt 目录。

[存储指南](storage-sqlite.md)还介绍 WAL 处理、输出体积估算和终态结果迁移。

## 队列恢复期间的已完成任务

Daemon 不会再次派发终态 attempt，其中包括 `complete`、
`completed_with_particle_errors`、`failed`、`cancelled` 和 `interrupted`。新的 daemon 进程会
从同一 catalog 读取这些状态。

需要另一个 attempt 时运行 `job rerun`。后续 complete attempt 通过 full verification 后，
较早的 attempt 可能出现在 dry-run prune plan 中：

```text
trajecta --format json job prune
```

`0.1.0-alpha.1` 中的 plan 只用于查看，不会删除结果目录或 catalog history。
