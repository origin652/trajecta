---
title: 配置本机资源并检查运行环境
description: 创建 Trajecta 本机配置，设置本地资源池，并使用 doctor 命令检查项目运行环境。
---

# 配置本机资源并检查运行环境

每条 Trajecta 命令都会选择一份本机配置。该文件记录本地调度器可使用的资源、默认气象
读取器、监测间隔和守护进程空闲时间。科学设置保存在项目的案例（`Case`）中；资料路径
与单次运行的资源申请则保存在运行配置（`RunProfile`）中。

这种分工使同一个项目可以在不同机器上采用不同的资源设置。例如，一台工作站使用四个
线程，另一台服务器使用更多线程；模拟时段、粒子群和数值设置仍由同一份案例控制。

## 配置控制的内容

| 范围 | 主要字段 | 作用 |
|---|---|---|
| 默认读取器 | `default_reader_backend` | 运行配置未指定读取器时采用此值 |
| 本地守护进程 | `daemon.idle_shutdown_seconds` | 本地守护进程空闲后保留的时间 |
| CPU 资源池 | `resources.cpu_slots` | 所有活跃任务合计可占用的 CPU 槽位 |
| 内存池 | `resources.memory_pool_mib` 与 `resources.memory_reserve_mib` | 纳入调度的总内存，以及始终留在工作进程分配之外的预留量 |
| 运行监测 | `monitoring.sample_interval_ms` | 资源状态的采样间隔 |
| 模板 | `profile_templates.<name>` | 准备项目运行配置时可复用的执行值 |

`config init` 会读取逻辑 CPU 数量和物理内存。初始内存预留量取物理内存的四分之一，
默认读取器为纯 Rust 实现。随后可通过 `config get` 和 `config set`，按本机用途逐项调整。

## 选择配置文件

Trajecta 按以下顺序解析路径：

1. 全局参数 `--config PATH`；
2. 环境变量 `TRAJECTA_CONFIG`；
3. 平台默认路径。

Windows 默认使用 `%APPDATA%\Trajecta\config.toml`。Ubuntu 默认使用
`$XDG_CONFIG_HOME/trajecta/config.toml`；未设置 `XDG_CONFIG_HOME` 时，路径为
`$HOME/.config/trajecta/config.toml`。

显示当前选择结果：

```text
trajecta config path
```

教程或隔离项目可以显式指定文件：

```text
trajecta --config quickstart.toml config path
```

需要让多个终端共享设置时，可以在 shell 配置文件中定义环境变量。单条命令中的
`--config` 仍具有最高优先级。

## 初始化并查看配置

创建文件并读取其中的叶子字段：

```text
trajecta --config local.toml config init
trajecta --config local.toml config list
trajecta --config local.toml config get resources.cpu_slots
trajecta --config local.toml config get resources.memory_pool_mib
```

`config list` 会用以点分隔的字段路径显示每项设置，适合比较两台机器。机器可读输出
沿用相同路径：

```text
trajecta --format json --config local.toml config list
```

当 `local.toml` 已经存在时，`config init` 保留现有文件。此时可继续使用 `get`、`set`
或 `validate`。

## 设置资源池

任务的运行配置申请需要落在当前空闲的 CPU 和内存范围内。多个小任务可以同时执行，
较大的任务会留在队列中，等候足够资源释放。

### CPU 槽位

如果工作站可供 Trajecta 使用四个逻辑核：

```text
trajecta --config local.toml config set resources.cpu_slots 4
```

运行配置中的 `execution.worker_threads` 需要落在这个范围内。日常使用的工作站可以
留出一部分核心维持交互；专用计算机则可向资源池开放更多逻辑核。

### 内存池与预留量

调度器可分配的内存为：

```text
memory_pool_mib - memory_reserve_mib
```

下面的配置把 Trajecta 内存池设为 4 GiB，其中 512 MiB 留给操作系统、守护进程和同机
程序：

```text
trajecta --config local.toml config set resources.memory_pool_mib 4096
trajecta --config local.toml config set resources.memory_reserve_mib 512
```

内存预留量必须小于内存池。每个运行配置通过 `execution.memory_budget_bytes` 申报预算，
队列会将该值与当前空闲内存比较。工作站上其他程序的占用会随时变化，预留量可以根据
实际使用习惯留得更宽裕。

## 设置读取器与守护进程

默认读取器可直接修改：

```text
trajecta --config local.toml config set default_reader_backend rust
```

运行配置也可针对具体数据集选择 `rust` 或 `native`。需要在多台机器上保持明确选择的
项目，通常把该值写入运行配置；探索性工作则可以采用本机默认值。

任务命令会在需要时启动本地守护进程。队列清空且没有活跃工作进程以后，
`daemon.idle_shutdown_seconds` 决定守护进程继续等待多久：

```text
trajecta --config local.toml config set daemon.idle_shutdown_seconds 600
```

队列与工作进程通过本机进程间通信通道连接。当前配置结构将 `daemon.local_ipc_only` 固定为
`true`。

## 添加执行模板

命名模板可以收纳准备运行配置时反复使用的执行值。下面定义一个单线程、1 GiB
预算并使用 Rust 读取器的模板：

=== "Windows PowerShell"

    ```powershell
    $value = '{"execution":{"worker_threads":1,"memory_budget_bytes":1073741824,"executor":"cpu","meteorology_reader":"rust"}}'
    .\trajecta.exe --config local.toml config set profile_templates.one_worker $value
    ```

=== "Ubuntu 24.04"

    ```bash
    ./trajecta --config local.toml config set profile_templates.one_worker \
      '{"execution":{"worker_threads":1,"memory_budget_bytes":1073741824,"executor":"cpu","meteorology_reader":"rust"}}'
    ```

模板名称属于本机配置键。删除示例模板时使用：

```text
trajecta --config local.toml config unset profile_templates.one_worker
```

## 检查运行环境

`doctor` 命令会读取后续运行任务时使用的同一份配置。显式提供项目后，它还会解析并
校验该项目。

### 常规检查

```text
trajecta --config local.toml doctor
trajecta --config local.toml --project examples/domain-fill-cfsr doctor
```

第一条命令检查配置，并报告当前目录是否发现项目。第二条命令直接选择示例项目，返回值中
会包含项目状态。

### `--deep` 深度检查

```text
trajecta --config local.toml --project examples/domain-fill-cfsr doctor --deep
```

`--deep` 模式会在项目下建立临时目录，实际执行文件创建、同步、改名与清理。已有
资料锁会按本地文件重新校验；每个已有资料根会打开一份受支持的气象文件。最后还会
完成一次 SQLite 创建、WAL 写入、完整性检查、检查点和清理。检查结束后，临时目录
随之删除。

资料放置完成并执行 `project finalize` 后，适合运行这项深度检查。项目迁到另一块磁盘，
或文件系统权限发生变化时，也可以用它重新检查本地环境。

## 安全地修改设置

一轮修改结束后运行：

```text
trajecta --config local.toml config validate
```

`config set` 先解析新值，再校验完整文档，最后以原子替换写回文件。值被拒绝时，原配置
继续保留。读取器名称等标量可以直接传入，数组和对象则使用 JSON 语法。

AI 辅助配置也可以沿用同样的小步流程：先用 `get` 读取当前值，每次只用 `set` 修改一个
选择器，再执行 `config validate`，最后针对目标项目运行 `doctor --deep`。配置文件本身
始终是双方共同读取的来源。

## 首次配置中的常见问题

| 现象 | 优先查看的位置 |
|---|---|
| `config.not_found` | `--config` 选择的路径或 `TRAJECTA_CONFIG` 值 |
| `config.invalid_schema` | 字段名、值类型、内存预留量与内存池的关系，或模板中的资源申请 |
| 任务长时间处于 `queued` | 运行配置申请的 CPU、内存与资源池当前余量 |
| `doctor.filesystem_unwritable` | 项目根目录的写入与改名权限 |
| `doctor.lock_invalid` | 资料锁路径、文件内容，或迁移项目后资料所在位置 |
| `doctor.data_inspect_failed` | 读取器选择及资料根中的首个受支持文件 |

[配置参考](../reference/configuration.md)列出全部字段和约束，具体诊断码可在
[故障索引](../operations/troubleshooting.md)中查找。
