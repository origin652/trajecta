---
title: 开发架构
description: Trajecta workspace 边界、依赖方向和贡献者接口。
---

# 开发架构

Workspace 按职责拆分。依赖从文档模型流向气象资料、数值执行、任务控制和 CLI adapter。

| Crate | 负责 | 不应负责 |
| --- | --- | --- |
| `trajecta-case` | Case、RunProfile、单位、引用、诊断和 DatasetLock 结构 | 气象解码或执行 |
| `trajecta-met` | Profile、文件索引、原生网格、派生、插值、预备查询和气象 provenance | 粒子生命周期或输出调度 |
| `trajecta-core` | 时钟、粒子、积分、边界、population、输出、manifest 和验证 | Provider 下载或 CLI 渲染 |
| `trajecta-job` | 持久化本地队列、调度器、attempt、IPC、daemon 和 worker 合同 | 科学数值算法 |
| `trajecta-cli` | 参数、文档加载、调度、机器 envelope 和结果产品视图 | 重复科学或调度逻辑 |

产品边界由 CLI、公开文档 schema、机器流和磁盘产物组成。公开 Rust 模块用于在 workspace
crate 之间落实类型合同；alpha 阶段将其视为贡献者接口。

## 端到端职责

1. `trajecta-case` 展开并校验不可变输入文档。
2. `trajecta-met` 索引 lock 约束的气象资料，并准备内存查询数据。
3. `trajecta-core` 执行 population 并提交结果产物。
4. `trajecta-job` 在 worker 外围持有队列状态与 attempt 身份。
5. `trajecta-cli` 对外提供生命周期操作，不改变其语义。

未来扩展可以使用这些边界。动态插件加载尚未实现，详见
[扩展性](../concepts/extensibility.md)。
