---
title: Extensibility 状态
description: 说明 Trajecta 当前的内置扩展边界，以及为未来 plugin system 预留的设计空间。
---

# Extensibility

Trajecta `0.1.0-alpha.1` 的 plugin loading **尚未实现**。当前安装包中没有 plugin manifest、
discovery directory、dynamic loader、plugin configuration section 或 plugin command。本页为后续
设计保留入口，同时说明目前在 source build 中可供 contributor 使用的边界。

## 当前产品接口

公开产品接口通过以下内容进行版本化：

- `trajecta` 命令行；
- 本机与项目配置；
- Case、RunProfile、DatasetLock、manifest 与 stream schema；
- 结果目录文件和 read-only SQLite schema；
- Stable diagnostic code 与 process exit code。

配置 reader 会拒绝未知字段。从未来设计中复制过来的 key 在当前版本会得到 schema
diagnostic，不会被静默忽略。

## 内置 registry 与 interface

Rust workspace 在产品接口之后保留了若干实现边界：

| 边界 | 当前作用 |
| --- | --- |
| Meteorological reader backend | 打开 GRIB 或 NetCDF 输入，并提供 normalized frame |
| Dataset profile catalog | 将 provider variable 和 transform 映射到 canonical field |
| Population strategy | 管理 initialization、birth、termination 与 mass accounting |
| Ozone assignment rule registry | 选择具名内置 rule 及其 field requirement |
| Integrator 与 boundary model | 推进 particle state 并处理物理边界 |
| Particle-state sink | 写入 SQLite output 与 provenance assignment |
| Job backend 与 local IPC | 持久化 queue lifecycle，并与 local daemon 通信 |

这些是面向 contributor 的 Rust interface。第三方 binary 无法仅凭这些接口与正式 Trajecta
安装包建立兼容。新的内置实现加入 workspace 后，需要连同 schema 与 scientific contract
审阅，再通过 source build 或 product build 分发。

## 当前如何扩展 source build

需要新 meteorological profile、population model 或 output sink 的 contributor，可以在 Rust
workspace 分支中开发：

1. 找到负责该功能的 crate，以及已有 trait 或 registry。
2. 使用新的稳定 model ID 加入实现。
3. 在 Case 或 Profile model 中定义配置与 validation。
4. 增加所需 meteorological capability 和 provenance identity。
5. 加入确定性 unit test 与 real-fixture integration path。
6. 新 persisted state 出现时，同步扩展 result verification。
7. 将生成的 Trajecta binary 作为独立 source revision 构建和分发。

这一途径会改变 binary，也可能改变 scientific contract。[Developer Guide](../developer/index.md)
说明 crate 职责、meteorology path、numerical runtime、test 和 release check。

当前 release 的 dataset interpretation profile 也位于 source tree。本地 profile source 可以通过
现有 RunProfile 字段描述已支持的 provider layout；reader 与 transform operation 仍来自已编译
实现。

## 为未来 plugin 预留的区域

未来 plugin 设计可以从以下区域中选择较小范围：

| 区域 | Contract 需要回答的问题 |
| --- | --- |
| Data reader | File access、field capability、topology identity、cache 与 native dependency |
| Field transform | Unit/type check、operation identity、deterministic execution 与 provenance parameter |
| Population model | Birth/termination lifecycle、carried state、mass accounting 与 verification |
| Output product | External destination、schema ownership、streaming、partial failure 与 digest participation |
| Runtime integration | Process isolation、resource accounting、cancellation、event delivery 与 recovery |

首个 plugin release 还需要 package identity、compatibility range、capability declaration、version
negotiation 和 installation workflow。Scientific extension 需要在每次运行结果中标识算法及其
validation status。

## Reproducibility 与 provenance

当前运行产品会记录 built-in model ID、dataset-profile hash、software version 和 resolved
document。Plugin system 需要提供等价的 plugin code 与 configuration identity。结果至少需要
保留：

- Plugin name、version 与 content identity；
- 兼容的 Trajecta product/schema version；
- 声明的 capability 与 model ID；
- 提供给 plugin 的 normalized configuration；
- 会影响输出的 native 或 external dependency；
- Plugin 生成 field 的 source/transform lineage；
- 中途停止时的 failure information。

因此，本页没有给出临时 manifest format。Loader、isolation 和 result identity 完成联合设计后，
再发布格式可以减少过早形成的 compatibility 约束。

## Isolation 与 credential

未来 data-provider 或 runtime integration 可能需要 credential。Plugin contract 需要定义 secret
传入方式，使其不进入 project file、command transcript、event、manifest 或 provenance bundle，
同时明确 file 与 network permission。

当前 data helper 沿用 provider official credential store，将相关值留在 Trajecta document 外。
这一约定可以作为以后设计的参考，目前不构成 plugin API。

## 保持配置可移植

现有项目可以继续使用稳定逻辑名称：

- Case 通过 dataset ID 引用 meteorology，避开 provider path；
- Machine path 与 reader selection 放在 RunProfile；
- Derived analysis product 放在 immutable run directory 之外；
- 归档结果时保留 resolved document 与 verification record。

这些做法已经属于当前产品模型，不依赖未来 plugin mechanism。

## 后续工作入口

Plugin proposal 准备成熟后，可以先通过 architecture decision 确定一种范围较窄的 extension
type，再为已经实现的 capability 增加 schema、CLI、packaging 和文档。在该 contract 发布前，
正式 Trajecta 安装使用[参考手册](../reference/index.md)列出的 built-in model。
