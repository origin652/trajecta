---
title: AI 辅助配置
description: 把 Trajecta 配置任务限定在清晰范围内，保持单一配置来源，并在本机校验每项建议。
---

# AI 辅助配置

AI 助手可以帮助查找字段、解释诊断，并给出 `config set` 或 `project set` 的修改顺序。实际配置
仍保存在 Trajecta 的本地文件中，并由同一套命令校验。无论修改建议来自研究者还是 AI，最终
审阅的都是同一份案例、运行配置和项目索引。

`0.1.0-alpha.1` 没有内置 AI 服务。使用本页方法时，AI 助手与 Trajecta 命令行分开运行。

## 明确一次配置任务的范围

一次协作可以只处理一个明确问题，例如：

- 为现有案例和资料映射增加一个运行配置；
- 保持输出间隔不变，只修改模拟时段；
- 解释一条 `project validate` 诊断；
- 为资料计划列出核对项目；
- 比较正向与反向运行配置；
- 根据已知粒子数估算调度内存请求。

向助手提供研究中已经确定的量。通常需要确认积分方向、物理时段、区域和粒子群。释放或填充
规则、输出间隔、资料系列及本机可用资源也可以写入同一份任务说明。尚未确定的科学选择可直接
标记为待讨论，避免工具根据零散上下文猜测。

## 提供最小配置材料

项目配置任务通常只需要以下内容：

| 文件或命令输出 | 用途 |
| --- | --- |
| `trajecta-project.yaml` | 确认案例、运行配置、逻辑资料和资料锁名称 |
| 当前案例 | 确认时段、区域、粒子群和科学设置 |
| 当前运行配置 | 确认执行资源和资料绑定 |
| `project show` 输出 | 查看项目状态和解析后的索引 |
| `project data-plan` 输出 | 查看资料覆盖和能力要求 |
| 相关 JSON Schema | 确认字段名、类型与允许值 |
| 一条结构化诊断 | 确认失败命令和稳定诊断码 |

当前问题所需的片段通常已经足够。服务方凭据、私钥、访问令牌和无关绝对路径继续保存在原来的
本机位置，无需放入提示词。

公开结构定义位于源码仓库的 `testdata/`，也可从
[结构定义参考](../reference/schemas.md)查找。命令语法以 `trajecta --help` 和
[CLI 参考](../reference/cli.md)为准。

## 让助手给出可审阅命令

修改已有项目时，可以要求助手先说明变更，再给出最短命令序列。例如：

```text
我正在编辑 PROJECT 下的 Trajecta 项目。
只使用 0.1.0-alpha.1 已有命令。
当前运行配置是 PROFILE，它选择 CASE。
请给出最少的 project get/set/unset 命令，把 WORKER_THREADS 改为 4。
保留所有无关字段。
最后列出只读校验命令。不要执行命令。
```

这种形式让项目文件继续作为唯一配置来源。每个拟议修改都能在执行前检查，也会清楚出现在终端
历史中。

需要新建结构化文档时，可让助手以仓库中的某个示例为基础，返回针对该文件的补丁。提示词中同时
给出对应结构定义和明确的 Trajecta 版本。

## 执行前检查当前值

先读取目标运行配置和项目状态：

```text
trajecta --project PROJECT project get profile.PROFILE
trajecta --project PROJECT project status
```

核对选择器、值类型、单位和路径。含义相近的字段可能采用不同单位，例如运行配置的内存预算以
字节表示，本机调度资源池以 MiB 表示。

一次应用一项修改：

```text
trajecta --project PROJECT project set SELECTOR VALUE
trajecta --project PROJECT project show
```

Trajecta 会在替换旧文件前校验完整新文档。修改失败后，可以把完整诊断码和相关结构定义片段
交给助手继续分析。

!!! tip "先 `get`，再 `set`"

    先读取目标值，可以避免改错同名运行配置或使用错误单位。

## 完成本机校验

最后一项修改完成后，按受影响层运行：

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta case resolve PROJECT/cases/CASE.yaml
trajecta --project PROJECT project data-plan --output PROJECT/data-plan.json
trajecta --project PROJECT doctor --deep
```

直接阅读解析后的案例和资料计划，核对：

| 项目 | 需要确认的内容 |
| --- | --- |
| 时间 | 物理起止时刻、方向和积分步长 |
| 区域 | 水平边界、垂直范围和插值边缘 |
| 粒子群 | 数量、填充或释放规则、随机种子 |
| 输出 | 起止事件和中间输出间隔 |
| 气象资料 | 资料系列、垂直坐标和读取器 |
| 资料覆盖 | 锚点、能力和本地目标目录 |
| 资源 | 工作线程和内存预算 |

资料和资料锁已经准备好时，再运行：

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
```

下一次提交会使用这套已经审阅并完成项目定稿的文档。

## 用结构化诊断继续对话

JSON 输出可以形成简洁的失败记录：

```text
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

向助手提供 `command`、`code`、`message` 和 `hint`，再附上最小相关配置片段。稳定诊断码可以
直接对应[诊断码参考](../reference/diagnostics.md)，比终端截图更便于定位。

后续提示词可以这样写：

```text
请结合给出的结构定义解释这条诊断。
找出能够解决问题的最小选择器修改。
保留诊断未涉及的科学设置。
返回供我审阅的命令，并在最后附上校验命令。
```

## 单独审阅科学选择

结构校验确认文档格式和内部关系。正式运行前，再按研究设计审阅科学选择：

| 选择 | 需要确认的值 |
| --- | --- |
| 方向 | 正向或反向 |
| 物理时段 | 起点、终点和积分步长 |
| 输出 | 起止事件与计划间隔 |
| 粒子群 | 区域填充、定时释放、气团或臭氧设置 |
| 区域 | 水平边界和垂直范围 |
| 气象资料 | 资料系列、垂直坐标和读取器 |
| 本机资源 | 工作线程和内存预算 |

研究需要保留人工决策记录时，可以把最终表格放入项目笔记。每个结果目录还会保存实际使用的
解析后输入。

## 资料尚未到齐时

AI 辅助配置可以在项目达到 `configured` 后结束。生成并审阅 `data-plan.json`，再按
[延后准备资料](deferred-data.md)流程下载气象文件。文件到齐后，在同一项目上执行
`project finalize`。

资料锁根据本地文件的实际大小、SHA-256 和读取能力生成。Trajecta 会在检查文件后创建它，助手提供的文本无法
代替这一步。
