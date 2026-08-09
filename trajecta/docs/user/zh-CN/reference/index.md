---
title: Trajecta 产品参考
description: 查找 Trajecta 0.1.0-alpha.1 的命令和配置字段，以及项目文档、schema、diagnostic、结果与支持平台。
---

# 产品参考

本参考描述 Trajecta `0.1.0-alpha.1` 的公开 product surface。Command synopsis 与 field name
可以从这里查询。Schema、process status 和 diagnostic code 也有独立页面，result table 与
supported platform 则位于各自的专题参考。

完整工作流可以从[入门](../getting-started/index.md)开始，也可以在[操作指南](../how-to/index.md)
中选择任务。本区以查找表和准确合同为主，教程则保留完整操作顺序。

## 公开接口

| 接口 | 格式 | 参考 |
| --- | --- | --- |
| Command-line interface | Human text、JSON envelope 或 JSONL stream | [CLI 命令树](cli.md) |
| 机器配置 | TOML | [配置字段](configuration.md) |
| Project index | YAML | [项目文档模型](documents.md) |
| Case 与 RunProfile | YAML 或 JSON document contract | [Case 与 Profile 字段](documents.md) |
| DatasetLock 与 data plan | JSON | [项目文档](documents.md)和 [schema](schemas.md) |
| Job 与 result machine output | Versioned JSON 或 JSONL | [Schema](schemas.md) |
| Run product | JSON manifest、SQLite、provenance bundle 和 optional report | [结果与 SQLite](results-sqlite.md) |
| Diagnostic | Namespaced code 与 contextual field | [Diagnostic 索引](diagnostics.md) |
| Shell status | Process exit code | [Exit code](exit-codes.md) |
| Distribution | Windows 或 Ubuntu release archive | [平台矩阵](platforms.md) |

## 稳定性模型

Software release 与 data format 各有自己的 identity。Executable 使用 semantic prerelease
version `0.1.0-alpha.1`。Machine document 与 stream 携带 `trajecta.config/v1` 或
`trajecta.cli-stream-item/v1` 一类值。Format contract 发生变化时才更新 schema version；
普通文档或实现更新不会创建新的 schema identifier。

用户与 automation 使用的稳定 integration surface 包括：

- Command path 与已经记录的 option meaning。
- Machine output envelope 与 stream record。
- Configuration、Project、Case、RunProfile 与 lock document。
- Run manifest、SQLite schema、provenance bundle 与 result-directory layout。
- Diagnostic code 与 process exit code。

Rust crate API 面向 workspace contributor，在 alpha series 中可以重新组织，同时保持 public
CLI 与 disk contract。Crate 文档入口位于 [Rust API 页面](../developer/api.md)。

## 自动生成参考

三类参考从实现重新生成：

| 页面 | 生成来源 |
| --- | --- |
| CLI 命令树 | 当前 binary `--help` 与 `M5_CLI_CONTRACT.v1.json` |
| JSON 与 JSONL schema | `testdata/` 中的 public schema 与 example file |
| Diagnostic code | Production CLI 与 job source 中带 namespace 的 diagnostic string |

文档 validation 会检查这些生成页。Command、schema 或 diagnostic 发生变化时，对应 reference
update 会与实现一起提交。

## Alpha 范围

当前 package platform、local-daemon behavior、dry-run pruning、result interface 与 future
extension point 汇总在 [Alpha 限制](alpha.md)。M5.1 期间 software version 保持
`0.1.0-alpha.1`。
