---
title: 项目、案例、运行配置与资料锁参考
description: 查阅 Trajecta 项目关系、文档字段、路径规则、草稿状态、资料计划、资料锁和项目定稿行为。
---

# 项目、案例、运行配置与资料锁

Trajecta 将可移植的科学设置、本机执行环境和实际使用的气象文件分别保存。项目通过名称连接这些部分，
每份源文档仍可单独阅读和校验。

## 文档职责

| 文档 | 格式 | 主要作用 | 创建或读取命令 |
| --- | --- | --- | --- |
| 项目索引 | YAML，`trajecta-project.yaml` | 登记案例与运行配置，选择内置资料配置，保存可选默认运行配置 | `project init`、`project get/set`、`project validate`、`project finalize` |
| 案例（`Case`） | YAML 或 JSON | 定义科学时间、气象要求、粒子群、数值方法、物理过程和输出计划 | `case validate`、`case resolve`、项目校验、工作进程 |
| 运行配置（`RunProfile`） | YAML 或 JSON | 选择一个案例，提供本机结果目录、资料绑定、读取器和资源 | 项目校验、项目定稿、调度器、工作进程 |
| 资料计划 | JSON，`trajecta.data-plan/v1` | 列出所需资料与能力，以及当前本地状态 | `project data-plan` |
| 资料锁（`DatasetLock`） | JSON | 将逻辑资料绑定到不可变本地文件、散列、覆盖和能力 | `data lock`、`project finalize`、`doctor --deep`、工作进程 |
| 解析后案例 | 结果目录中的 JSON | 展开全部案例组件，并记录源摘要 | `case resolve`、运行准入、工作进程 |
| 解析后运行配置 | 结果目录中的 JSON | 规范化本机路径，并记录实际执行绑定 | 运行准入、工作进程 |

## 项目索引

项目索引的格式版本为 `trajecta.project-index/v1`：

```yaml
schema_version: trajecta.project-index/v1
name: moisture-study
cases:
  wet-season: cases/wet-season.yaml
  dry-season: cases/dry-season.yaml
profiles:
  wet-local:
    path: profiles/wet-local.yaml
    dataset_profiles:
      met: era5-hybrid
  dry-local:
    path: profiles/dry-local.yaml
    dataset_profiles:
      met: era5-hybrid
default_profile: wet-local
```

### 字段

| 字段 | 约束 | 含义 |
| --- | --- | --- |
| `schema_version` | 必须为 `trajecta.project-index/v1` | 项目索引格式版本 |
| `name` | 非空字符串 | 项目的人类可读名称 |
| `cases` | 名称与路径均非空且唯一的映射 | 案例名称到项目相对文档路径 |
| `profiles` | 名称非空且唯一的映射 | 可以提交运行的配置条目 |
| `profiles.<name>.path` | 非空项目相对路径 | 该条目选择的运行配置文档 |
| `profiles.<name>.dataset_profiles` | 逻辑资料 ID 到内置资料配置名称的映射 | 为案例中的每项资料选择准备与锁定规则 |
| `profiles.<name>.template` | 可选非空名称 | 准备期间采用的本机运行模板 |
| `profiles.<name>.template_sha256` | 使用 `template` 时必需；64 位十六进制 | 所选模板的内容散列 |
| `default_profile` | 可选，必须引用已有运行配置 | 允许省略选择时采用的默认配置 |

同一映射层中的键必须唯一。原始 YAML 在转换为类型化索引前就会检查重复键，包括
`dataset_profiles` 中的嵌套重复。

## 案例与运行配置的关系

项目可以包含多个案例和多个运行配置。每个项目运行配置条目选择一份 `RunProfile` 文档，该文档
的 `case_path` 再选择一个已登记案例。多个运行配置可以指向同一案例，用于比较读取器、线程、
内存或结果目录。

单个运行配置条目只选择一个案例。四个案例分别在两种机器设置下运行时，可以登记八个带名称的组合。
每份提交回执由此明确对应一个解析后案例和一个解析后运行配置。

案例使用的所有逻辑资料 ID 都必须出现在该运行配置条目的 `dataset_profiles` 中。多余或缺失映射
会在项目校验或定稿前检查中报告。

## 案例文档

案例采用当前数字结构版本，并设置 `kind: case`。顶层对象拒绝未知字段。

| 字段 | 作用 |
| --- | --- |
| `schema_version` | 案例文档数字结构版本 |
| `kind` | 固定为 `case` |
| `metadata` | 名称、说明、作者或元数据约定支持的标签 |
| `time` | 方向、起止时刻、输送步长和输出时间要求 |
| `meteorology` | 区域、逻辑资料引用、必需字段、覆盖和垂直解释 |
| `particle_population` | 释放、气团、臭氧或区域填充的初始化与生命周期 |
| `substances` | 跟踪物质定义和初始质量关系 |
| `numerics` | 积分器和边界控制设置 |
| `physics` | 所选物理模块及参数 |
| `outputs` | 产品和输出事件设置 |

组件可以内嵌，也可以引用项目内组件文档。`case resolve` 会展开引用、规范化数值并记录每个源
摘要。工作进程接收解析后形式，因此源组件在任务准入后发生修改，不会改变该执行轮次。

校验意图决定文档至少需要哪些部分：

```text
trajecta case validate cases/study.yaml --intent simulation
trajecta case validate cases/study.yaml --intent met-probe
trajecta case validate cases/study.yaml --intent migration
```

`simulation` 要求数值运行所需的完整组件。`met-probe` 可以使用只关注气象查询覆盖的较小案例。
`migration` 用于文档转换检查。

## 运行配置文档

运行配置采用当前数字结构版本，并设置 `kind: run_profile`。

| 字段 | 作用 |
| --- | --- |
| `schema_version` | 运行配置数字结构版本 |
| `kind` | 固定为 `run_profile` |
| `metadata` | 运行配置说明信息 |
| `case_path` | 指向唯一所选案例的本地路径 |
| `output_root` | 创建独立运行和执行轮次目录的根路径 |
| `datasets` | 逻辑资料绑定，含资料锁、根目录、可选缓存目录和可选读取器覆盖 |
| `profile_sources` | 气象读取器使用的明确配置文件或非递归目录源 |
| `execution.worker_threads` | 正数 CPU 调度请求 |
| `execution.memory_budget_bytes` | 以字节表示的正数内存请求 |
| `execution.executor` | 非空执行器标识 |
| `execution.meteorology_reader` | 没有单项覆盖时采用的 `rust` 或 `native` 读取器 |

解析后运行配置会规范化本机路径，并记录源摘要。内存预算在队列准入时向上取整到 MiB。单项资料
绑定可以为一个逻辑资料覆盖运行配置的默认读取器。

## 项目路径规则

项目索引中的路径同时经过词法和规范化范围检查：

- 路径以项目根目录为基准；
- 项目索引使用 `/` 作为可移植分隔符；
- 空路径段、重复分隔符、`.` 与 `..` 路径段会被拒绝；
- 绝对路径和规范化后越过项目根目录的路径会被拒绝；
- 已存在文件经规范化解析后也不能通过符号链接越过项目根目录。

`project set` 会先校验修改后的完整索引再写入。路径被拒绝时，索引字节保持不变，也不会在项目外
创建文件。

运行配置中的资料目录和结果目录描述当前计算机的本地路径。公开项目格式要求的资料计划和资料锁
路径仍使用项目相对形式。

## 草稿、已配置与已定稿

| 状态 | 含义 | 可进行的工作 |
| --- | --- | --- |
| `draft` | 案例或运行配置仍缺少必填值 | 继续使用 `project set` 补充字段，查看部分状态 |
| `configured` | 文档可解析，资料、资料锁或结果目录尚待准备 | 生成资料计划、准备文件并校验 |
| `finalized` | 文档、资料锁、能力、覆盖和结果目录均已就绪 | 运行 `doctor`，并提交运行配置 |

运行配置缺少必填值时可以保持草稿。未知字段、错误类型、重复键和案例语义错误会作为文档错误返回。

## 资料计划

```text
trajecta --project PROJECT project data-plan
trajecta --format json --project PROJECT project data-plan --output data-plan.json
```

资料计划根据所选案例推导时空覆盖，再按资料和能力列出要求。它会报告本地状态为 `missing`、
`partial` 或 `ready`。要求、根目录和能力采用确定顺序，因此项目状态相同会产生相同计划字节。

项目可以在资料到达前处于 `configured`。计划只描述需要准备的内容，不创建资料锁。文件到齐后，
执行项目定稿时会读取实际元数据和文件内容。

## 资料锁与项目定稿

资料锁记录：

- 结构版本和内置资料配置散列；
- 覆盖区间与空间能力集合；
- 每个文件的相对路径、大小和 SHA-256；
- 解析锁定路径所需的本地根目录名称；
- 验证资料绑定所需的读取器和元数据。

`data lock` 可以根据资料根、内置资料配置和案例直接构建一份锁：

```text
trajecta data lock --root DATA --profile PROFILE --case CASE --output LOCKFILE
```

目标已存在时，默认返回 `data.lock_exists`。加入 `--replace` 后，只有完整构建成功才替换旧文件。

项目定稿会一次处理全部所选映射：

```text
trajecta --project PROJECT project finalize
```

它会校验项目索引和引用文档，检查运行配置与案例的选择关系，扫描资料目录，推导所需覆盖与能力，
构建候选资料锁，并检查结果目录。全部预检通过后才替换现有资料锁；预检失败会返回嵌套诊断，并
保留旧文件字节。

案例、运行配置、资料映射、模板散列或锁定文件发生变化后，旧绑定会失效。下一次提交前需要重新
生成资料计划，准备变化的文件并重新完成项目定稿。
