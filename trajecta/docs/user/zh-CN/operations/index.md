---
title: Trajecta 运维手册
description: 安全管理本地资源池、队列、daemon、worker 和并发 reader。
---

# 运维手册

Trajecta 使用本地 control plane。CLI 向 daemon 提交任务；daemon 在 job database 中
持久化 series、run、attempt、事件、资源和 lease；worker 执行不可变 resolved input 并
写入结果目录。

## 资源池与调度

`resources.cpu_slots`、`resources.memory_pool_mib` 和
`resources.memory_reserve_mib` 定义接收容量。每个 RunProfile 请求 worker thread 和
内存预算。请求能够放入可用容量时，scheduler 才接收任务。

内存 reserve 必须小于 pool。Pool 用于 Trajecta worker，同时应为 daemon、操作系统、
reader 和临时分配保留空间。外部内存压力会暂停新任务派发，不会取消运行中的 worker。

## Daemon 生命周期

CLI 按需启动本地 daemon 或重新连接。Daemon 记录 instance identity 与进程 start token，
避免复用的 PID 冒充旧 lease。Idle shutdown 只释放进程；任务数据库和完成产物继续留在
磁盘上。

重启后，daemon 会协调 catalog state、worker lease、终态 manifest 和已保留 forensic。
完成的 attempt 保持终态，不会重跑。只有身份与 lease 检查通过时，已接收 worker 才能
重新附着。

## Worker 生命周期

Worker 依次经历 queued、running 和 terminal 状态。安全取消等待数值边界，并生成终态
manifest、完成收尾的 SQLite、provenance，以及零字节或不存在的终态 WAL。强制取消会
保留可获得的部分产物与 forensic 证据。

自动化应使用事件流，单次状态查看使用 `job status`。并发结果 reader 应遵守 indexed
high-water 合同并使用读快照；writer 独占生命周期收尾权。
