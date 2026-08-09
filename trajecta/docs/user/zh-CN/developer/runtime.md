---
title: 输出与运行时
description: 持久化任务控制、daemon 与 worker 进程、输出事务、取消、attempt 和恢复机制。
---

# 输出与运行时

运行时在一次 `trajecta-core` 模拟外层提供持久化本地任务控制。它接收已经验证的请求，预留主机
资源，启动独立 worker，记录进度，并在结束时协调结果状态。Worker 使用的数值 runner 与 core
直接测试中的 runner 相同。

这一设计中有两个 SQLite database：

| Database | Owner | 用途 |
| --- | --- | --- |
| 本地 job catalog | `trajecta-job` | 队列状态、资源预留、worker lease、事件、attempt 和验证历史 |
| Attempt 目录中的 `particles.sqlite` | `trajecta-core` | 粒子、质量、事件、状态和终止等科学产品 |

两个 database 有各自的 schema 与生命周期。读取任务状态无需打开科学结果；daemon 停止后，结果
检查仍可独立进行。

## 请求入队

`trajecta-cli` 将 `run` 参数转换为 `SubmitRequest`。输入可以是 finalized project 加具名 Profile，
也可以是显式的 resolved Case 与 RunProfile。请求还包含：

- 从本地资源池预留的 CPU slot；
- 以 MiB 表示的内存预留；
- Worker thread count，其值不能超过 CPU reservation。

Daemon 先根据配置容量验证请求，再写入 catalog。入队会创建两个 UUIDv7 身份：

| 身份 | 生命周期 |
| --- | --- |
| Job series ID | 连接首次提交和后续所有 rerun |
| Run ID | 标识一个不可变 attempt 及其输出目录 |

Attempt number 从 1 开始，并在 series 内递增。Rerun 会把已验证输入和资源请求复制到新的 queued
attempt，不会复用或覆盖早期目录。

## 持久化状态机

调度器可见状态如下：

```text
queued -> starting -> running -> complete
                         |       completed_with_particle_errors
                         |       failed
                         |       interrupted
                         +-> cancelling -> cancelled
```

`starting` 覆盖资源预留和进程启动。`cancelling` 表示 cooperative request 已经记录，worker 正在
等待安全 macro-step boundary。Terminal state 不再转换，恢复过程也不会再次 dispatch。

所有转换都通过 catalog transaction 应用。一次转换涉及 state row、resource reservation、worker
lease、diagnostic 和 persistent event 时，这些内容会在同一个 catalog operation 内更新。非法状态
边会返回 typed backend error。

## Catalog 存储

`LocalJobCatalog` 使用 SQLite，启用 foreign key 与 WAL，busy timeout 为 5 秒。Daemon 管理的队列
变更采用 full synchronous durability。Worker telemetry 使用独立连接和 normal synchronous mode，
因为 heartbeat 与 progress sample 属于 advisory 信息；attempt 状态转换仍由 daemon connection
完成。

Catalog 记录以下内容：

- 每个 attempt 的 current snapshot；
- Resource request 与 output-directory allocation；
- Daemon 和 worker lease；
- Sequence 单调递增的 fan-out event；
- 成功 attempt 的 full-verification identity；
- Rerun 之间的 supersession relationship；
- `job forget` 产生的 visibility change。

读取事件不会消费 row。两个 reader 可以请求相同 sequence range，`job events --follow` 也会从自己
的 cursor 继续。因此 terminal、监控进程和 AI assistant 可以同时观察同一任务，不会争抢消息。

## 调度器与资源池

`plan_dispatch` 是纯调度函数。输入包括 configured capacity、当前 active reservation、持久化 FIFO
queue 与 external-memory-pressure flag，输出 selected run ID，以及需要由 catalog 原子保存的
queue-head bypass counter。

通常按 FIFO 调度。Head 暂时无法放入剩余 CPU 或内存池时，较小的后续请求可以利用空闲容量。
同一个 blocked head 最多被绕过三次。达到上限后，scheduler 会为它保留后续资源，并停止选择
更靠后的任务。

外部内存压力处于 active 状态时，dispatch 会暂停。超过 daemon 总容量的请求在 submit 阶段就会
被拒绝，因此不可能执行的 job 不会长期占据 queue head。

Scheduler 管理声明的 reservation。任务入队后，操作系统中的其他进程仍可能继续占用内存。这类
情况由 worker termination 与 recovery 处理；资源模型不会试图控制主机上的其他程序。

## Daemon ownership 与本地 IPC

同一时刻只有一个 daemon 拥有 catalog。Lease 包含 instance ID、PID、process-start token 和
heartbeat。Start token 用于区分原进程与之后复用同一个数字 PID 的其他进程。

CLI client 通过 local IPC 与同一 release 的 daemon 通信。Request 和 response 由
`trajecta-job::ipc` 定义类型，单个 frame 上限为 4 MiB。Job event 保留在 catalog 中，因此 IPC
负责发起 query，不承担临时 event queue。

Daemon dispatch loop 每轮都不会长期阻塞：

1. 读取 queued attempt 与 active reservation。
2. 计算一份 dispatch plan。
3. 将选中 row 转换为 `starting`，同时保存 bypass update。
4. 以隐藏的 `__worker` mode 启动当前 executable。
5. 探测 process identity 并附加 worker lease。
6. 启动失败或 lease attach 失败时，写入受控终态及 diagnostic。

Windows 启动 worker 时不会显示额外 console window，Ubuntu 使用独立 child process。前台与后台
CLI mode 的差别在于 client 是否等待；两者都提交到相同 daemon，attempt 结构也一致。

## Worker 执行

Worker 接收一个 run ID，并重新打开 catalog。开始科学计算前，它会确认 lease 与 start token 和
attempt 匹配。随后按以下顺序工作：

1. 重新加载并验证 resolved project input。
2. 为已经分配的 attempt 构造 production `SimulationRunner`。
3. 将 catalog state 转换为 `running`。
4. 通过 `trajecta-core` 执行 macro step。
5. 按有限采样间隔报告 heartbeat 和 progress。
6. 关闭 output artifact，并将 manifest terminal state 返回 catalog。

Progress 包括 completed macro-step count、simulation time、active particle，以及 normal 与 abnormal
termination 总数。Resource observation 已经为 wall time、CPU time 和 RSS 定义持久化结构；未采样
的字段保持 absent，不会进行估算。

Worker adapter 中没有另写科学算法。它负责解析文档、提供 runner control，并将 `RunOutcome`
映射到 catalog。

## 科学输出事务

Attempt 目录会包含 running manifest、`particles.sqlite`、临时 provenance 文件、worker log，以及
需要保留的 forensic material。SQLite sink 采用[结果与 SQLite 参考](../reference/results-sqlite.md)
中的版本化 schema。

运行期间遵循以下规则：

- Foreign key 始终启用；
- Output event 定义物理 sample time；
- Particle 先于对应 mass 和 state 写入；
- Lifecycle event 使用有界 transaction；
- 普通 scheduled output 会提交 transaction；
- SQLite 保持 WAL mode，使 indexed reader 能读取 high-water snapshot，同时不阻塞 writer。

正常 terminal completion 的顺序为：

1. 提交待写 SQLite row，并更新 terminal run row。
2. 执行 `PRAGMA wal_checkpoint(TRUNCATE)`，要求终态 WAL 为空。
3. 检查 schema pragma 和 `PRAGMA integrity_check`。
4. 统计全部公开 table，并计算 canonical SQL digest。
5. 关闭 writer connection，计算独立 SQLite database 的 hash。
6. 从排序后的临时 run 流式写 provenance bundle，同时与 SQLite sample cursor 对照。
7. 验证 temporary bundle，原子 rename，再计算 content 与 canonical-output digest。
8. 将 row count、termination summary、mass ledger、SQLite identity 和 provenance identity 加入
   terminal manifest。
9. 通过同目录 temporary file、`sync_all` 和 atomic rename 写入 manifest。

Terminal manifest 是最后完成的绑定记录。状态为 `complete` 时，它引用的 artifact 已经关闭，并且
拥有确定身份。

Bundle 或 SQLite finalize 失败时，runner 会 abort 或 quarantine 受影响输出，并在能够安全完成时
写 failed manifest。Temporary file 与 forensic file 的用途不同：成功结束会删除 ephemeral sort
run，中断资料则保留用于诊断。

## 取消

Safe cancel 与 force cancel 具有不同的 artifact 语义。

### Safe cancel

Daemon 把 safe-cancel request 写入 worker lease。`CatalogRunnerControl` 在下一次采样的 macro-step
boundary 向 runner 返回 `Cancel`。Runner 合法关闭部分输出，写入 `cancelled` manifest，catalog
再接收该终态。

!!! tip "Safe cancel 不生成续跑 checkpoint"

    `job rerun` 会从 resolved input 创建新 attempt。

### Force cancel

Daemon 会同时验证 PID 和 process-start token，然后停止 worker。之后检查 attempt：

- Worker 如果刚好在停止竞态期间完成 terminal manifest，catalog 会接收已经验证的终态；
- 其余情况将 attempt 标为 `interrupted`，并保留 forensic file。

Queued job 可以在 worker 启动前取消。对 terminal attempt 重复取消不会重新打开该 attempt。

## 重启与异常恢复

Daemon 重启后，recovery 会比较 durable lease、平台进程身份和 attempt manifest。每个 active
attempt 会得到一种 disposition：

| 观测 | 恢复动作 |
| --- | --- |
| Worker PID 与 start token 仍然存活 | 将 lease 重新附加到新 daemon instance |
| 已经存在有效 terminal manifest | 将该状态协调到 catalog |
| 没有匹配的 live worker，也没有有效 terminal result | 将 attempt 标为 interrupted，并保留目录 |

Process-start token 可以避免把复用 PID 的无关进程当成 Trajecta worker。Attempt 转入 terminal
state 后，recovery 还会释放 CPU 与 memory reservation，使队列中的任务可以继续。

Completed attempt 保持不可变。Daemon restart、client restart 或新 queue submission 都不会使它
重跑。只有显式 `job rerun` 会创建另一个 attempt。

## Attempt history 与清理计划

History 按 attempt number 升序保留全部 attempt。Complete attempt 通过 full verify 后，可以记录
canonical output identity。同一个 series 中出现较新的 fully verified attempt 时，history 可以把
早期 attempt 标为 superseded，同时保留原文件。

`job forget` 只改变 catalog visibility，不会删除 attempt directory。当前 prune operation 返回
deterministic dry-run plan，列出以后可以清理的路径。`0.1.0-alpha.1` 没有删除这些路径的生产命令。

## Runtime 修改检查表

| 范围 | 需要增加或重跑的检查 |
| --- | --- |
| State transition | 合法/非法 edge、event order、resource release |
| Catalog schema | Fresh create、reopen、WAL、concurrent reader、schema version rejection |
| Scheduler | FIFO、三次 bypass cap、capacity overflow、external pressure |
| IPC message | Encode/decode round trip、frame bound、同版本 client/server |
| Worker launch | PID token probe、launch failure、lease-attachment race、retained log |
| Safe cancellation | Macro-step polling、cancelled manifest、final event、partial result verification |
| Force cancellation | PID reuse protection、terminal race reconciliation、interrupted forensic |
| Output sink | Transaction rollback、table count、reader snapshot、terminal checkpoint |
| Manifest 或 provenance | Atomic replacement、identity mismatch、failure quarantine、full verify |
| Rerun/history | 新 run ID、递增 attempt、保留 completed attempt、dry-run prune |

Runtime integration test 应启动 production daemon 与 worker binary。Mock process manager 适合构造
罕见的启动和恢复竞态，end-to-end contract 则确认 adapter 最终调用真实 runner。
