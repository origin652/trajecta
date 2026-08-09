---
title: Trajecta 操作指南
description: 说明渐进配置、延后准备资料、finalize、队列、事件、取消、重跑、dry-run prune、结果读取和 AI 辅助。
---

# 操作指南

操作指南从一个具体任务出发，适合已经了解 Trajecta 项目基本结构的读者。第一次运行可从
[15 分钟快速开始](../getting-started/quickstart.md)进入；需要完整科研流程时，可选择相应的
[教程](../tutorials/index.md)。

本节命令使用三个占位符：

| 占位符 | 含义 | 示例 |
| --- | --- | --- |
| `PROJECT` | 项目目录或 `trajecta-project.yml` 的路径 | `examples/domain-fill-cfsr` |
| `PROFILE` | 项目索引中的 Profile 名称 | `quickstart` |
| `RESULT` | Job series ID、run ID 或运行结果目录 | `runs/0198.../attempt-1` |

项目文档中的路径以项目根目录为基准。示例统一使用正斜杠；在这里列出的 Windows
PowerShell 和 POSIX shell 命令都可以识别这种写法。

## 按任务选择指南

| 要完成的工作 | 指南 | 主要内容 |
| --- | --- | --- |
| 修改一项本机配置或项目配置 | [渐进配置](progressive-configuration.md) | 配置文件选择、类型化值、模板、项目 selector 和校验 |
| 气象资料尚未到齐时先准备项目 | [先配置项目，后准备资料](deferred-data.md) | 项目状态、data-plan、资料请求、本地检查和 finalize |
| 选择前台运行或提交后台队列 | [前台运行与任务队列](run-queue.md) | 提交、资源接收、状态查询、等待和 daemon 托管 |
| 向终端、脚本或监控程序提供状态 | [跟踪任务事件](events.md) | 全局游标、单任务过滤、JSONL 结构、断线续读和轮询 |
| 停止任务或创建新的 attempt | [取消、重跑、隐藏与清理计划](cancel-rerun-prune.md) | 安全取消、强制停止、attempt 历史、列表可见性和 dry-run 清理计划 |
| 阅读完整或部分运行结果 | [检查和读取结果](results.md) | inspect、verify、轨迹流、报告和 SQLite 高级查询 |
| 借助 AI 准备配置 | [AI 辅助配置](ai-companion.md) | 共享配置真源、限定任务、命令审阅和本地校验 |

## 新项目的一般顺序

一个新项目通常按以下顺序形成：

1. 创建或选择本机配置。
2. 初始化项目，加入 Case 与 RunProfile 文档。
3. 项目进入 `configured` 状态后生成 data-plan。
4. 准备所需资料，并抽查有代表性的输入文件。
5. Finalize 项目，生成不可变的资料锁定记录。
6. 前台提交运行，或交给后台队列。
7. 跟踪持久化事件，随后检查终态结果。

各命令都可以通过 `--format json` 输出机器可读 envelope。事件流和轨迹流还支持
`--format jsonl`。[Schema 参考](../reference/schemas.md)说明这些记录的结构，
[diagnostic 索引](../reference/diagnostics.md)给出稳定错误码及处理入口。

## 修改运行环境前

`config set`、`config unset`、`project set` 和 `project unset` 会先校验新文档，再替换原
文件。编辑失败时保留命令输出，其中会指出需要处理的 selector 和文档。

Finalized 项目将解析后的配置与一份资料清单绑定。Case、Profile、资料映射或气象文件发生
变化后，在下一次提交前重新执行 `project finalize`。已经生成的 attempt 会在自己的结果
目录中保留当时使用的 resolved document 和 provenance。
