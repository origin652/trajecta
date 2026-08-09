---
title: 渐进配置 Trajecta
description: 逐项选择、查看、修改和校验 Trajecta 的本机配置与项目配置。
---

# 渐进配置

Trajecta 将本机设置和科研运行文档分开保存。本机配置管理 local daemon、共享 CPU 与内存
池、监测间隔，以及可选的执行模板。项目配置连接具名 Case、RunProfile、资料和 lock。

两层配置都支持小范围的校验式修改。这种方式适合分几次完成项目设置，也方便通过机器可读
输出逐项审阅。

## 选择本机配置文件

同一用户账户下有多套安装时，可在编辑前查询实际路径：

```text
trajecta config path
```

配置路径按以下顺序选择：

1. 当前命令中的 `--config FILE`；
2. `TRAJECTA_CONFIG` 环境变量；
3. 平台默认位置。

| 平台 | 默认位置 |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| Ubuntu 24.04 及其他受支持 Linux 系统 | `${XDG_CONFIG_HOME:-$HOME/.config}/trajecta/config.toml` |

独立基准测试或第二套本机资源池可以使用显式路径：

```text
trajecta --config configs/workstation.toml config path
trajecta --config configs/workstation.toml config validate
```

所选配置也决定 local daemon endpoint 和任务 catalog。因此，指向不同配置文件的命令会
看到不同的本机队列。

## 创建并查看配置

`config init` 根据当前平台创建起始文件。目标文件已经存在时，原文件保持不变。

```text
trajecta config init
trajecta config list
```

`config list` 按稳定的 dotted-key 顺序列出叶子值。读取单项值时使用 `config get`：

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get default_reader_backend
```

脚本可以把全局输出选项放在命令名之前：

```text
trajecta --format json config get resources.memory_pool_mib
```

## 修改带类型的值

`config set` 接收 dotted key 和 JSON 值。无法解析为 JSON 的普通文本按字符串处理。

```text
trajecta config set resources.cpu_slots 8
trajecta config set resources.memory_pool_mib 16384
trajecta config set resources.memory_reserve_mib 2048
trajecta config set default_reader_backend '"rust"'
trajecta config validate
```

Trajecta 会先解析并检查更新后的完整文档，随后替换旧文件。编辑被拒绝时，旧文件的字节内容
不变。

资源字段各自表达以下含义：

| 字段 | 含义 |
| --- | --- |
| `resources.cpu_slots` | 所有已接收任务共享的 scheduler CPU 容量 |
| `resources.memory_pool_mib` | 本机 scheduler 纳入分配的内存总量 |
| `resources.memory_reserve_mib` | 留给操作系统和其他进程、不进入调度池的内存 |

Reserve 小于 pool。增加容量时可先调高 pool；缩小容量时先调低 reserve。完成一组修改后
运行 `config validate`。

## 添加执行模板

具名 Profile 模板可以通过一个结构化值创建：

```text
trajecta config set profile_templates.workstation '{"execution":{"worker_threads":4,"memory_budget_bytes":2147483648,"executor":"local","meteorology_reader":"rust"}}'
trajecta config get profile_templates.workstation
trajecta config validate
```

模板的 worker 数量位于 `resources.cpu_slots` 范围内，内存预算以字节表示。删除整个模板时
使用模板名：

```text
trajecta config unset profile_templates.workstation
```

`config unset` 只用于可选 Profile 模板，配置中的固定字段保留在文档内。

## 通过 selector 编辑项目

先查看项目的派生状态和完整索引：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
```

`project get`、`project set` 与 `project unset` 操作项目索引中的 selector。选择 selector 前
可以先读取当前文档：

```text
trajecta --project PROJECT project get index
trajecta --project PROJECT project get profile.quickstart
```

值可以是 JSON 或 YAML 的标量与结构。命令在项目根目录内解析各条相对路径，校验更新后的
索引，再原子替换文件。

```text
trajecta --project PROJECT project set profile.quickstart.template workstation
trajecta --project PROJECT project validate
```

绝对路径、`../data` 形式的父级穿越、重复名称，以及解析到项目根目录之外的路径都会被
拒绝。将资料根目录放在项目树内，项目在 Windows 和 Linux 之间移动时更容易保持一致。

## 逐步完成 draft

Profile 的必填字段尚未录入完整时，可以保留 draft 状态。Draft 表示可识别的内容仍有缺项。
未知字段、类型错误和格式损坏的 Case 或 RunProfile 会以 error 返回。

编辑期间可以反复执行：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
trajecta --project PROJECT project validate
```

文档完整后，项目从 `draft` 进入 `configured`。`project finalize` 检查所选气象资料和
DatasetLock 后，项目进入 `finalized`。

## 检查完整本机环境

配置校验读取所选 TOML 文件。项目校验读取索引和当前已有文档。Doctor 将这些内容与本机
运行环境合在一起检查：

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta --project PROJECT doctor
trajecta --project PROJECT doctor --deep
```

Deep doctor 会在相应内容存在时打开已配置的资料根目录与 lock。它还会创建临时 SQLite
数据库，依次测试 WAL 模式、integrity check 和 checkpoint，最后清理临时文件。移动项目、
更换存储位置或调整本机配置后，可以用这条命令检查运行环境。

## 查看编辑失败原因

需要保存稳定的诊断记录时，以 JSON 模式重复只读校验：

```text
trajecta --format json config validate
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

输出中的命令路径和 diagnostic code 可以放入 issue 或运行日志。修正所选文件后，重新运行
同一条校验命令，再继续 finalize 或提交任务。
