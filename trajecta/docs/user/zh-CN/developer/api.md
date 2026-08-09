---
title: Rust API
description: 面向贡献者的 Rust 模块、主要入口、文档构建和 alpha 兼容策略。
---

# Rust API

Trajecta 的 Rust API 连接五个工作区 crate，也供直接集成测试使用。公开项带有完整文档，
跨 crate 调用在编译期进行类型检查。在 `0.1.0-alpha.1` 中，这些 API 仍属于贡献者接口；面向
用户和自动化程序使用的公开接口包括 CLI、公开文档格式、机器输出结构和结果产物。

## 构建 API 文档

为全部工作区 crate 生成 rustdoc：

```text
cargo doc --offline --locked --no-deps --workspace
```

根据工作范围打开相应入口：

| Crate | 本地 rustdoc 入口 |
| --- | --- |
| `trajecta-case` | `target/doc/trajecta_case/index.html` |
| `trajecta-met` | `target/doc/trajecta_met/index.html` |
| `trajecta-core` | `target/doc/trajecta_core/index.html` |
| `trajecta-job` | `target/doc/trajecta_job/index.html` |
| `trajecta-cli` | `target/doc/trajecta_cli/index.html` |

Windows 可通过文件资源管理器或浏览器打开文件。Ubuntu 桌面会话可以执行
`xdg-open target/doc/trajecta_core/index.html`。

工作区会拒绝缺少文档的公开项，也禁止 unsafe 代码。因此，生成 rustdoc 既是源码检查的一部分，
也是阅读 API 的入口。

## Crate 入口

### `trajecta-case`

该 crate 不需要气象读取器或任务进程。主要公开模块如下：

| 模块 | 用途 |
| --- | --- |
| `document` | 案例、运行配置、解析后的文档和元数据类型 |
| `schema` | 解析 YAML 或 JSON，并验证解析后的结构 |
| `expand` | 将本地组件参考展开成一份解析后的文档 |
| `intent` | 检查特定操作所需内容是否存在 |
| `lockfile` | 资料锁、已锁定文件、根目录、覆盖范围和能力要求标识 |
| `diagnostic` | 类型化路径、严重程度、诊断码和稳定的诊断顺序 |
| `model` | 案例文档使用的科学组件定义 |
| `quantity` | 单位注册表与规范化为 SI 的物理量 |
| `reference` 与 `resolver` | 受项目路径范围约束的本地引用和来源摘要 |

解析入口包括 `parse_case_yaml`、`parse_case_json`、`parse_run_profile_yaml` 和
`parse_run_profile_json`。展开函数接收路径和解析上下文，使诊断信息能够保留原文档位置。

该 crate 会在格式边界拒绝未知字段。下游代码应使用解析后的类型，避免再次通过字符串键访问
YAML。

### `trajecta-met`

`trajecta-met` 将已锁定来源清单转换为类型化查询输出：

| 模块 | 用途 |
| --- | --- |
| `profile` | 解析并编译数据集运行配置与计算图 |
| `io` | 检查、索引并解码 GRIB 或 NetCDF 来源 |
| `field` | 规范字段注册表、能力要求、质量检查和字段键 |
| `frame` | 原始字段、计算后的资料帧、已准备窗口和资料帧缓存 |
| `grid` | 来源原生网格落点与球面矢量插值 |
| `vertical` | 气压或混合坐标几何范围、垂直列、边界与插值模板 |
| `derive` | 有名称的气象与区域填充派生 |
| `query` | 查询计划、已准备批次、输出列、执行上下文和缓存 |
| `provenance` | 来源记录、变换过程和赋值 |
| `validation` | 容差与对比报告类型 |

`MetEngine` 持有准备状态。调用方先取得 `PreparedWindow`，使用编译后的计划准备批次，随后通过
`ExecutionContext` 和调用方持有的 `BatchWorkspace` 执行。这套顺序保证执行阶段没有隐式 I/O。

`RayonExecutionContext` 是生产环境使用的 CPU 执行策略。测试可以提供另一种执行上下文，无需改动
插值代码。

### `trajecta-core`

核心 crate 负责科学执行和结果收尾：

| 模块 | 用途 |
| --- | --- |
| `clock` 与 `lifecycle_clock` | 有符号模拟时间和主机生命周期时间戳 |
| `particle` | 使用数组结构存储的粒子批次、状态、来源和终止信息 |
| `integrator` | `IntegratorModel`、`Rk2Spherical`、带时间分组的粒子组和步进结果 |
| `boundary` | 边界路径采样、策略、判断和交点 |
| `population` 与 `release` | 释放和区域填充生命周期模型 |
| `output` | 输出计划、输出 trait、SQLite 写入端和溯源信息文件 |
| `manifest` 与 `manifest_store` | 运行 ID、生命周期记录和原子持久化 |
| `runner` | 生产构建器、`SimulationRunner`、运行器控制和 `RunOutcome` |
| `verification` | 快速或完整的结果目录验证 |

`build_runner` 构造直接运行器。`build_runner_for_attempt` 将输出绑定到持久化任务系列 ID、运行 ID
和执行轮次编号。返回的 `SimulationRunner` 提供 `run` 与 `run_with_control`；工作进程通过后者
报告进度并处理取消请求。

`verify_run_directory` 读取已有结果，不启动模拟。结果读取器与运行器构造过程彼此分离，
因此检查结果时不会修改科学状态。

### `trajecta-job`

任务 crate 是带类型的本地控制面：

| 模块 | 用途 |
| --- | --- |
| `model` | 任务状态、ID、事件、进度、资源和提交请求 |
| `backend` | CLI 客户端与守护进程访问任务存储时使用的操作接口 |
| `catalog` | 由 SQLite 支持的本地队列、状态转换、租约和恢复 |
| `scheduler` | 计入资源容量的 FIFO 与有界回填计划 |
| `daemon` | 派发循环、工作进程接口和运行器控制桥接 |
| `ipc` | 带消息大小上限的同版本本地请求与响应传输 |
| `history` | 执行轮次、完整验证记录、重跑、隐藏和清理预览计划 |

`JobBackend` 是日常控制接口，`JobHistoryBackend` 增加执行轮次操作。
`LocalJobCatalog` 实现持久化行为，`LocalJobClient` 通过 IPC 与守护进程通信。
`plan_dispatch` 保持纯函数，因此调度决策可以在不启动进程或数据库的测试中检查。

这一 trait 为后续调度器集成保留清晰的调用边界。当前发布没有动态插件接口。

### `trajecta-cli`

CLI crate 充当适配层。`main_entry` 接收 `OsString` 迭代器并返回进程退出码，使测试可以执行
完整命令调用，而不必调用 `std::process::exit`。

公开模块覆盖参数类型、解析后的命令、机器响应生成和输出格式选择。项目、运行时与结果实现
直接使用公开库约定，没有另设一套科学 API。

外部 Rust 程序若需要自动化 Trajecta，可直接执行产品二进制，选择 JSON 或 JSONL 输出，再按
公开 schema 校验。CLI 内部模块会随实现调整，不适合作为绕过文档或任务验证的入口。

## 错误与诊断约定

库错误保留足够的结构化信息，使适配层可以选择公开诊断码。配置诊断还会携带可排序的字段路径和
严重程度，因此一次检查可以返回多个文档问题。

跨越 crate 边界时遵循以下约定：

- 可恢复输入、I/O、资源和生命周期失败使用 `Result`；
- 持久化前验证完整公开类型；
- 机器诊断码保持稳定，可读消息不包含凭据；
- 来源或路径上下文有助于定位字段时予以保留；
- 数值层完成分类前，不把类型化气象样本状态提前压成普通字符串。

工作区静态检查会在生产代码中拒绝 `panic!`、`unwrap`、`expect`、`todo!` 和
`unimplemented!`。测试可以在准备固定测试资料和编写断言时局部放宽规则。

## 序列化边界

`serde` 用于转换 Rust 类型与文档，但派生得到的序列化实现只是公开格式的一部分。公开的磁盘
文档和数据流格式还包括：

- 文档内部的 schema 标识符；
- `testdata` 下的 JSON Schema 或冻结 SQL schema；
- 有效示例；
- 写入端与读取端测试；
- 计算内容散列时采用的稳定排序或规范化规则。

未知字段的处理策略在格式边界决定。大多数配置和控制面记录使用
`deny_unknown_fields`，使拼写错误及时显现。正向兼容性应通过明确的格式设计引入，
不由某一个读取器任意忽略字段。

## Trait 接口边界

以下 trait 用于隔离策略和平台相关代码：

| Trait | 边界 |
| --- | --- |
| `DataProvider` | 定位已经声明的本地数据集根目录 |
| `GridBackend` | 在来源原生网格中计算水平落点 |
| `ExecutionContext` | 控制已准备查询所用的工作线程数 |
| `IntegratorModel` | 在应用边界规则前确定性地推进粒子 |
| `BoundaryPolicy` | 根据物理边界对有序路径进行分类 |
| `PopulationModel` | 管理公共平流步骤前后的粒子群操作 |
| `OutputProduct` 与 `ParticleStateSink` | 写入计划科学输出及其存储介质 |
| `RunnerControl` | 在宏步边界报告进度并处理取消请求 |
| `JobBackend` | 提供客户端可见的本地任务操作 |
| `WorkerProcessManager` | 统一处理各平台的进程启动与强制停止行为 |

这些 trait 便于编写范围明确的测试，也使依赖方向保持清晰。具体实现仍需满足公开结果格式
和生命周期规则；实现一个公开 trait 并不会自动注册新的扩展。

## Alpha 阶段的兼容性

兼容性分为两组：

| 接口层 | `0.1.0-alpha.1` 中的处理 |
| --- | --- |
| CLI 语法、机器输出结构、配置、项目/案例/运行配置文档、资料锁、运行清单、溯源信息、SQLite、软件包清单 | 公开接口；修改时需要同步 schema、示例、测试、文档和兼容性说明 |
| 公开 Rust 模块、trait、构造函数和错误枚举 | 贡献者接口；Alpha 期间可随内部实现完善而调整 |

将来若发布供第三方嵌入使用的 crate，还需另行制定稳定性策略，发布相应 crate 软件包，说明
依赖版本范围，并建立集成兼容性测试。当前版本尚未开放这种使用方式。

## 增加或修改公开项

1. 将规则放在拥有其含义的 crate。
2. 在 rustdoc 中说明输入、输出、失败情形和生命周期影响。
3. 优先复用现有公开类型，不另建一套平行的字符串或映射表示。
4. 增加范围明确的单元测试；必要时补充经过公开边界的集成测试。
5. 运行 `cargo doc`，并以拒绝警告的方式运行 Clippy。
6. 按依赖顺序更新下游 crate。
7. 若该项改变公开结果格式，同步结构定义和用户参考手册。
8. 运行该边界对应的真实资料、运行时或软件包矩阵。

每个 `src/lib.rs` 顶部的 crate 级文档都会概述模块职责。阅读单个 rustdoc 类型前，可以先从
相应 crate 的首页开始。
