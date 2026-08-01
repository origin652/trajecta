---
title: Alpha 限制
description: Trajecta 0.1.0-alpha.1 的明确范围与兼容性限制。
---

# Alpha 限制

`0.1.0-alpha.1` 适合在保留输入和验证证据的条件下评估科研流程。该版本属于预发布，
存在以下限制：

- 只有 Windows x86_64 与 Ubuntu 24.04 x86_64 产品安装包完成正式验证。
- 本地 daemon 只开放 local IPC，尚不支持远程调度。
- `job prune` 只生成 dry-run 计划，不删除任何内容。
- 插件加载与插件 API 尚未实现。
- 当前没有通用 export 命令。结果接口为 SQLite 与产品结果命令。
- Rust crate API 面向贡献者内部，不提供产品兼容性保证。
- 配置和磁盘 schema 单独版本化。其标识发生变化时，alpha 版本可能要求迁移。
- 任务绑定到 finalize 后的 Case、Profile、DatasetLock、输入摘要和 attempt 身份。
  输入变化后需要重新 finalize 或建立新 attempt。
- Provider 下载需要网络；提供方要求认证时，凭据由用户持有。凭据不进入项目或
  provenance 文件。

科学与性能声明仅覆盖 [Validation](../validation/index.md) 中的冻结证据。发表派生结果时，
应保留原始输入、manifest、provenance 和结果目录。
