---
title: 使用 AI 辅助配置 Trajecta
description: 为 AI 助手提供边界清楚的 Trajecta 配置任务，保持单一配置真源，并在本地校验每项建议。
---

# AI 辅助配置

AI 助手可以协助选择字段、解释 diagnostic，并提出一组 `config set` 或 `project set` 命令。
Trajecta 的本地文件和 validator 仍是配置入口。无论修改由人录入，还是由助手提出，最终审阅
的都是同一组 Case、Profile 和项目索引。

本页给出一种可复用的协作方式。Trajecta `0.1.0-alpha.1` 暂未内置 AI 服务、网络 connector
或 plugin runtime。

## 限定单次任务

一次只给出一个结果目标，更容易审阅。适合交给助手的任务包括：

- 为已有 Case 和资料映射增加 RunProfile；
- 保持输出间隔不变，调整模拟时段；
- 解释一条 `project validate` diagnostic；
- 准备 data-plan 审阅清单；
- 对照 forward 与 backward 的 resolved Profile；
- 根据已知粒子量估算 scheduler memory request。

“配置整个实验”会留下较多隐含科研选择。可以先写明运行方向、时段和空间域，再补充
population 及其 release 或 filling 规则。输出间隔、资料家族和本机可用资源也应给出。

## 提供配置真源

项目类任务通常只需以下材料：

| 文件或输出 | 用途 |
| --- | --- |
| `trajecta-project.yml` | 给出 Case、Profile、dataset 和 lock 名称 |
| 所选 Case | 定义时间、空间域、population 和科研意图 |
| 所选 RunProfile | 定义执行方式和资料绑定 |
| `project show` 输出 | 显示派生项目状态和索引视图 |
| `project data-plan` 输出 | 显示所需覆盖与 capability |
| 相关 JSON Schema | 给出精确字段名、类型和允许值 |
| 一条结构化 diagnostic | 标识失败命令和稳定错误码 |

只提供当前任务需要的文件。Provider credential、private key、access token 和无关的绝对路径
可以继续保存在原本的本机位置。

公开 schema 位于源码仓库的 `testdata/`，也可从 [schema 参考](../reference/schemas.md)查找。
CLI 语法以 `trajecta --help` 和[命令参考](../reference/cli.md)为准。

## 请求命令，不维护第二份文档

可以让助手先给出简短说明，再列出确切命令。例如：

```text
我正在编辑 PROJECT 中的 Trajecta 项目。
只使用 0.1.0-alpha.1 已提供的命令。
所选 Profile 为 PROFILE，它使用 CASE。
请给出修改 WORKER_THREADS 为 4 所需的最小 project get/set/unset 命令序列。
保留无关字段。最后列出只读校验命令。不要执行命令。
```

项目文件由此保持为单一真源。每项建议也会成为可见的 shell 命令，执行前便于逐条检查。

确实需要新增结构化文档时，可以让助手从仓库示例开始，返回针对该文件的 patch。请求中附上
对应 schema 和准确的 Trajecta release。

## 应用前读取现状

读取当前值和项目状态：

```text
trajecta --project PROJECT project get profile.PROFILE
trajecta --project PROJECT project status
```

检查建议中的 selector、值类型、单位和路径。名称接近的字段可能使用不同单位，例如执行内存
使用 byte，本机 scheduler pool 使用 MiB。

每次运行一项修改：

```text
trajecta --project PROJECT project set SELECTOR VALUE
trajecta --project PROJECT project show
```

Trajecta 会在替换旧文件前校验更新后的文档。修改被拒绝时，可以把确切 diagnostic code 和
相关 schema 片段一起交回助手。

## 完成本地校验循环

最后一项编辑完成后，根据受影响的层次运行：

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta case resolve PROJECT/cases/CASE.yml
trajecta --project PROJECT project data-plan --output PROJECT/data-plan.json
trajecta --project PROJECT doctor --deep
```

直接阅读 resolved Case 和 data-plan，先检查物理时段、空间域与运行方向，再查看 population
数量或 filling 规则。随后确认输出节奏、dataset profile、coverage anchor 和本地目标根目录。

资料和 lock 已经到位时，再运行：

```text
trajecta --project PROJECT project finalize
trajecta --project PROJECT project status
```

后续运行会读取刚刚审阅的 finalized document。

## 在下一轮提供结构化 diagnostic

JSON 输出能形成紧凑的失败记录：

```text
trajecta --format json --project PROJECT project validate
trajecta --format json --project PROJECT doctor --deep
```

将 `command`、diagnostic `code`、`message` 和 `hint`，连同最小的相关文档片段交给助手。
稳定 code 可以直接映射到 [diagnostic 参考](../reference/diagnostics.md)，比终端排版截图更容易
准确处理。

后续请求可以写成：

```text
请结合所给 schema 解释这条 diagnostic，指出可以解决它的最小 selector 修改。
保留 diagnostic 未涉及的科研选择。先返回供审阅的命令，再列出校验命令。
```

## 单独审阅科研选择

Schema 校验确认文档格式和内部关系。研究设计中的选择可以再做一次明确审阅：

| 选择 | 需要确认的值 |
| --- | --- |
| 方向 | Forward 或 backward |
| 物理时段 | Start、end 与 integration step |
| 输出 | Initial/final 覆盖和 scheduled interval |
| Population | Domain fill、release、air mass 或 ozone 设置 |
| 空间域 | 水平边界与垂直范围 |
| 气象资料 | Dataset family、垂直坐标和 reader |
| 资源 | Worker thread 和 memory budget |

研究需要保留人工记录时，可把最终表格加入项目科研笔记。Trajecta 会在每个运行目录中保存
当时使用的 resolved machine input。

## 资料缺席时分阶段协作

AI 辅助配置可以停在 `configured` 项目状态。生成并审阅 `data-plan.json` 后，通过
[延后资料流程](deferred-data.md)准备气象文件。文件到达后 finalize 同一个项目，无需让助手
填写 DatasetLock。

DatasetLock 来自对本地文件的实际检查。其文件身份和 capability 只有在资料获取及准备完成后
才能确定。
