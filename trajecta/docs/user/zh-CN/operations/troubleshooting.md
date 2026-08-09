---
title: 按症状与诊断码排查
description: 根据人类可读输出或稳定诊断码，排查 Trajecta 配置、项目、队列、工作进程、存储和结果问题。
---

# 故障排查索引

先找到观察到问题的命令，并在适用时用 JSON 模式重新运行。例如，以结构化方式读取项目状态：

```text
trajecta --format json --project PROJECT project status
```

应用诊断位于 `diagnostics[]`。每项都有稳定 `code` 和与当前操作相关的 `message`；一次预检包含
多个失败项时，还可能有嵌套诊断。CLI 用法错误返回退出码 `2`，已经成功解析的命令遇到产品或
运行错误时返回 `1`。

`job events --follow` 等流式命令使用 JSONL：

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

需要比较两个执行轮次时，可以分别保存完整 JSON 或 JSONL。机器模式把结构化应用响应写入标准
输出，标准错误通常为空。

## 本机配置与环境检查

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| CLI 选择了意外的配置 | 用相同全局参数运行 `trajecta config path` | `config.not_found`、`config.invalid_path` | 依次核对 `--config`、`TRAJECTA_CONFIG` 和平台默认路径 |
| 配置值无法保存 | 先 `config get KEY`，再用 JSON 模式重复 `config set` | `config.invalid_key`、`config.invalid_value`、`config.invalid_schema`、`config.write_failed` | 修正选择器或值；被拒绝的更新不会替换原配置 |
| 环境检查拒绝本机配置 | `config validate` | `doctor.config_invalid`、`config.invalid_schema` | 先修正本机配置，再检查项目 |
| 深度检查无法写入临时文件 | 项目根目录和文件系统权限 | `doctor.filesystem_unwritable`、`doctor.cleanup_failed` | 恢复创建、同步、重命名和清理权限 |
| SQLite 临时流程失败 | 可用空间和文件系统状态 | `doctor.sqlite_create_failed`、`doctor.sqlite_integrity_failed`、`doctor.sqlite_checkpoint_failed` | 修复文件系统或 SQLite 运行环境，再运行深度检查 |
| 无法检查气象资料 | 资料目录、读取器和文件系列 | `doctor.data_inspect_failed`、`doctor.data_scan_limit` | 核对目录内容与运行配置中的读取器 |

配置路径与资源设置见[本机配置与环境检查](../getting-started/configuration.md)。

## 项目与资料准备

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| 项目一直为 `draft` | `project status`、`project validate` | `project.document_invalid`、`project.invalid_index` | 补齐案例或运行配置必填项，并修正文档类型 |
| 项目一直为 `configured` | `project data-plan`、`project status` | `project.lock_missing`、`project.finalize_pending`、`project.output_root_pending` | 准备计划中的文件与结果目录，再执行项目定稿 |
| 项目定稿失败且资料锁未更新 | `project finalize` 的完整 JSON 响应 | `project.finalize_preflight_failed` 及嵌套 `data.*` 或 `project.*` | 处理全部预检项后重新定稿 |
| 运行配置选错案例 | `project show`、`case_path` 和项目案例表 | `project.profile_case_mismatch`、`run.profile_case_mismatch` | 让 `case_path` 指向项目中登记的案例 |
| 项目路径被拒绝 | 项目索引中的具体相对路径 | `project.path_escape`、`project.output_root_invalid` | 使用项目根目录内的规范相对路径 |
| 资料锁与本地文件不再一致 | `doctor --deep`、`data inspect` 和资料锁文件表 | `project.lock_invalid`、`doctor.lock_invalid`、`data.lock_invalid` | 恢复锁定文件，或对完整资料重新定稿 |
| 案例所需时次超出资料锁 | 解析后案例时段和资料锁覆盖 | `data.lock_requirements_mismatch` | 补齐时间与空间覆盖，再次定稿 |

`project finalize` 会先完成全部预检，再替换资料锁绑定。预检失败时，现有资料锁保持原状。

## 队列与守护进程

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| 主机看似空闲，任务仍在排队 | `job status` 中的资源请求与本机资源池 | `scheduler.queued`、`job.invalid_resources`、`daemon.invalid_capacity` | 确认单任务能放入总 CPU 和可调度内存 |
| 小任务先于较早的大任务运行 | 队列位置和派发事件 | `scheduler.dispatched` | 空闲资源回填最多越过队首三次，之后会为队首保留资源 |
| 资源看似空闲，队列仍暂停 | 主机总可用内存和事件日志 | `daemon.external_memory_pressure` | 降低其他程序内存占用；下批任务前调整预留 |
| 运行时命令无法连接 | 本机配置、端点路径和现有守护进程 | `daemon.launch_failed`、`daemon.start_timeout`、`daemon.ipc_failed`、`job_backend.unavailable` | 核对配置路径和本地 IPC 权限后重试 |
| 守护进程端点已被占用 | 进程 ID 与启动标记 | `daemon.bind_failed`、`daemon.owner_probe_failed`、`daemon.identity_failed` | 检查是否已有守护进程使用同一任务数据库和端点 |
| 任务数据库操作失败 | 空间、目录权限和 SQLite 伴随文件 | `job_backend.storage`、`job_backend.conflict` | 暂停新提交，修复任务数据库存储，再用同一配置连接 |

[运维手册](index.md)详细说明资源准入、空闲资源回填、空闲退出和重启协调。

## 工作进程与中断运行

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| 工作进程一直未进入 `running` | 任务事件、工作进程错误输出和结果目录创建 | `worker.launch_failed`、`worker.start_failed`、`worker.lease_timeout` | 修复启动或文件系统问题后重跑 |
| 工作进程运行中消失 | 进程列表、任务事件和执行轮次目录 | `worker.lost`、`run.interrupted.worker_lost` | 等待终态协调，检查部分结果，再按需重跑 |
| 守护进程恢复时工作进程仍存活 | 事件和工作进程标识 | `worker.reattached` | 继续监测同一执行轮次 |
| 守护进程失联后已有终态清单 | 清单运行 ID 与任务数据库运行 ID | `worker.terminal_reconciled` | 检查并完整验证结果 |
| 任务准入后输入发生变化 | 解析后输入和已存散列 | `worker.input_changed` | 恢复准入时的文件，或在项目重新定稿后提交新任务 |
| 安全取消需要等待 | 当前数值宏步进度 | `job.safe_cancel_requested` | 等待下一个安全边界；需要立即停止时再使用强制取消 |

完整重启和重跑顺序见[恢复中断执行轮次](recovery.md)。

## 结果、SQLite 与报告

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| 运行清单无法解析 | 保留文件，用 JSON 模式运行 `result inspect` | `result.manifest_invalid`、`result.encoding` | 找到产生该目录的执行轮次，读取工作进程收尾事件 |
| 运行清单可读，SQLite 不可用 | 主库、WAL、SHM 和文件权限 | `result.inspect_sqlite_unavailable`、`result.sqlite_invalid` | 保持文件成组，按部分结果处理 |
| 终态结果缺少必需文件 | `result inspect` 的产物列表 | `result.artifact_missing` | 查看终态事件，修复收尾问题后创建重跑 |
| 运行清单行数与 SQLite 不同 | 完整验证输出 | `result.manifest_sqlite_count_mismatch` | 保留目录原状，创建独立重跑 |
| 无法读取某个粒子 | 粒子 ID 范围和解析到的执行轮次 | `result.particle_not_found`、`result.trajectory_particle_id_out_of_range` | 选择结果中存在的 ID，并确认当前解析到哪一轮 |
| 运行报告写入失败 | 结果目录权限和可用空间 | `report.write_failed`、`report.refresh_failed` | 恢复写入和原子重命名权限，再生成报告 |
| 运行中 WAL 持续增长 | 任务状态、输出间隔和可用空间 | 写入失败时为 `result.io` | 监测活跃写入端；终态收尾会执行 WAL 检查点 |

WAL 和目录移动方式见[存储、SQLite 与 WAL](storage-sqlite.md)。

## 气象读取

| 症状 | 先检查 | 常见诊断码 | 处理方法 |
| --- | --- | --- | --- |
| 读取器无法打开文件 | `data inspect`、文件系列和所选读取器 | `data.inspect_failed`、`data.unknown_format`、`met.runtime` | 确认文件属于受支持系列，并可由所选读取器打开 |
| 反向运行缺少时间支持 | 解析后时段和资料锁两侧时次 | `met.missing_symmetric_time_support` | 准备查询区间周围所需的相邻帧 |
| 机器 JSON 混入原生输出 | 命令输出选项 | `met.machine_stdout_requires_file` | 使用机器模式时，把原生探测输出写入文件 |

## 查找诊断码

[自动生成的诊断码索引](../reference/diagnostics.md)按功能列出公开诊断码，范围包括 CLI、守护进程、
工作进程和调度器，也包括项目、资料与结果处理。用完整诊断码搜索该页，再根据页面中的源码
位置定位问题。

提交问题时，以下信息通常最有用：

- `trajecta --version` 输出；
- 操作系统和发行包架构；
- 移除凭据和私人路径后的命令；
- 完整机器响应；
- 已接受任务的任务系列 ID、运行 ID 和执行轮次；
- 终态 `job status` 和相关事件区间；
- 运行清单状态与产物列表。
