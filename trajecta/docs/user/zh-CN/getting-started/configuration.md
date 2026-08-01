---
title: 初始化配置与 doctor 检查
description: 渐进创建 Trajecta 本机配置，并使用 validate 与 deep doctor 进行检查。
---

# 初始化配置与 doctor 检查

本机配置描述资源上限和 daemon 行为，不承载 Case 或科学意图。RunProfile 提供本机路径
绑定，因此项目文档可以在不同机器间迁移。

## 路径优先级

Trajecta 按以下顺序选择配置：

1. 全局参数 `--config PATH`；
2. 环境变量 `TRAJECTA_CONFIG`；
3. 平台默认路径。

`config path` 只显示所选路径，不写入文件。

## 渐进设置

```text
trajecta --config local.toml config init
trajecta --config local.toml config set resources.cpu_slots 4
trajecta --config local.toml config set resources.memory_reserve_mib 512
trajecta --config local.toml config set resources.memory_pool_mib 4096
trajecta --config local.toml config validate
```

`memory_reserve_mib` 必须小于 `memory_pool_mib`。更新失败时，旧配置字节保持不变。
`config get`、`set` 和 `unset` 支持人类或 AI 逐项设置，无需维护中间 JSON 文件。

## Doctor 层级

```text
trajecta --config local.toml doctor
trajecta --config local.toml --project PROJECT doctor --deep
```

普通 doctor 检查配置和本地运行前提。Deep 模式会真实执行 SQLite 创建、WAL、
integrity、checkpoint 和清理。选择项目后，它还通过公开验证接口检查已有 lock 与数据根。

Doctor 返回 error 时，应将其视为 preflight 失败。请记录 diagnostic code，修正所选配置
或项目后再提交任务。
