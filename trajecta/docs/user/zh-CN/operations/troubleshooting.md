---
title: 按症状与 diagnostic code 排查 Trajecta
description: 根据 human output 或稳定 machine-readable code，排查 Trajecta 运行中的常见问题。
---

# 故障索引

先找到观察到问题的命令，并尽量以 JSON mode 再运行一次：

例如，可以将项目状态读取为结构化输出：

```text
trajecta --format json --project PROJECT project status
```

应用 diagnostic 位于 `diagnostics[]`。每项包含稳定 `code` 和带上下文的 `message`；一次操作
检查多个项目时，还可能包含嵌套 diagnostic。CLI usage error 返回 exit code `2`。命令已经
成功解析，但遇到 product 或 runtime error 时返回 `1`。

`job events --follow` 一类 stream 使用 JSONL：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

比较两个 attempt 时，可以保存完整 JSON 或 JSONL response。Machine mode 将结构化应用响应
写入 stdout，通常不在 stderr 中重复输出。

## 配置与 doctor

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| CLI 选择了意外配置 | 使用相同 global option 执行 `trajecta config path` | `config.not_found`、`config.invalid_path` | 依次检查 `--config`、`TRAJECTA_CONFIG` 和平台默认路径 |
| 设置无法保存 | `config get KEY`，随后以 JSON mode 重复 `config set` | `config.invalid_key`、`config.invalid_value`、`config.invalid_schema`、`config.write_failed` | 修正 selector 或 value；更新被拒绝后，原有效文件保持不变 |
| Doctor 拒绝机器文件 | `config validate` | `doctor.config_invalid`、`config.invalid_schema` | 修正机器配置，再检查项目 |
| Deep doctor 无法写 probe | 项目根与文件系统权限 | `doctor.filesystem_unwritable`、`doctor.cleanup_failed` | 恢复项目根下的 create、sync、rename 和 cleanup 访问 |
| Deep doctor 的 SQLite 周期失败 | 可用空间与文件系统状态 | `doctor.sqlite_create_failed`、`doctor.sqlite_integrity_failed`、`doctor.sqlite_checkpoint_failed` | 修正文件系统或 SQLite runtime，再次运行 deep doctor |
| Deep doctor 无法读取气象资料 | Data root、reader backend 与支持的文件 | `doctor.data_inspect_failed`、`doctor.data_scan_limit` | 确认目录内容和 Profile reader 选择 |

配置路径和资源设置见[配置与 doctor](../getting-started/configuration.md)。

## 项目与资料准备

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| Project 一直为 `draft` | `project status`、`project validate` | `project.document_invalid`、`project.invalid_index` | 补齐 Case 或 Profile 字段，并修正文档类型 |
| Project 一直为 `configured` | `project data-plan`、`project status` | `project.lock_missing`、`project.finalize_pending`、`project.output_root_pending` | 准备计划中的文件，并在显式 finalize 前创建输出根 |
| Finalize 退出且 lock 未变化 | `project finalize` 的完整 JSON response | `project.finalize_preflight_failed` 及嵌套 `data.*` 或 `project.*` code | 修正全部 preflight item，再运行 finalize |
| Profile 选择了错误 Case | `project show`、Profile `case_path` 与 indexed Case | `project.profile_case_mismatch`、`run.profile_case_mismatch` | 将 Profile 指向一个已索引的 resolved Case |
| 项目路径被拒绝 | Project index 中的准确相对值 | `project.path_escape`、`project.output_root_invalid` | 使用项目根以下的规范化路径 |
| Lock content 与本地资料不再匹配 | `doctor --deep`、`data inspect` 与 DatasetLock file list | `project.lock_invalid`、`doctor.lock_invalid`、`data.lock_invalid` | 恢复 locked byte，或使用目标完整资料重新 finalize |
| Case 所需 frame 超出 lock | Resolved Case coverage 与 lock coverage | `data.lock_requirements_mismatch` | 准备所需时空覆盖，再次 finalize |

`project finalize` 会先完成所有 preflight check，之后才替换 lock binding。Preflight 失败时，
已有 lockfile 保持不变。

## 队列与 daemon

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| 机器空闲但任务一直 queued | `job status` resource request 与配置 pool | `scheduler.queued`、`job.invalid_resources`、`daemon.invalid_capacity` | 确认请求能够放入总 CPU 和可调度内存池 |
| 小任务越过较早的大任务 | Queue position 与 dispatch event | `scheduler.dispatched` | Safe backfill 可越过队首三次，之后会为队首保留资源 |
| 看似有资源，队列仍暂停 | 主机总可用内存与事件记录 | `daemon.external_memory_pressure` | 减少其他应用内存压力，或在后续批次前调整 reserve |
| Runtime command 无法连接 | 所选配置、endpoint 路径和已有 daemon process | `daemon.launch_failed`、`daemon.start_timeout`、`daemon.ipc_failed`、`job_backend.unavailable` | 确认配置路径与 local IPC 权限，再重试命令 |
| Daemon endpoint 已被占用 | Process identity 与 start token | `daemon.bind_failed`、`daemon.owner_probe_failed`、`daemon.identity_failed` | 查看是否已有 daemon 使用同一 catalog 和 endpoint |
| Catalog operation 失败 | 可用空间、目录权限和 SQLite sidecar | `job_backend.storage`、`job_backend.conflict` | 暂停新提交，恢复 catalog storage，并用同一配置重新连接 |

[运维手册](index.md)详细介绍 admission、safe backfill、空闲退出与重启协调。

## Worker 与中断 run

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| Worker 一直未进入 `running` | 事件、worker stderr 与输出目录创建 | `worker.launch_failed`、`worker.start_failed`、`worker.lease_timeout` | 修正启动或文件系统条件，再创建 rerun |
| Worker 在运行中消失 | Process list、job event 与 attempt 目录 | `worker.lost`、`run.interrupted.worker_lost` | 等待 terminal reconciliation，检查部分结果，再按需要 rerun |
| Daemon 返回时 worker 仍存活 | Event 与 worker identity | `worker.reattached` | 继续监测同一 attempt |
| Daemon 消失后已有 terminal manifest | Event、manifest run ID 与 catalog run ID | `worker.terminal_reconciled` | Inspect 并完整验证结果 |
| Admission 后输入发生变化 | Resolved input file 与 stored hash | `worker.input_changed` | 恢复已接收的 byte，或从 finalized input 提交新 run |
| 安全取消需要等待 | 当前 macro-step progress | `job.safe_cancel_requested` | 等待 worker 到达下一个 safe boundary；需要立即终止时再使用 force |

完整的重启与 rerun 顺序见[恢复与中断 attempt](recovery.md)。

## 结果、SQLite 与报告

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| Manifest 无法解析 | 保留文件，并以 JSON mode 运行 `result inspect` | `result.manifest_invalid`、`result.encoding` | 定位生成该文件的 attempt，读取 worker closeout event |
| Manifest 可读，但 SQLite 不可用 | 主库、WAL、SHM 与文件系统访问 | `result.inspect_sqlite_unavailable`、`result.sqlite_invalid` | 保持文件位于一起，并将 inspection 视为 partial |
| 终态结果缺少所需文件 | `result inspect` 的 artifact inventory | `result.artifact_missing` | 检查 terminal event，修正 closeout failure 后 rerun |
| Manifest row count 与 SQLite 不同 | Full verification output | `result.manifest_sqlite_count_mismatch` | 保持目录不变，并创建单独 rerun |
| 无法读取某个粒子 | Particle ID range 与所选 run | `result.particle_not_found`、`result.trajectory_particle_id_out_of_range` | 选择结果中存在的 ID，并确认解析的是哪个 attempt |
| 报告无法生成 | 结果目录权限与可用空间 | `report.write_failed`、`report.refresh_failed` | 恢复 write 与 atomic rename 访问，再生成 `run-report.md` |
| 运行期间 WAL 持续增长 | Job state、output event cadence 与可用空间 | 写入失败时出现 `result.io` | 监测活跃 writer；terminal closeout 会 checkpoint WAL |

WAL 与目录迁移的详细流程见[存储、SQLite 与 WAL](storage-sqlite.md)。

## 气象 reader

| 症状 | 检查位置 | 常见 code | 后续动作 |
| --- | --- | --- | --- |
| Reader 无法打开资料文件 | `data inspect`、file family 与所选 reader | `data.inspect_failed`、`data.unknown_format`、`met.runtime` | 确认文件属于支持的 family，且所选 backend 可以读取 |
| Backward run 缺少时间支持 | Resolved time range 与区间两侧 lock frame | `met.missing_symmetric_time_support` | 准备 query interval 周围需要的附加 frame |
| Machine JSON 与 native output 混合 | Command output option | `met.machine_stdout_requires_file` | Machine mode 下将 native probe output 指向文件 |

## 查找 code

[自动生成 diagnostic 索引](../reference/diagnostics.md)列出各组件实现中的全部公开 code。
可以在该页搜索准确 code；调试安装或准备 bug report 时，还可从页面进入对应 source link。

一份便于复现的报告可以包含：

- `trajecta --version` 输出；
- 操作系统与 package architecture；
- 已移除路径和 credential 的命令；
- 完整 machine response；
- 任务已接收时的 job-series ID、run ID 与 attempt number；
- Terminal `job status` 与相关 event interval；
- Result manifest status 与 artifact inventory。
