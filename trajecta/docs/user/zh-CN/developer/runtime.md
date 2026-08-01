---
title: 输出与运行时
description: Worker 执行、持久化任务控制、SQLite 输出、manifest、provenance、取消和恢复内部结构。
---

# 输出与运行时

## 任务生命周期

CLI 将请求准入本地持久化 catalog。调度器检查 CPU 与内存池，分配 attempt，然后启动
独立 worker 进程。任务由 daemon 持有，因此客户端断开后，已准入工作仍可继续。

Catalog 记录状态转换和单调递增的事件序列。重跑会在同一 job series 下建立新 attempt。
恢复流程协调 worker lease、终态 manifest 和中断进程，不会重新运行已经完成的 attempt。

## 输出提交

Core 使用 WAL 模式、外键和有界事务写入 `particles.sqlite`。数据库记录运行、粒子、质量、
物理输出事件、状态和终止。达到终态时，系统在关闭 manifest 前执行完整性检查与 WAL
checkpoint。

Run manifest 绑定软件与解析后输入，同时记录执行设置、数值设置和行数。它还保存终止
分类、质量账本、SQLite 身份与 provenance 身份。Provenance bundle 将采样字段映射回
源记录与变换。

## 取消与中断

安全取消请求 worker 在安全点关闭并写入分类终态。无法完成协作关闭时，强制取消会保留
forensic 状态。进程丢失或主机关机会进入显式失败或中断路径，内存压力与磁盘错误亦然。
清理代码必须保留 `result verify` 与运维诊断需要的证据。

Daemon 和 worker 路径不得绕过 `trajecta-core` 独立执行科学计算。
