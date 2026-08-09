---
title: Trajecta 参考手册
description: 查阅 Trajecta 0.1.0-alpha.1 的命令、配置字段、项目文档、结构定义、诊断码、结果和支持平台。
---

# 参考手册

本章集中列出 Trajecta `0.1.0-alpha.1` 可供用户使用的命令、字段和文件格式，也收录进程状态、
诊断码、结果表以及支持平台。需要确认某个参数或机器输出字段时，可以直接从本章查找。

第一次运行请先阅读[开始使用](../getting-started/index.md)。需要完成某个具体任务时，可从
[操作指南](../how-to/index.md)选择相应流程。参考章节以便于快速查找的说明和表格为主。

## 公开接口

| 接口 | 格式 | 参考页面 |
| --- | --- | --- |
| 命令行 | 人类可读文本、JSON 响应或 JSONL 数据流 | [CLI 命令树](cli.md) |
| 本机配置 | TOML | [本机配置字段](configuration.md) |
| 项目索引 | YAML | [项目文档模型](documents.md) |
| 案例与运行配置 | YAML 或 JSON 文档 | [案例与运行配置字段](documents.md) |
| 资料锁与资料计划 | JSON | [项目文档](documents.md)和[结构定义](schemas.md) |
| 任务与结果机器输出 | 带版本的 JSON 或 JSONL | [结构定义](schemas.md) |
| 运行结果 | JSON 清单、SQLite、溯源信息和可选报告 | [结果与 SQLite](results-sqlite.md) |
| 诊断 | 带命名空间的代码和上下文字段 | [诊断码索引](diagnostics.md) |
| 命令状态 | 进程退出码 | [退出码](exit-codes.md) |
| 发行包 | Windows 或 Ubuntu 压缩包 | [平台矩阵](platforms.md) |

## 版本与格式稳定性

软件版本与数据格式分别编号。可执行文件使用语义化预发布版本 `0.1.0-alpha.1`；本机文档和
数据流则携带 `trajecta.config/v1`、`trajecta.cli-stream-item/v1` 等 `schema_version`。
只有格式约定发生变化时才增加结构版本，普通实现或文档更新不会自动创建新标识。

用户和自动化使用的公开接口包括：

- 命令路径及已记录的参数含义。
- JSON 响应格式和 JSONL 流记录。
- 本机配置、项目、案例、运行配置与资料锁。
- 运行清单、SQLite 结构、溯源信息和结果目录布局。
- 诊断码与进程退出码。

Rust crate API 面向工作区贡献者，Alpha 阶段可能调整内部组织，而公开 CLI 与磁盘约定可以保持
不变。[Rust API](../developer/api.md)提供相应入口。

## 自动生成的参考页面

以下三页从实现重新生成：

| 页面 | 生成来源 |
| --- | --- |
| CLI 命令树 | 当前二进制 `--help` 和 `M5_CLI_CONTRACT.v1.json` |
| JSON 与 JSONL 结构 | `testdata/` 下的公开结构定义和示例 |
| 诊断码 | 生产 CLI 与任务源码中的带命名空间诊断字符串 |

文档校验会检查生成结果。命令、结构或诊断发生变化时，需要在同一修改中更新相应参考页。

## Alpha 范围

当前平台、本地守护进程、只读清理计划、结果接口和未来扩展入口集中列在
[Alpha 范围](alpha.md)。当前软件版本为 `0.1.0-alpha.1`。
