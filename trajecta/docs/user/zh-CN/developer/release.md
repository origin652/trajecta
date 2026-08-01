---
title: 打包与发布
description: 确定性产品归档、native 运行时清单、发布证据、版本策略和 GitHub 发布流程。
---

# 打包与发布

正式产品归档在 Windows x86_64 与 Ubuntu 24.04 x86_64 上构建。每份归档包含二进制、
native 库及 ecCodes definitions，也包含示例、离线恢复文档和资料助手。许可材料、SBOM
与 `BUILD-MANIFEST.json` 随归档提供。

打包器使用固定源码身份与源码时间。归档路径、权限与时间戳保持确定性，payload 排序和
摘要也保持确定性。安装包验证会检查相邻 SHA-256、manifest 结构和每项 payload 摘要，
随后核对二进制身份、供应链文件与 native 组件版本。

## 发布顺序

1. 通过 workspace、合同、文档和产品安装包门禁。
2. 在干净解压目录中构建并验证两个正式平台归档。
3. 在两个平台运行 domain-fill quickstart 与产品 smoke。
4. 构建演示资料 asset，并校验其冻结 manifest。
5. 使用 `mike` 建立不可变文档快照。
6. 发布软件归档、checksum、资料 asset 与 release notes。
7. 检查公开 Pages release URL、sitemap、canonical 元数据和语言切换。

M5.1 保持软件版本 `0.1.0-alpha.1`。普通文档提交只更新 `dev` 站点，不建立新发布版本。
Provider 条款或 GitHub 登录尚未解决时，不能继续正式发布。
