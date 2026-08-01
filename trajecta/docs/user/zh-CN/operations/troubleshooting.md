---
title: 按症状与 diagnostic code 排查故障
description: 根据观测症状或稳定 diagnostic code 查找 Trajecta 恢复动作。
---

# 故障索引

报告故障时，请保留完整 machine envelope。Machine mode 的 stdout 包含一个 JSON
envelope 或 JSONL stream；结构化应用 diagnostic 不写入 stderr。

## 从症状开始

| 症状 | 首要检查 | 可能的 code family |
|---|---|---|
| 项目一直处于 configured | `project status`、`project data-plan` | `project.lock_missing`、`project.finalize_pending` |
| Finalize 未修改 lock | 检查 preflight diagnostic 与真实数据根 | `project.finalize_preflight_failed`、`data.*` |
| 任务长期 queued | `job events --follow`、资源配置 | `daemon.external_memory_pressure`、`scheduler.*` |
| Worker 消失 | Daemon reconciliation 与保留的运行目录 | `worker.lost`、`run.interrupted.worker_lost` |
| Result 为 partial | `result inspect`、manifest 与 SQLite 状态 | `result.inspect_sqlite_unavailable`、`result.artifact_missing` |
| Full verify 失败 | 保留结果，对照 manifest、SQLite 与 bundle | `result.*`、`doctor.sqlite_*` |
| 磁盘用量持续增长 | 输出 schedule、WAL、空闲空间、活跃 writer | `project.output_root_*`、`result.io` |
| 气象查询失败 | Lock coverage 与 reader capability | `data.lock_*`、`met.*` |

## 从 diagnostic code 开始

| Code | 含义 | 处理动作 |
|---|---|---|
| `project.path_escape` | 项目相对路径越出 jail | 改为项目根以下的规范化路径 |
| `project.lock_missing` | 所选资料集缺少完成 finalize 的 lock | 准备资料并审阅 plan，随后显式 finalize |
| `project.finalize_preflight_failed` | 一个或多个 finalize 检查失败 | 阅读嵌套 diagnostic；此时 lock 未变化 |
| `daemon.external_memory_pressure` | 主机内存压力暂停派发 | 降低压力或后续并发，不终止健康 worker |
| `run.interrupted.worker_lost` | Worker 消失且没有有效完成状态 | 保留 forensic，等待协调，再创建新 attempt |
| `result.manifest_invalid` | Manifest 形状或内容无效 | 保持字节不变，检查生成该文件的 attempt |
| `result.artifact_missing` | 终态结果缺少必需产物 | 将验证视为失败并保留目录 |
| `result.inspect_sqlite_unavailable` | Manifest 可读，SQLite 检查失败 | 使用 partial inspect，并保留 SQLite sidecar |
| `result.manifest_sqlite_count_mismatch` | Manifest 行数与 SQLite 不一致 | 停止分析并保留两侧身份 |
| `doctor.sqlite_integrity_failed` | Deep doctor 的临时 SQLite 检查失败 | 检查文件系统健康、权限和 SQLite runtime |

[完整自动生成 diagnostic 索引](../reference/diagnostics.md)列出 production source 或可执行
回归测试中出现的全部公开 code。
