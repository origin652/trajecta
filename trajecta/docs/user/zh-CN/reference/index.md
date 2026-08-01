---
title: 产品参考
description: Trajecta 0.1.0-alpha.1 命令、配置、文档、schema、诊断、结果与支持平台的规范参考。
---

# 产品参考

本区说明 `0.1.0-alpha.1` 的稳定产品面：

- `trajecta` 命令行接口；
- 本机配置与项目文档；
- JSON、JSONL、manifest、provenance 和结果产物；
- diagnostic code 与进程退出码；
- 正式安装包平台和气象资料 reader。

CLI、schema 和磁盘产物是用户与自动化程序的集成边界。Rust crate API 面向贡献者，
在 alpha 阶段可能变化。只有开发 Trajecta 本身时才需要查阅
[Rust API](../developer/api.md)。

自动生成页面会在页首标注来源。二进制帮助、schema 或生产 diagnostic 发生变化后，
如未同步重建参考页，CI 将直接失败。
