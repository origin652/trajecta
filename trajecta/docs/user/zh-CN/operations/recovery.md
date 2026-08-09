---
title: 恢复中断执行轮次
description: 在断电、工作进程失联、强制取消、磁盘故障或输出收尾中断后，协调任务状态并创建安全重跑。
---

# 恢复中断执行轮次

恢复时，先保留并检查任务数据库和执行轮次目录中已有的文件。守护进程重新启动后，会根据两者
判断应当重连仍存活的工作进程、接收有效终态清单，还是把失联工作进程对应的执行轮次标记为中断。

重启不会自动创建重跑。已完成执行轮次保持完成，排队任务继续等待正常资源准入，中断执行轮次则
等待显式 `job rerun`。

## 第一轮检查

读取初始状态期间，保持本机配置、项目、任务数据库和执行轮次目录位于原位置。尤其要让 SQLite
主库与 `-wal`、`-shm` 伴随文件留在一起。

使用提交时相同的 `--config`：

```text
trajecta --config workstation.toml config validate
trajecta --config workstation.toml --project PROJECT doctor --deep
trajecta --config workstation.toml job list
trajecta --config workstation.toml --format json job status JOB_ID
trajecta --config workstation.toml --format json job events JOB_ID --since 0
```

首条任务命令会启动或连接本地守护进程。等待启动协调更新任务数据库后，再读取最新事件。

为每个受影响执行轮次记录：

| 值 | 从哪里读取 | 用途 |
| --- | --- | --- |
| 任务系列 ID | 提交回执或 `job status` | 后续状态、事件和重跑命令 |
| 运行 ID 与轮次编号 | `job status` 和事件 | 确定受影响进程与结果目录 |
| 当前状态 | `job status` | 判断仍活跃、已终态或仍排队 |
| 结果目录 | `job status` | 定位运行清单、SQLite、日志和临时文件 |
| 最后持久事件序号 | `job events` | 后续续读游标 |

## 判断启动协调结果

启动后的事件通常对应以下路径：

| 事件或状态 | 含义 | 后续操作 |
| --- | --- | --- |
| `worker.reattached` 且状态为 `running` | 原工作进程仍存活，租约匹配 | 继续监测，不移动结果目录 |
| `worker.terminal_reconciled` 和终态 | 工作进程已经写出匹配的终态清单 | 检查并完整验证结果 |
| `worker.lost` 后出现 `run.interrupted.worker_lost` | 没有匹配工作进程，也没有有效终态收尾 | 检查部分目录，修复原因后按需重跑 |
| `queued` | 断电前尚未启动 | 保持排队，或按当前计划取消 |
| 原有终态且没有新派发事件 | 任务数据库已认为该轮结束 | 保持终态，需要新执行时显式重跑 |

`worker.lease_attach_failed` 表示守护进程未能把已接受租约连接到新进程。若进程所有权也无法恢复，
还会出现 `worker.uncontrolled_after_attach_failure`。此时先检查进程列表和执行轮次目录，等待
状态协调完成，再按需要为同一输入创建重跑轮次。

## 检查执行轮次目录

先使用结果读取命令：

```text
trajecta --format json result inspect JOB_ID
```

`result inspect` 可以接受任务系列 ID、运行 ID 或直接目录路径。运行清单可读而 SQLite 不可用时，
命令返回部分信息和 `result.inspect_sqlite_unavailable`，仍会列出运行 ID 和文件清单。

随后以只读方式查看目录：

| 文件 | 需要关注的内容 |
| --- | --- |
| `run-manifest.json` | 生命周期状态、运行 ID、数量、输入文件散列和记录的失败 |
| `particles.sqlite` | 是否存在、文件大小、结果命令能否打开 |
| `particles.sqlite-wal` 与 `particles.sqlite-shm` | 写入端是否在检查点前停止 |
| 溯源目录或溯源信息文件 | 正式收尾是否开始或完成 |
| 工作进程标准输出与错误输出 | 原生库消息、内存分配失败或写入错误 |
| 临时和中断文件 | 工作进程或守护进程在收尾失败时保留的路径 |

!!! warning "保留原执行轮次"

    重跑会使用独立目录。原运行清单、数据库、伴随文件和中断文件应一起保留。

终态为 `complete` 时运行：

```text
trajecta result verify JOB_ID --full
```

中断执行轮次常常缺少完整验证所需的终态产物，但部分检查仍可定位最后运行清单状态和已写入
SQLite 行。

## 断电后的恢复顺序

1. 确认项目、资料目录、结果根目录和本机配置挂载到原路径。
2. 运行 `config validate` 和项目 `doctor --deep`。
3. 通过 `job list` 启动守护进程。
4. 对断电前活跃的任务系列读取 `job status` 和全部事件。
5. 等待协调形成稳定的活跃状态或终态。
6. 检查每个受影响执行轮次目录。
7. 对协调为 `complete` 的结果运行完整验证。
8. 只为确实需要再次计算的任务创建重跑。

文件系统若在断电后报告错误，进一步修复前先复制已经停止写入的完整执行轮次目录，并保持 SQLite
主库与伴随文件在一起。

## 安全取消与强制取消后的恢复

安全取消在工作进程到达数值宏步边界并完成输出收尾后进入 `cancelled`。这类部分结果通常可以
检查，终态 WAL 应已经完成检查点或不存在。

强制取消会立即停止工作进程并记录 `interrupted`。目录中出现非空 WAL、`running` 清单或未完成
溯源临时文件都符合这种生命周期。

创建新执行轮次前先检查：

```text
trajecta job status JOB_ID
trajecta job events JOB_ID --since 0
trajecta result inspect JOB_ID
```

按原输入重跑：

```text
trajecta --format json job rerun JOB_ID
```

需要修改案例、运行配置、资料锁或资源请求时，编辑项目并提交新任务系列。

## 外部内存压力或内存不足

`daemon.external_memory_pressure` 只暂停排队任务派发，不影响已经运行的工作进程。主机可用内存
回到预留量以上后，正常调度会自动继续，单纯保持排队的任务不需要恢复操作。

操作系统因内存不足终止工作进程时，会表现为工作进程失联和 `interrupted`。重跑前：

1. 读取旧执行轮次的资源事件和操作系统内存记录。
2. 将工作集与 `execution.memory_budget_bytes` 比较。
3. 减少并发准入，提高本机内存预留，或调整运行配置请求。
4. SQLite 或溯源写入被打断时运行深度环境检查。

更多资源设置见[关机与内存压力](shutdown-memory.md)。

## 磁盘空间耗尽

先恢复足够空间，并避免移动活跃结果目录。确认没有工作进程继续写入后：

1. 保持 SQLite 主库和伴随文件在一起。
2. 运行项目 `doctor --deep`，检查当前文件系统上的创建、同步、重命名、WAL 和清理。
3. 检查结果与终态事件。
4. 对协调为 `complete` 的结果运行验证。
5. 修复存储后，将失败或中断任务重跑到新执行轮次目录。

[存储、SQLite 与 WAL](storage-sqlite.md)说明 WAL 处理、结果大小估算和终态目录移动。

## 队列恢复时如何处理已完成任务

守护进程不会重新派发任何终态执行轮次，包括 `complete`、
`completed_with_particle_errors`、`failed`、`cancelled` 和 `interrupted`。新的守护进程从同一
任务数据库读取这些状态。

需要再次计算时使用 `job rerun`。后续执行轮次完成并通过完整验证后，旧轮次可能进入只读清理
计划：

```text
trajecta --format json job prune
```

`0.1.0-alpha.1` 中该计划仅供查看，不会删除结果目录或任务历史。
