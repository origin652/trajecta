---
title: 文档贡献规范
description: Trajecta 文档的源码布局、双语策略、可执行片段、自动参考页、风格、SEO 和本地检查。
---

# 文档贡献规范

用户文档位于 `docs/user`。英文为规范源，中文使用相同相对路径与标题层级。工程计划和
交付报告位于 `docs/engineering`，不进入站点导航、搜索或 sitemap。

## 内容规则

- 正文使用 Markdown，并复用 `mkdocs.yml` 已启用的 MkDocs Material 组件。
- `examples/` 下的示例项目是配置真相。文档应引入这些文件的片段，避免维护第二份 YAML。
- 使用 `tools/generate_m5_1_reference.py` 重建 CLI、schema 和 diagnostic 页面。
- 只描述已实现命令，以及有门禁证据的平台声明。
- 对未来插件与导出构想明确标记为尚不可用。
- 每个发布页面提供独立 title 与 description。
- 比较术语和相关图表只能位于 Validation 区域。

中文采用偏学术手册的语气。先定义前提与适用范围，再给步骤。一个句子含有多个独立条件
时，优先改为表格或列表。

## 本地检查

```text
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_docs.py
mkdocs build --strict
```

Windows 使用 `target/debug/trajecta-cli.exe`。外部链接在 CI 中带重试检查；本地 strict
构建不需要 provider 凭据。
