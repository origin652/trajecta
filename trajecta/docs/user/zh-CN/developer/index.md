---
title: 开发架构
description: Trajecta workspace 的目录职责、依赖方向、修改入口和贡献者接口。
---

# 开发架构

Trajecta 的 Rust workspace 包含五个产品 crate。目录划分遵循一次模拟的处理顺序：先解析文档，
再准备气象资料，随后推进粒子并写出结果，任务系统负责调度，命令行负责呈现。沿着这条链路定位
修改，可以较快找到规则的归属位置。

本节面向参与 Rust 实现、测试设施、打包工具或文档维护的贡献者。已经承诺给使用者的产品接口
集中列在[参考手册](../reference/index.md)。

## 仓库目录

| 路径 | 负责内容 | 常见修改 |
| --- | --- | --- |
| `crates/trajecta-case` | Case 与 RunProfile 文档、单位、本地引用、诊断和 DatasetLock 类型 | 新增可验证字段、引用规则或文档诊断 |
| `crates/trajecta-met` | Dataset Profile、源文件读取、原生网格、垂直坐标、字段推导、插值和查询准备 | 新读取器、推导字段、垂直坐标规则或查询优化 |
| `crates/trajecta-core` | 模拟时钟、粒子状态、RK2 积分、边界、population、输出、manifest 和结果验证 | 数值规则、生命周期、输出记录或科学检查 |
| `crates/trajecta-job` | 持久化本地 catalog、调度器、attempt、事件历史、IPC、daemon 与 worker 控制 | 队列转换、恢复规则、资源决策或任务事件 |
| `crates/trajecta-cli` | 命令解析、项目组装、调用分发、机器输出 envelope 和结果视图 | 新命令、渲染器、项目操作或退出码映射 |
| `testdata` | 公开 schema、冻结合同、小型 fixture 和验证输入 | 与 schema 一致的示例或带版本的合同 fixture |
| `examples` | 教程及自动快速入门使用的完整项目 | 可执行工作流或样例配置 |
| `tools` | 打包、参考页生成、资料获取和验证程序 | 发布门禁或可重复维护操作 |
| `docs/user` | 公开双语手册 | 用户、运维和开发文档 |
| `docs/engineering` | 计划、Prompt、执行报告和历史设计资料 | 工程记录；不会进入公开站点 |

Workspace 的依赖方向如下：

```text
trajecta-case
    └── trajecta-met
            └── trajecta-core
                    └── trajecta-job

trajecta-cli 依赖四个产品库。
trajecta-core 还会直接使用 trajecta-case 的文档类型。
trajecta-job 会使用 trajecta-case 中已经验证的输入类型。
```

底层 crate 不会反向调用 CLI。数值执行也不依赖 daemon，因此直接集成测试和 daemon 启动的
worker 可以共用同一条 `trajecta-core` 执行路径。

## 从命令到结果

项目运行按以下顺序经过 workspace：

1. `trajecta-cli` 查找 `trajecta-project.yaml`，选择具名 Profile，并在项目根目录内解析相对路径。
2. `trajecta-case` 解析 Case 和 RunProfile。解析过程会拒绝未知字段，展开组件引用，规范化单位，
   最后返回顺序稳定的诊断。
3. `project finalize` 将解析后的文档绑定到 DatasetLock。Lock 记录本次运行使用的文件、哈希、
   覆盖范围、能力集合和资料集身份。
4. `trajecta-job` 把请求写入本地 catalog。调度器预留 CPU slot 与内存，分配 attempt 身份，
   然后启动 worker。
5. Worker 调用 `trajecta-met` 建立锁定资料的索引，生成 canonical field，选择所需 frame window，
   并准备查询结构。
6. `trajecta-core` 创建 population，按照带符号的 macro step 推进。边界处理、粒子出生、终止和
   定时输出遵循稳定的生命周期顺序。
7. 输出事务关闭 SQLite，写入 provenance，并用终态 manifest 替换 running manifest。Job catalog
   记录相同的终态，同时追加最后一条事件。
8. 结果命令读取公开 manifest 和 SQLite 文件，不会再次启动数值计算。

[气象资料与数值路径](science-path.md)详细展开第 3 至第 6 步。[输出与运行时](runtime.md)
说明任务入队、执行和终态恢复。

## 修改应从哪里开始

| 修改目标 | 首要入口 | 后续检查 |
| --- | --- | --- |
| 增加 Case 字段 | `trajecta-case` 的 model 与 schema | 展开逻辑、示例、自动生成的 schema 参考页 |
| 增加资料家族 | `trajecta-met` 的 Profile、inventory 与 reader | Lock capability、真实 fixture、软件包矩阵 |
| 修改插值 | `trajecta-met/query` 或 `trajecta-met/grid` | 科学容差、确定性、性能 |
| 修改粒子运动 | `trajecta-core/integrator` | 正反向 replay、边界、输出身份 |
| 修改 population | `trajecta-core/population`，气象推导放在 `trajecta-met` | 质量账本、出生、稳定 ID、教程 |
| 增加结果字段 | `trajecta-core/output` 与 manifest | SQLite schema、验证、result inspect、参考页 |
| 修改队列行为 | `trajecta-job` | Catalog 事务、IPC、CLI 事件流、恢复测试 |
| 增加命令 | `trajecta-cli` | CLI 合同、JSON/JSONL envelope、退出码、自动参考页 |
| 修改软件包内容 | `tools/m5_a4_package.py` | Build manifest、SBOM、全新解压、两个平台 |
| 修改公开文档 | `docs/user/en` 与 `docs/user/zh-CN` | 严格构建、链接、双语结构、dev/release SEO |

一项功能可能跨过表中的多行。新规则应放在拥有该含义的层中，后续 adapter 直接调用它，避免在
命令行、worker 或测试工具里另写一份实现。

## 跨 crate 的合同

下列对象在多个层之间传递，审查时需要同时查看生产方和使用方：

| 合同 | 生产方 | 主要使用方 | 兼容性重点 |
| --- | --- | --- | --- |
| 已解析的 Case 与 RunProfile | `trajecta-case` | CLI、core runner | 字段含义、默认值、规范化单位 |
| DatasetLock | 项目 finalize | met loader、manifest builder | 文件身份、时空覆盖、capability |
| Prepared query 与 typed sample status | `trajecta-met` | 积分器、边界采样器、输出 | Execute 阶段无隐式文件 I/O；无效状态明确可见 |
| Run manifest | `trajecta-core` | 任务恢复、verify、结果命令 | 生命周期、输入身份、行数、provenance |
| Job record 与 event | `trajecta-job` | CLI status、wait、events、history | 持久化顺序、attempt 身份、终态语义 |
| CLI envelope | `trajecta-cli` | Shell 脚本、AI 工具、CI | 完整命令路径、诊断结构、stdout 纪律 |

`testdata` 中的公开 schema 描述磁盘格式和机器输出。Rust 类型是 workspace 内使用的编译期对应物。
Alpha 阶段仍可能随着产品合同调整公开 Rust item，具体范围见 [Rust API](api.md)。

## 常用贡献流程

多数修改可以沿用以下工作顺序：

1. 阅读目标 crate 的 `src/lib.rs` 顶部合同，以及离修改位置最近的 module 文档。
2. 编辑期间持续运行范围最小的单元测试或集成测试。
3. 检查受到影响的公开格式，包括 schema、example、diagnostic、CLI envelope、manifest 和 SQLite row。
4. 运行[测试与 fixture](testing.md)列出的格式化、lint、workspace test 和文档门禁。
5. 修改涉及 reader、数值路径、任务恢复或原生库时，再运行对应的真实资料或软件包门禁。
6. 使用者可见行为发生变化时，同步更新两个语言目录。

`0.1.0-alpha.1` 尚未提供动态插件加载。现有 crate 与 trait 边界可以支持后续设计，但当前不会
对外呈现扩展 API。状态说明见[可扩展性](../concepts/extensibility.md)。
