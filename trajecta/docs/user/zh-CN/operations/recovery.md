---
title: 关机、OOM、磁盘故障与输出中断后的恢复
description: 突然关机、worker 丢失、内存压力、磁盘耗尽或 SQLite/WAL 异常后，保留证据并恢复 Trajecta 任务。
---

# 恢复与 forensic

恢复工作的第一步是保留状态。初始评估期间，请勿删除临时文件、WAL、任务行和部分结果
目录。

## 突然关机或 worker 丢失

1. 机器重启后，使用相同配置运行 `trajecta doctor --deep`。
2. 运行 `trajecta job list`，再对非终态任务执行 `job status JOB_ID`。
3. 读取 `job events JOB_ID --since 0`，记录最后一个持久化状态转换。
4. 只读检查运行目录。
5. 允许 daemon reconciliation 将无法重新附着的 worker 标记为 interrupted。
6. Attempt 进入稳定终态后，才执行 `job rerun JOB_ID`。

终态 manifest 的 run identity 与 catalog 一致时，恢复后的 manifest 具有权威性。Worker
消失且没有有效终态 manifest 时，该 attempt 进入 interrupted 状态。已有的部分 SQLite、
WAL、manifest、临时文件和 forensic 文件均需保留。

## OOM 与外部内存压力

外部压力会针对受影响 queued attempt 写入一次 `daemon.external_memory_pressure`，并暂停
派发。可以等待内存恢复，或降低后续任务的资源请求。操作系统未终止 worker 时，运行中
任务继续执行。

操作系统导致的 worker 丢失会被协调为 interruption。请保存系统事件、任务事件、worker
stderr、manifest 状态和峰值内存证据。确认真实 working set 后，再降低
`execution.memory_budget_bytes`、减少并行任务或提高配置 pool。

## 磁盘耗尽或写入失败

先停止接收新任务。在相同文件系统上恢复可用空间，避免触碰活跃结果目录。随后运行 deep
doctor，检查输出根，并逐一查看受影响任务。写入失败可能留下部分 manifest、SQLite
sidecar 或 provenance 临时文件；这些文件均应保留。

终态结果通过 full verify 后，才能以完整目录为单位迁移。不要移动正在写入的文件来改变
live writer 的目标。

## SQLite 与 WAL 异常

不要通过删除 `particles.sqlite-wal` 或 `jobs.sqlite3-wal` 来消除警告。先复制完整的非活跃
目录用于 forensic。结果目录使用 `result inspect` 与 `result verify`；任务数据库环境使用
`doctor --deep`。成功终态要求 SQLite integrity 通过，并且终态 WAL 为零字节或不存在。

Integrity 失败时，将原始字节保持只读。Rerun 会创建新 attempt，不修补或替换损坏证据。

## 队列恢复中的已完成任务

Daemon 重启不会再次提交已完成任务。请核对 series、run ID、attempt number、终态
manifest 和 artifact fingerprint。`job forget` 只从日常列表移除终态记录，产物和审计
摘要继续保留。当前版本的 `job prune` 仅预览候选项。
