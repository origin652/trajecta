---
title: 本机配置参考
description: 查阅 Trajecta 本机配置的路径选择、初始值、资源字段、监测设置、运行模板和更新规则。
---

# 本机配置

本机配置描述一套本地 Trajecta 运行环境。它选择默认读取器、调度容量、状态采样间隔、守护进程
空闲行为和可复用运行模板。科学时段、区域、粒子群和输出保存在项目案例中；资料路径与单次运行
资源请求保存在运行配置中。

文件使用 TOML，格式版本为：

```toml
schema_version = "trajecta.config/v1"
```

未知字段和重复 TOML 键会被拒绝。

## 路径选择

每次命令按以下顺序选择一份配置：

1. 全局参数 `--config PATH`；
2. 环境变量 `TRAJECTA_CONFIG`；
3. 平台默认路径。

| 平台 | 默认路径 |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| 已设置 `XDG_CONFIG_HOME` 的 Ubuntu 24.04 | `$XDG_CONFIG_HOME/trajecta/config.toml` |
| 未设置 `XDG_CONFIG_HOME` 的 Ubuntu 24.04 | `$HOME/.config/trajecta/config.toml` |

显示实际路径：

```text
trajecta config path
trajecta --config workstation.toml config path
```

Trajecta 不合并多份本机配置。使用某份配置提交的 `run` 属于该配置选择的守护进程端点和任务
数据库。后续 `job` 与依赖任务数据库的 `result` 命令应继续选择同一文件。

## 初始值

`config init` 创建新文件，目标已经存在时保持原状：

```text
trajecta --config workstation.toml config init
```

初始值按以下方式计算：

| 字段 | 初始值 |
| --- | --- |
| `default_reader_backend` | `rust` |
| `daemon.idle_shutdown_seconds` | `300` |
| `daemon.local_ipc_only` | `true` |
| `resources.cpu_slots` | 主机报告的逻辑并行度，至少为 1 |
| `resources.memory_pool_mib` | 检测到的物理内存 MiB；无法检测时为 4,096 MiB |
| `resources.memory_reserve_mib` | 内存池的四分之一，并保持小于内存池 |
| `monitoring.sample_interval_ms` | `1000` |
| `monitoring.maximum_median_overhead_percent` | `1.0`% |
| `profile_templates` | 空映射 |

这些数值反映检测到的主机容量。共享工作站可以减少 CPU 槽位，或在提交大型批次前提高内存预留。

## 顶层字段

| 选择器 | 类型 | 约束 | 运行含义 |
| --- | --- | --- | --- |
| `schema_version` | 字符串 | 必须为 `trajecta.config/v1` | 选择配置约定 |
| `default_reader_backend` | 枚举 | `rust` 或 `native` | 资料绑定和运行配置未指定时采用的读取器 |
| `daemon` | 表 | 必需 | 本地守护进程生命周期 |
| `resources` | 表 | 必需 | 共享调度容量 |
| `monitoring` | 表 | 必需 | 进度与资源采样策略 |
| `profile_templates` | 表 | 必需，可以为空 | 准备运行配置时按名称选用的执行模板 |

## 守护进程字段

| 选择器 | 类型与范围 | 含义 |
| --- | --- | --- |
| `daemon.idle_shutdown_seconds` | 大于或等于 `0` 的整数 | 空队列守护进程退出前等待秒数；`0` 表示持续可用 |
| `daemon.local_ipc_only` | 固定为 `true` | Windows 使用具名管道，Linux 使用 Unix 域套接字，不创建网络监听 |

空闲退出会关闭进程和端点，同时保留 SQLite 任务数据库、事件与结果目录。后续运行时命令会重新
启动守护进程，并协调同一任务数据库。

## 资源字段

| 选择器 | 类型与范围 | 含义 |
| --- | --- | --- |
| `resources.cpu_slots` | 至少为 `1` 的整数 | 所有活跃执行轮次共享的工作线程槽位总数 |
| `resources.memory_pool_mib` | 至少为 `1` 的整数 | 调度器准入使用的内存池 |
| `resources.memory_reserve_mib` | 大于或等于 `0`，并小于内存池 | 不参与工作进程准入的内存，也是外部内存压力阈值 |

可调度内存为：

```text
memory_pool_mib - memory_reserve_mib
```

一个运行配置申请 `execution.worker_threads` 个 CPU 槽位，并将
`execution.memory_budget_bytes` 向上取整到 MiB。多项执行轮次的 CPU 和内存总和都能放入资源池
时，可以同时运行。其他程序使用主机内存后，可用内存低于预留量会暂停新任务派发。

## 监测字段

| 选择器 | 类型与范围 | 含义 |
| --- | --- | --- |
| `monitoring.sample_interval_ms` | 至少为 `100` 的整数 | 运行进度与资源采样的目标间隔 |
| `monitoring.maximum_median_overhead_percent` | `0.0` 至 `1.0` 的有限数 | 监测约定记录的中位开销上限 |

资源和进度采样会成为持久任务事件，供状态显示和运行报告使用。轨迹样本仍保存在结果数据库中。

## 运行模板

每个非空模板名称包含一个 `execution` 表：

```toml
[profile_templates.local.execution]
worker_threads = 4
memory_budget_bytes = 2147483648
executor = "rayon"
meteorology_reader = "rust"
```

| 字段 | 约束 |
| --- | --- |
| `worker_threads` | 正整数，不大于 `resources.cpu_slots` |
| `memory_budget_bytes` | 正整数，以字节为单位 |
| `executor` | 非空稳定执行器名称 |
| `meteorology_reader` | `rust` 或 `native` |

模板用于本机复用执行设置。项目索引可以记录模板名称及其 SHA-256，把已经准备的运行配置连接到
所选本机模板。运行时，工作进程仍接收完整解析后的运行配置。

用点分选择器创建或移除模板：

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

`config list` 按确定顺序返回所有叶子选择器。`config get` 要求选择器已经存在。`config set`
可直接接收普通字符串；对象或数组则使用 JSON。`config unset` 只处理可选模板选择器，必需字段
会继续保留。

每次更新按以下顺序执行：

1. 读取并解析当前文件。
2. 在内存中替换或移除选定值。
3. 反序列化并校验完整配置。
4. 写入同目录临时文件。
5. 刷新并同步文件内容。
6. 原子替换所选配置。

值被拒绝时，原文件字节保持不变。一组修改完成后运行 `config validate`，提交长队列前再运行
项目 `doctor --deep`。

机器可读结构与示例见[结构定义参考](schemas.md)。
