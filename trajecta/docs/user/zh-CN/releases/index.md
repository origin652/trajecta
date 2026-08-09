---
title: 发布策略
description: Trajecta 软件、schema、资料 asset、文档快照和 alpha 兼容性的版本规则。
---

# 发布策略

Trajecta 发布记录将一个软件版本与对应 package、公开格式、文档和已知限制关联。当前处于 alpha
阶段，因此每个版本页还会区分适合 automation 的产品接口，以及仍可能调整的部分。

## 版本标识

一个项目中会同时出现多种 identifier，它们分别在不同条件下更新：

| 标识 | 示例 | 更新条件 |
| --- | --- | --- |
| 软件版本 | `0.1.0-alpha.1` | 准备新的 executable release |
| Git tag | `v0.1.0-alpha.1` | 选定该软件 release 的 source commit |
| 文档 schema | `trajecta.config/v1` | 对应 disk 或 stream format 需要新合同 |
| SQLite user version | `1` | Result database schema 发生不兼容修改 |
| DatasetLock schema | Lock 内的版本字段 | Dataset identity 或 lock semantics 改变 |
| 演示资料 asset | `trajecta-demo-cfsr-20090101-v1` | Sample file set、metadata 或 packaging contract 改变 |
| 文档版本 | `0.1.0-alpha.1` 或 `dev` | 发布软件 snapshot，或者已接收文档继续更新 |

软件 release 不会自动增加全部 schema。Automation 在读取 command-specific field 前，应先查看
对应格式自己的 `schema_version`。

## 发布 channel

| Channel | URL 行为 | 内容 |
| --- | --- | --- |
| 具名 release | 不可变 version path；当前默认版本为 `0.1.0-alpha.1` | 与已发布软件版本对应的手册 |
| `latest` alias | 指向当前具名 release | 稳定文档入口 |
| `dev` | 可变 `/dev/` path，并设置 search-engine `noindex` | 最新 snapshot 之后已经接收的文档修改 |

普通文档修订更新 `dev`。Release event 会在相同版本的 source、package 和 documentation gate 通过后
建立具名 snapshot。

## Release 内容

完整 release entry 提供：

- Windows x86_64 与 Ubuntu 24.04 x86_64 产品 archive；
- 每个 archive 相邻的 SHA-256 文件；
- 单独下载的演示资料与 checksum；
- 说明主要工作流和已知限制的 release note；
- 带版本的双语手册；
- 软件包内部的 build manifest、SBOM 和 license inventory；
- 通过手册 Validation 章节发布的验证 data 与 chart。

Archive name、native component version 和 reader support matrix 见
[平台与 reader 矩阵](../reference/platforms.md)。

## Alpha 系列兼容性

CLI、公开配置、schema、machine envelope 和 result artifact 构成产品边界。修改这些 surface 时，
会同时更新 example、validation 和 release note。

稳定 1.0 合同建立前，alpha release 可能调整 default、field 或 behavior。需要重复运行或审计的项目
可以保留以下内容：

- Product archive 或其精确 checksum；
- Resolved Case 与 RunProfile document；
- DatasetLock 与 source file hash；
- Terminal run manifest 与 provenance bundle；
- 本次运行使用的 release documentation snapshot。

Public Rust module 在 alpha 期间属于贡献者接口，源码兼容范围见 [Rust API](../developer/api.md)。

## 文档修订

不可变 snapshot 保存 release 随附的原始手册。小型澄清先进入 `dev`。修订如果涉及命令、格式、
科学解释或安全操作，release note 会记录范围，并在下一份 snapshot 发布前链接到修订后的 dev
page。

Raw validation file 与 package checksum 保持原始 identity。只调整呈现方式时，chart 从同一份
冻结 input 重新生成。

## 阅读版本页

每个具名页面采用相同顺序：

1. Release status 与支持 package；
2. 科学工作流与资料家族；
3. Runtime 与 result capability；
4. 文档与 demonstration data；
5. Compatibility note 与 known limit；
6. Installation、quickstart 和精确 reference link。

首个发布条目为 [0.1.0-alpha.1](0.1.0-alpha.1.md)。
