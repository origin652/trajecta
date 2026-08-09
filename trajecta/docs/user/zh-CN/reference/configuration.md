---
title: 机器配置参考
description: 查找 Trajecta 机器配置的路径选择、初始值、资源字段、monitoring value、Profile template 与更新规则。
---

# 机器配置

机器配置描述一个本地 Trajecta runtime。它选择 reader default、scheduler capacity、monitoring
cadence、daemon idle behavior 和 reusable execution template。科学 time、domain、population 与
output choice 位于项目 Case。Dataset path 和每次运行的 resource request 位于 RunProfile。

文件使用 TOML，schema identity 为：

```toml
schema_version = "trajecta.config/v1"
```

Unknown field 与 duplicate TOML key 会被拒绝。

## 路径选择

每次 invocation 按以下顺序选择一份文件：

1. Global `--config PATH`。
2. `TRAJECTA_CONFIG` environment variable。
3. Platform default。

| Platform | 默认路径 |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| 设置了 `XDG_CONFIG_HOME` 的 Ubuntu 24.04 | `$XDG_CONFIG_HOME/trajecta/config.toml` |
| 未设置 `XDG_CONFIG_HOME` 的 Ubuntu 24.04 | `$HOME/.config/trajecta/config.toml` |

显示 resolved location：

```text
trajecta config path
trajecta --config workstation.toml config path
```

Trajecta 不合并多份配置。使用某份 configuration 提交的 `run` 属于其中的 daemon endpoint
与 job catalog。后续 `job` 和 catalog-backed `result` command 可继续选择同一文件。

## 初始值

`config init` 创建新文件，并保留已经存在的文件：

```text
trajecta --config workstation.toml config init
```

初始值按以下方式计算：

| 字段 | 初始值 |
| --- | --- |
| `default_reader_backend` | `rust` |
| `daemon.idle_shutdown_seconds` | `300` |
| `daemon.local_ipc_only` | `true` |
| `resources.cpu_slots` | Host 报告的 logical parallelism，最小为 1 |
| `resources.memory_pool_mib` | 检测到的 physical memory MiB；无法检测时使用 4,096 MiB |
| `resources.memory_reserve_mib` | Pool 的四分之一，并保持小于 pool |
| `monitoring.sample_interval_ms` | `1000` |
| `monitoring.maximum_median_overhead_percent` | `1.0` percent |
| `profile_templates` | Empty mapping |

这些数值描述检测到的 capacity。共享工作站可以在提交大型批次前减少 CPU slot，或提高
memory reserve。

## Top-level field

| Selector | Type | Constraint | Runtime 含义 |
| --- | --- | --- | --- |
| `schema_version` | String | 准确等于 `trajecta.config/v1` | 选择 configuration contract |
| `default_reader_backend` | Enum | `rust` 或 `native` | Binding 与 Profile 都未选择时使用的 reader |
| `daemon` | Table | Required | Local daemon lifecycle setting |
| `resources` | Table | Required | Shared scheduler capacity |
| `monitoring` | Table | Required | Resource-observation policy |
| `profile_templates` | Table | Required，可以为空 | Profile 准备期间使用的 named execution template |

## Daemon field

| Selector | Type 与范围 | 含义 |
| --- | --- | --- |
| `daemon.idle_shutdown_seconds` | Integer，不小于 `0` | Empty local daemon 退出前等待的秒数；`0` 表示保持可用 |
| `daemon.local_ipc_only` | 固定 `true` | Windows 使用 named pipe，Linux 使用 Unix socket；不配置 network listener |

Idle exit 会关闭 process 与 endpoint，同时保留 SQLite job catalog、event 和 result directory。
后续 runtime command 会启动 daemon，并协调同一 catalog。

## Resource field

| Selector | Type 与范围 | 含义 |
| --- | --- | --- |
| `resources.cpu_slots` | Integer，至少 `1` | Active attempt 共享的 worker-thread slot 总数 |
| `resources.memory_pool_mib` | Integer，至少 `1` | Scheduler admission 使用的 memory pool |
| `resources.memory_reserve_mib` | Integer，至少 `0`，并小于 pool | 不参与 worker admission 的部分，也是 external-pressure threshold |

可调度内存为：

```text
memory_pool_mib - memory_reserve_mib
```

一个 RunProfile 请求 `execution.worker_threads` 个 CPU slot，并将
`execution.memory_budget_bytes` 向上取整到 MiB。多个 attempt 可以在两项总和都放入 pool 时
共同运行。其他程序占用主机内存后，可用内存低于 reserve 会暂停新任务派发。

## Monitoring field

| Selector | Type 与范围 | 含义 |
| --- | --- | --- |
| `monitoring.sample_interval_ms` | Integer，至少 `100` | Runtime progress 与 resource observation 的 nominal interval |
| `monitoring.maximum_median_overhead_percent` | `0.0` 至 `1.0` 的 finite number | Monitoring contract 记录的 median overhead ceiling |

Resource 与 progress observation 会成为 durable job event，可用于 status display 和 run report。
Trajectory sample 则保存在 result database 中。

## Profile template

每个非空 template name 包含一个 `execution` table：

```toml
[profile_templates.local.execution]
worker_threads = 4
memory_budget_bytes = 2147483648
executor = "rayon"
meteorology_reader = "rust"
```

| Field | Constraint |
| --- | --- |
| `worker_threads` | Positive，并且不大于 `resources.cpu_slots` |
| `memory_budget_bytes` | Positive integer byte count |
| `executor` | Non-empty stable executor name |
| `meteorology_reader` | `rust` 或 `native` |

Template 是本机使用的便利入口。Project index 可以记录 template name 及其 SHA-256，从而将
prepared Profile 与准确 local template 联系起来。Runtime execution 仍会接收 fully resolved
RunProfile。

通过 dotted selector 创建或移除 template：

=== "Windows PowerShell"

    ```powershell
    $value = '{"execution":{"worker_threads":2,"memory_budget_bytes":1073741824,"executor":"rayon","meteorology_reader":"rust"}}'
    .\trajecta.exe --config workstation.toml config set profile_templates.two_workers $value
    .\trajecta.exe --config workstation.toml config unset profile_templates.two_workers
    ```

=== "Ubuntu 24.04"

    ```bash
    ./trajecta --config workstation.toml config set profile_templates.two_workers \
      '{"execution":{"worker_threads":2,"memory_budget_bytes":1073741824,"executor":"rayon","meteorology_reader":"rust"}}'
    ./trajecta --config workstation.toml config unset profile_templates.two_workers
    ```

## 读取与更新

```text
trajecta --config workstation.toml config list
trajecta --config workstation.toml config get resources.memory_pool_mib
trajecta --config workstation.toml config set resources.memory_reserve_mib 2048
trajecta --config workstation.toml config validate
```

`config list` 以确定性顺序返回全部 leaf selector。`config get` 接受已经存在的 selector。
`config set` 对 scalar text 接受 plain string，对 object 或 array value 接受 JSON syntax。
`config unset` 只处理 optional template selector，required field 会继续保留。

每次更新采用相同顺序：

1. 读取并解析 current file。
2. 在内存中替换或移除 selected value。
3. Deserialize 并校验 complete configuration。
4. 写入同目录 temporary file。
5. Flush 并同步文件。
6. 原子替换 selected configuration。

Value 被拒绝后，previous file byte 保持原状。一组更改完成后，可以运行 `config validate`；
提交长队列前再运行 project `doctor --deep`。

Machine-readable shape 与 example 位于 [schema 参考](schemas.md)。
