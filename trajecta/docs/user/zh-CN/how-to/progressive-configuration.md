---
title: 渐进配置
description: 逐项选择、查看、修改并校验 Trajecta 的本机配置和项目配置。
---

# 渐进配置

Trajecta 把本机运行环境与科学项目分开配置。本机配置管理守护进程、共享 CPU 与内存资源池、
状态采样频率和可选运行模板。项目配置把带名称的案例、运行配置、逻辑资料和资料锁连接起来。

两层配置都支持按字段修改。每次修改都会先构造并校验完整文档，通过后再替换原文件。项目需要
分几次完成，或需要由人和自动化工具轮流审阅时，可以逐项填写，无需一次生成整份 JSON 或 YAML。

## 选择本机配置文件

同一账户下有多套运行环境时，编辑前先查看当前选择：

```text
trajecta config path
```

配置路径按以下优先级确定：

1. 当前命令的全局参数 `--config FILE`；
2. 环境变量 `TRAJECTA_CONFIG`；
3. 平台默认路径。

| 平台 | 默认路径 |
| --- | --- |
| Windows | `%APPDATA%\Trajecta\config.toml` |
| Ubuntu 24.04 及其他受支持 Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/trajecta/config.toml` |

独立性能测试或第二套本机资源池可以使用显式路径：

```text
trajecta --config configs/workstation.toml config path
trajecta --config configs/workstation.toml config validate
```

所选配置还决定守护进程端点和任务数据库。两个终端若指向不同配置，即使位于同一台计算机，
也会看到不同的任务队列。

## 创建并查看配置

`config init` 根据当前平台和硬件创建初始文件。目标文件已经存在时不会覆盖：

```text
trajecta config init
trajecta config list
```

`config list` 按稳定的点分字段顺序显示所有叶子值。读取单个值可使用：

```text
trajecta config get resources.cpu_slots
trajecta config get resources.memory_pool_mib
trajecta config get default_reader_backend
```

脚本读取时，把全局输出格式放在子命令之前：

```text
trajecta --format json config get resources.memory_pool_mib
```

## 修改带类型的值

`config set` 接受点分字段和一个 JSON 值。无法解析为 JSON 的普通文本会按字符串处理：

```text
trajecta config set resources.cpu_slots 8
trajecta config set resources.memory_pool_mib 16384
trajecta config set resources.memory_reserve_mib 2048
trajecta config set default_reader_backend '"rust"'
trajecta config validate
```

资源字段的用途如下：

| 字段 | 含义 |
| --- | --- |
| `resources.cpu_slots` | 所有已准入任务共享的 CPU 槽位总数 |
| `resources.memory_pool_mib` | 本地调度器用于任务准入的内存总量 |
| `resources.memory_reserve_mib` | 留给操作系统和其他程序、不进入调度池的内存 |

`memory_reserve_mib` 必须小于 `memory_pool_mib`。需要同时调整两者时：

- 扩大资源池，先增大 `memory_pool_mib`；
- 缩小资源池，先降低 `memory_reserve_mib`；
- 全部修改完成后运行 `config validate`。

修改值未通过校验时，原配置文件的字节内容保持不变。

## 添加运行模板

可以用一个结构化值创建带名称的运行模板：

```text
trajecta config set profile_templates.workstation '{"execution":{"worker_threads":4,"memory_budget_bytes":2147483648,"executor":"local","meteorology_reader":"rust"}}'
trajecta config get profile_templates.workstation
trajecta config validate
```

模板中的工作线程数要落在 `resources.cpu_slots` 范围内，内存预算以字节表示。按名称移除模板：

```text
trajecta config unset profile_templates.workstation
```

`config unset` 只用于可选模板字段，必需配置字段不能移除。

!!! tip "模板只保存本机执行设置"

    模拟时段、区域和粒子群仍写在案例中。模板适合复用线程、内存和读取器选择。

## 按字段编辑项目索引

先查看项目当前状态和完整索引：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
```

`project get`、`project set` 和 `project unset` 使用项目索引中的选择器。修改前可先读取目标：

```text
trajecta --project PROJECT project get index
trajecta --project PROJECT project get profile.quickstart
```

值可以使用 JSON，也可以是 YAML 标量或结构。命令会完成以下步骤：

1. 在内存中应用修改；
2. 将项目相对路径解析到项目根目录内；
3. 校验完整项目索引；
4. 通过同目录临时文件原子替换原文件。

例如，为运行配置指定本机模板：

```text
trajecta --project PROJECT project set profile.quickstart.template workstation
trajecta --project PROJECT project validate
```

绝对路径、`../data` 这类访问上级目录的写法、重复名称，以及最终指向项目根目录外的路径都会被拒绝。
资料目录放在项目根目录内，项目在 Windows 与 Ubuntu 之间移动时更容易保持路径有效。

## 保留草稿状态

运行配置必填项尚未填写完整时，项目可以保持 `draft`。这一状态表示文档结构可识别，但还不能
解析成完整运行请求。未知字段、错误类型、畸形 YAML，以及案例或运行配置中的语义错误会报告为
`error`，不会被归入草稿。

编辑期间可重复执行：

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project show
trajecta --project PROJECT project validate
```

文档齐全后，状态进入 `configured`。气象文件和资料锁通过 `project finalize` 检查后，状态进入
`finalized`。

## 检查整套本机环境

各命令检查的范围不同：

| 命令 | 检查范围 |
| --- | --- |
| `config validate` | 当前选择的 TOML 本机配置 |
| `project validate` | 项目索引及目前存在的案例、运行配置文档 |
| `doctor` | 本机配置、项目状态和基础运行条件 |
| `doctor --deep` | 再检查资料目录、现有资料锁、气象文件和 SQLite/WAL 文件系统 |

常用顺序如下：

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta --project PROJECT doctor
trajecta --project PROJECT doctor --deep
```

深度检查会创建临时 SQLite 数据库，启用 WAL，写入并读取数据，运行完整性检查，将 WAL 检查点
回主库，最后删除临时文件。移动项目、调整存储位置或更改本机资源配置后，适合再运行一次。

## 保存诊断信息

需要把错误交给脚本或问题记录时，可用 JSON 方式重复只读检查：

```text
trajecta --format json config validate
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

输出包含完整命令路径和稳定诊断码。根据诊断修改相应文件后，再运行同一条校验命令。项目通过
校验并完成项目定稿后，再提交较长任务。
