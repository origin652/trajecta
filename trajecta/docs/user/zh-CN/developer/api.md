---
title: Rust API
description: Trajecta workspace crate 面向贡献者的 Rust API 文档与兼容性状态。
---

# Rust API

在本地生成 crate API 文档：

```text
cargo doc --offline --no-deps --workspace
```

打开 `target/doc/trajecta_case/index.html`，并沿 workspace crate 链接浏览。Crate 顶层合同
注释说明职责与模块边界。Workspace 对公开项启用 missing-docs deny，并全局禁止 unsafe
代码。

`0.1.0-alpha.1` 的 Rust API 面向贡献者，不属于稳定自动化产品面。外部集成应使用 CLI
与公开 schema，也可以读取机器 envelope、DatasetLock、manifest 和 provenance bundle。
结果产品仅支持只读访问。
