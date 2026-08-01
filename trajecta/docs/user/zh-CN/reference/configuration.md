---
title: 本机配置
description: Trajecta 本机配置的选择优先级、字段、约束、模板和安全修改规则。
---

# 本机配置

本机配置描述资源容量与本地运行策略。其中不保存 Case 科学设置、项目身份、资料 lock
或 provider 凭据。

## 文件选择

Trajecta 按以下优先级选择一份配置：

1. `--config PATH`；
2. `TRAJECTA_CONFIG`；
3. Windows 使用 `%APPDATA%/Trajecta/config.toml`，Linux 使用
   `$XDG_CONFIG_HOME/trajecta/config.toml`；
4. 平台变量缺失时使用文档规定的用户主目录回退路径。

`trajecta config path` 可显示当前路径。命令不会合并多份配置。

## 字段

| Selector | 类型与约束 | 含义 |
| --- | --- | --- |
| `schema_version` | `trajecta.config/v1` | 磁盘合同身份 |
| `default_reader_backend` | `rust` 或 `native` | Profile 未覆盖时使用的 reader |
| `daemon.idle_shutdown_seconds` | 不小于 0 的整数 | 本地 daemon 空闲寿命 |
| `daemon.local_ipc_only` | 固定为 `true` | 禁止开放网络控制端点 |
| `resources.cpu_slots` | 不小于 1 的整数 | 调度器共享 CPU 容量 |
| `resources.memory_pool_mib` | 不小于 1 的整数 | 调度器共享内存池 |
| `resources.memory_reserve_mib` | 不小于 0 且小于内存池 | 调度准入时保留的内存 |
| `monitoring.sample_interval_ms` | 不小于 100 的整数 | 资源观测间隔 |
| `monitoring.maximum_median_overhead_percent` | 0 至 1 | 监控开销上限 |
| `profile_templates.<name>.execution` | 对象 | 命名执行默认值 |

每个模板需要正数 `worker_threads` 与 `memory_budget_bytes`、非空 executor 名称，以及
`rust` 或 `native` 气象 reader。模板线程数不得超过 `resources.cpu_slots`。

## 安全修改

```text
trajecta config get resources.memory_pool_mib
trajecta config set resources.memory_reserve_mib 1024
trajecta config set resources.memory_pool_mib 8192
trajecta config unset profile_templates.experimental
trajecta config validate
```

`set` 根据目标字段解析值，并在原子替换前校验完整文档。修改被拒绝时，原文件字节保持
不变。`config init` 不会覆盖现有文件。

规范字段结构由 [JSON 与 JSONL schema](schemas.md) 页面生成。
