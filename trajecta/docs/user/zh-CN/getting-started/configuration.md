---
title: 初始化配置与 doctor 检查
description: 创建 Trajecta 本机配置，设置本地资源池，并使用 doctor 检查项目运行环境。
---

# 初始化配置与 doctor 检查

每条 Trajecta 命令都会选择一份本机配置。该文件记录本地调度器可使用的资源、默认气象
reader、监测间隔和 daemon 空闲时间。科学设置保存在项目的 Case 中，资料路径与单次运行
的资源申请保存在 RunProfile 中。

分开保存后，同一个项目可以在一台机器上使用四核 Profile，在另一台机器上使用更大的
Profile，模拟时段、粒子总体和数值设置仍保持一致。

## 配置控制的内容

| 范围 | 主要字段 | 作用 |
|---|---|---|
| Reader 默认值 | `default_reader_backend` | Profile 未指定时采用的 reader |
| 本地 daemon | `daemon.idle_shutdown_seconds` | 本地 daemon 空闲后保留的时间 |
| CPU 池 | `resources.cpu_slots` | 所有活动任务合计可占用的 CPU 槽位 |
| 内存池 | `resources.memory_pool_mib` 与 `resources.memory_reserve_mib` | 调度器考虑的总内存和留在 worker 分配之外的部分 |
| 运行监测 | `monitoring.sample_interval_ms` | 资源状态的采样间隔 |
| 模板 | `profile_templates.<name>` | 准备项目 Profile 时可复用的执行值 |

`config init` 会读取逻辑 CPU 数量和物理内存。初始 reserve 取物理内存的四分之一，reader
默认选择 Rust。随后可通过 `config get` 和 `config set` 按本机用途确定最终配置。

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

需要让多个终端共享设置时，可以在 shell profile 中定义环境变量。单条命令中的
`--config` 仍具有最高优先级。

## 初始化并查看配置

创建文件并读取其中的叶子字段：

```text
trajecta --config local.toml config init
trajecta --config local.toml config list
trajecta --config local.toml config get resources.cpu_slots
trajecta --config local.toml config get resources.memory_pool_mib
```

`config list` 会按点分 selector 显示每个设置，适合比较两台机器。机器输出沿用相同的
selector：

```text
trajecta --format json --config local.toml config list
```

当 `local.toml` 已经存在时，`config init` 保留现有文件。此时可继续使用 `get`、`set`
或 `validate`。

## 设置资源池

任务的 Profile 申请需要落在当前空闲的 CPU 和内存范围内。多个小任务可以同时执行，
较大的任务会留在队列中，等候足够资源释放。

### CPU 槽位

如果工作站可供 Trajecta 使用四个逻辑核：

```text
trajecta --config local.toml config set resources.cpu_slots 4
```

RunProfile 中的 `execution.worker_threads` 需要落在这个范围内。日常使用的工作站可以
留出一部分核心维持交互；专用计算机则可向资源池开放更多逻辑核。

### 内存池与 reserve

调度器可分配的内存为：

```text
memory_pool_mib - memory_reserve_mib
```

下面的配置把 Trajecta 内存池设为 4 GiB，其中 512 MiB 留给操作系统、daemon 和同机
程序：

```text
trajecta --config local.toml config set resources.memory_pool_mib 4096
trajecta --config local.toml config set resources.memory_reserve_mib 512
```

Reserve 保持小于 pool。每个 RunProfile 通过 `execution.memory_budget_bytes` 申报预算，
队列会将该值与当前空闲内存比较。工作站上其他程序的占用会随时变化，reserve 可以根据
实际使用习惯留得更宽裕。

## Reader 与 daemon 设置

默认 reader 可直接修改：

```text
trajecta --config local.toml config set default_reader_backend rust
```

RunProfile 也可针对具体数据集选择 `rust` 或 `native`。需要在多台机器上保持明确选择的
项目，通常把该值写入 Profile；探索性工作则可以采用本机默认值。

任务命令需要时会启动本地 daemon。队列和活动 worker 清空以后，
`daemon.idle_shutdown_seconds` 决定 daemon 继续等待多久：

```text
trajecta --config local.toml config set daemon.idle_shutdown_seconds 600
```

队列与 worker 通过本机通道通信。当前 schema 将 `daemon.local_ipc_only` 固定为
`true`。

## 添加执行模板

命名模板可以收纳准备 RunProfile 时反复使用的执行值。下面定义一个单线程、1 GiB
预算并使用 Rust reader 的模板：

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

## 运行 doctor

Doctor 会读取后续运行命令选择的同一份配置。显式提供项目时，它也会解析并校验该项目。

### 常规检查

```text
trajecta --config local.toml doctor
trajecta --config local.toml --project examples/domain-fill-cfsr doctor
```

第一条命令检查配置，并报告当前目录是否发现项目。第二条命令直接选择示例项目，返回值中
会包含项目状态。

### Deep 检查

```text
trajecta --config local.toml --project examples/domain-fill-cfsr doctor --deep
```

Deep 模式会在项目下建立临时目录，实际执行文件创建、同步、改名与清理。已有
DatasetLock 会按本地文件重新校验；每个已有资料根会打开一份受支持的气象文件。最后还会
完成一次 SQLite 创建、WAL 写入、完整性检查、checkpoint 和清理。检查结束后，临时目录
随之删除。

资料放置并 finalize 后适合运行 deep doctor。项目迁到另一块磁盘，或文件系统权限发生
变化时，也可以用它重新检查本地环境。

## 安全地修改设置

一轮修改结束后运行：

```text
trajecta --config local.toml config validate
```

`config set` 先解析新值，再校验完整文档，最后以原子替换写回文件。值被拒绝时，原配置
继续保留。Reader 名称等标量可以直接传入；数组和对象使用 JSON 语法。

AI 辅助配置也可以沿用同样的小步流程：先用 `get` 读取当前值，每次只用 `set` 修改一个
selector，再执行 `config validate`，最后针对目标项目运行 `doctor --deep`。配置文件本身
始终是双方共同读取的来源。

## 首次配置中的常见问题

| 现象 | 优先查看的位置 |
|---|---|
| `config.not_found` | `--config` 选择的路径或 `TRAJECTA_CONFIG` 值 |
| `config.invalid_schema` | 字段名、值类型、reserve 与 pool 的关系，或模板资源申请 |
| 任务长时间处于 queued | Profile 申请的 CPU、内存与资源池当前余量 |
| `doctor.filesystem_unwritable` | 项目根目录的写入与改名权限 |
| `doctor.lock_invalid` | DatasetLock 路径、文件内容，或迁移项目后资料所在位置 |
| `doctor.data_inspect_failed` | Reader 选择及资料根中的首个受支持文件 |

[配置参考](../reference/configuration.md)列出全部字段和约束，具体 diagnostic code 可在
[故障索引](../operations/troubleshooting.md)中查找。
