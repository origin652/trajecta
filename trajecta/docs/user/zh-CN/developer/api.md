---
title: Rust API
description: 面向贡献者的 Rust module、主要入口、文档构建和 alpha 兼容策略。
---

# Rust API

Trajecta 的 Rust API 连接五个 workspace crate，也用于直接 integration test。Public item 有完整
文档，并通过 crate boundary 进行类型检查。在 `0.1.0-alpha.1` 中，它们仍属于贡献者接口；稳定
automation surface 由 CLI、公开 document schema、machine envelope 和 result artifact 组成。

## 构建 API 文档

为全部 workspace crate 生成 rustdoc：

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

Windows 可通过 Explorer 或 browser 打开文件。Ubuntu desktop session 可以执行
`xdg-open target/doc/trajecta_core/index.html`。

Workspace 会拒绝 public item 缺少文档，也禁止 unsafe code。因此 rustdoc generation 同时是 source
gate 和阅读入口。

## Crate 入口

### `trajecta-case`

该 crate 不需要 meteorological reader 或 runtime process。主要 public area 为：

| Module | 用途 |
| --- | --- |
| `document` | Case、RunProfile、resolved document 和 metadata type |
| `schema` | 解析 YAML/JSON，并验证 resolved shape |
| `expand` | 将 local component reference 展开成一份 resolved document |
| `intent` | 检查特定 operation 所需内容是否存在 |
| `lockfile` | DatasetLock、locked file、root、coverage 和 capability identity |
| `diagnostic` | Typed path、severity、code 和稳定 diagnostic ordering |
| `model` | Case document 使用的 scientific component specification |
| `quantity` | Unit registry 与 SI-normalized quantity |
| `reference` 与 `resolver` | 受 containment 约束的 local reference 和 source digest |

Parser 入口包括 `parse_case_yaml`、`parse_case_json`、`parse_run_profile_yaml` 和
`parse_run_profile_json`。Expansion function 接收 path 与 resolver context，使 diagnostic 能保留
document location。

该 crate 会在 schema boundary 拒绝 unknown field。Downstream code 应使用 resolved type，不应重复
通过字符串访问 YAML。

### `trajecta-met`

`trajecta-met` 将 locked source inventory 转换为 typed query output：

| Module | 用途 |
| --- | --- |
| `profile` | 解析并编译 Dataset Profile 与 computation graph |
| `io` | Inspect、index 与 decode GRIB/NetCDF source |
| `field` | Canonical field registry、capability、quality 和 field key |
| `frame` | Raw field、computed frame、prepared window 和 frame cache |
| `grid` | Source-native grid placement 与 spherical vector interpolation |
| `vertical` | Pressure/hybrid geometry、column、bound 和 stencil |
| `derive` | 有名称的 meteorological 与 domain-fill derivation |
| `query` | Query plan、prepared batch、output column、execution context 和 cache |
| `provenance` | Source record、transformation 和 assignment |
| `validation` | Tolerance 与 comparison report type |

`MetEngine` 拥有 preparation state。Caller 先取得 `PreparedWindow`，使用 compiled plan 准备 batch，
随后通过 `ExecutionContext` 与 caller-owned `BatchWorkspace` 执行。该顺序属于 no-hidden-I/O contract。

`RayonExecutionContext` 是 production CPU policy。Test 可以提供另一种 execution context，无需改动
interpolation code。

### `trajecta-core`

Core crate 负责科学执行和结果收尾：

| Module | 用途 |
| --- | --- |
| `clock` 与 `lifecycle_clock` | Signed simulation time 和 host lifecycle timestamp |
| `particle` | Structure-of-arrays particle batch、status、origin 和 termination |
| `integrator` | `IntegratorModel`、`Rk2Spherical`、timed cohort 和 step result |
| `boundary` | Boundary path sampling、policy、decision 和 intersection |
| `population` 与 `release` | Release 和 domain-fill lifecycle model |
| `output` | Schedule、product trait、SQLite sink 和 provenance bundle |
| `manifest` 与 `manifest_store` | Run identity、lifecycle record 和 atomic persistence |
| `runner` | Production builder、`SimulationRunner`、runner control 和 `RunOutcome` |
| `verification` | Quick/full result-directory verification |

`build_runner` 构造直接运行。`build_runner_for_attempt` 将 output 绑定到 durable job-series ID、run ID
和 attempt number。返回的 `SimulationRunner` 提供 `run` 与 `run_with_control`，后者供 worker
报告 progress 和处理 cancellation。

`verify_run_directory` 读取已有产品，不启动 simulation。Result reader 与 runner construction 保持
分离，可以避免 inspect 修改科学状态。

### `trajecta-job`

Job crate 是带类型的本地 control plane：

| Module | 用途 |
| --- | --- |
| `model` | Job state、identity、event、progress、resource 和 submit request |
| `backend` | CLI client 与 daemon implementation 使用的 backend operation |
| `catalog` | SQLite-backed local queue、transition、lease 和 recovery |
| `scheduler` | 纯 resource-accounted FIFO 与 bounded backfill plan |
| `daemon` | Dispatch cycle、worker process trait 和 runner-control bridge |
| `ipc` | 有 frame 上限的 same-release local request/response transport |
| `history` | Attempt、full-verification record、rerun、forget 和 prune plan |

`JobBackend` 是日常 control interface，`JobHistoryBackend` 增加 attempt operation。
`LocalJobCatalog` 实现 durable behavior，`LocalJobClient` 通过 IPC 与 daemon 通信。
`plan_dispatch` 保持纯函数，因此 scheduler decision 可在没有进程或 database 的 test 中检查。

Backend trait 为后续 scheduler integration 保留边界。当前 release 没有动态 plugin interface。

### `trajecta-cli`

CLI crate 是 adapter。`main_entry` 接收 `OsString` iterator 并返回 process exit code，使完整 invocation
可以在 test 中执行，无需调用 `std::process::exit`。

Public module 覆盖 argument type、parsed command、envelope rendering 和 output selection。Project、
runtime 与 result implementation 使用 public library contract，没有另设 scientific API。

外部 Rust application 不宜调用 CLI internal 来绕过 document 或 job validation。Automation 可执行
产品 binary，选择 JSON 或 JSONL 输出，再按公开 schema 校验。

## Error 与 diagnostic 约定

Library error 保留足够结构，使 adapter 可以选择 public diagnostic code。Configuration diagnostic
还会携带可排序 path 与 severity，因此一次能够返回多个 document problem。

跨越 crate boundary 时遵循以下约定：

- 可恢复 input、I/O、resource 和 lifecycle failure 使用 `Result`；
- 持久化前验证完整 public type；
- Machine code 保持稳定，human message 不包含 secret；
- Source 或 path context 有助于定位字段时予以保留；
- Numerical layer 完成分类前，不把 typed meteorological sample status 提前压成 generic string。

Workspace lint 会在 production code 中拒绝 `panic!`、`unwrap`、`expect`、`todo!` 和
`unimplemented!`。Test 可以为 fixture setup 与 assertion 使用局部 allowance。

## Serialization boundary

`serde` 用于对应 Rust type 与 document，但 derived serializer 只构成公开格式的一部分。Public
disk/stream type 还包括：

- 文档内部的 schema identifier；
- `testdata` 下的 JSON Schema 或 frozen SQL schema；
- Valid example；
- Producer 与 consumer test；
- 身份需要 hash 时的 deterministic ordering 或 canonicalization rule。

Unknown-field policy 在 schema boundary 决定。大多数 configuration 与 control-plane record 使用
`deny_unknown_fields`，使拼写错误及时显现。Forward compatibility 应通过显式 schema decision 引入，
不由某一个 reader 任意忽略 field。

## Trait boundary

以下 trait 用于隔离 policy 或 platform code：

| Trait | 边界 |
| --- | --- |
| `DataProvider` | 定位已经声明的 local dataset root |
| `GridBackend` | Source-native horizontal placement |
| `ExecutionContext` | Prepared query execution 的 worker-count policy |
| `IntegratorModel` | Boundary handling 前的 deterministic particle advance |
| `BoundaryPolicy` | 根据 physical boundary 分类 ordered path |
| `PopulationModel` | Shared advection step 前后的 population work |
| `OutputProduct` 与 `ParticleStateSink` | Scheduled scientific output 与 storage |
| `RunnerControl` | Macro-step boundary 上的 progress 和 cancellation |
| `JobBackend` | Client-visible local job operation |
| `WorkerProcessManager` | 平台进程启动和 force-stop behavior |

Trait 使 focused test 更容易编写，也保持 dependency direction 清晰。Implementation 仍需满足 product
schema 与 lifecycle rule；实现 public trait 本身不会注册产品 extension。

## Alpha 阶段的兼容性

兼容性分为两组：

| Surface | `0.1.0-alpha.1` 中的处理 |
| --- | --- |
| CLI syntax、machine envelope、configuration、Project/Case/Profile document、DatasetLock、manifest、provenance、SQLite、package manifest | 产品合同；修改时需要 schema、example、test、docs 和 compatibility decision |
| Public Rust module、trait、constructor 和 error enum | 贡献者合同；alpha 期间可随产品实现完善而调整 |

将来如果发布供第三方 embedding 的 crate，还需制定明确 stability policy，发布 crate package，提供
dependency-version guidance，并建立 integration compatibility suite。当前 release 暂无这些承诺。

## 增加或修改 public item

1. 将规则放在拥有其含义的 crate。
2. Rustdoc 说明 input、output、failure case 和 lifecycle effect。
3. 优先复用现有 public type，不另建平行 string 或 map representation。
4. 增加 focused unit test；必要时增加 public-boundary integration test。
5. 运行 `cargo doc` 和 warnings-denied clippy。
6. 按 dependency order 更新 downstream crate。
7. Item 改变 product format 时，同步 schema 和 user reference。
8. 运行该 boundary 对应的 real-data、runtime 或 package matrix。

每个 `src/lib.rs` 顶部的 crate-level comment 都会概述 ownership。阅读单个 rustdoc type 前，可先从
这里开始。
