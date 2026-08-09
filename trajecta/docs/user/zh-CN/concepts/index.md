---
title: Project、Case、RunProfile 与 DatasetLock
description: 了解 Trajecta 如何分别表示研究项目、科研意图、本机执行选择和确切气象输入。
---

# Project、Case、RunProfile 与 DatasetLock

Trajecta 使用四类相互关联的对象描述一次运行。这样的分层让科研设计保持稳定，同时可以为
具体计算机选择资料位置和执行资源。

| 对象 | 回答的问题 | 主要内容 |
| --- | --- | --- |
| Project | 哪些实验属于同一项研究？ | Case 与 Profile 的名称和路径；dataset-profile 映射；默认 Profile |
| Case | 要进行怎样的物理模拟？ | 时间、方向、气象域、population、substance、数值模型、边界与输出 |
| RunProfile | 这个 Case 在本机如何运行？ | Case 路径、输出根目录、资料根与 lock、reader backend、worker 和内存预算 |
| DatasetLock | 本次接收哪些确切气象输入？ | 文件路径、字节数、hash、有效时间、网格签名、垂直坐标签名和 interpretation-profile 身份 |

命令行选择一个 Project 和其中一个具名 Profile。该 Profile 再选择一个 Case，两者共同解析为
worker 使用的一对不可变输入。

## Project：研究容器

项目根目录包含 `trajecta-project.yaml` 和项目相对文件。一个简洁的索引如下：

```yaml
schema_version: trajecta.project-index/v1
name: domain-fill-cfsr
cases:
  moisture: cases/moisture.yaml
profiles:
  product:
    path: profiles/product.yaml
    dataset_profiles:
      cfsr: cfsr-pgbl-pressure-v0
default_profile: product
```

左侧名称是项目内部标识。Case 引用逻辑资料 `cfsr`；所选项目 Profile 将其映射到内置
interpretation profile `cfsr-pgbl-pressure-v0`。

一个项目可以包含多个 Case 和多个 Profile。常见组织方式包括：

- 一个 Case 配置几种 Profile，用于不同 worker 数量或 reader；
- Forward 与 backward Case 共享同一套气象资料；
- 多个季节 Case 使用等价的 Profile entry；
- 在正式配置旁保留一个小规模本机检查 Profile。

每个 Profile entry 仍指向一份 RunProfile 文档，其中的 `case_path` 选择一个已经进入索引的
Case。两个 Case 使用相同执行设置时，可以创建两个具名 Profile entry，各自选择相应 Case。
这样每条提交命令都有明确含义。

## Case：可移植的科研意图

Case 表达会影响物理轨迹计算的选择。例如：

```yaml
schema_version: 0
kind: case
metadata: { name: domain-fill-cfsr }
time:
  start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1230790200, nanosecond: 0 }
  direction: forward
meteorology:
  domains:
    - id: global
      dataset: cfsr
      priority: 1
      horizontal_halo_cells: 1
particle_population:
  strategy: domain_fill_air_mass
  id: moisture-air
  domain_id: global
  target_particle_count: 1000
substances: []
numerics:
  time_step: { value: 300, unit: s }
  integrator: { model: rk2_spherical/v0 }
  boundaries:
    policies: [surface_reflect/v0, model_top_terminate/v0, global_periodic/v0]
  random_seed: 4202
outputs:
  - product: particle_state/v1
    schedule: { mode: endpoints }
    sink: { model: particle_state_sqlite/v1 }
```

Case 使用逻辑 dataset name，不保存 provider credential。时间戳采用 Unix second 加 nanosecond，
表示确切 UTC 时刻。Direction 控制从 `start` 向 `end` 的物理积分方向；资料覆盖计算会把两个
时刻按实际先后排序。

较大的 domain、population 或 output 片段可以放入独立项目文件，再由组件引用组合。`case
resolve` 展开这些引用，并输出后续检查所使用的规范化 Case。

## RunProfile：本机绑定与资源

RunProfile 把 Case 连接到路径和执行设置：

```yaml
schema_version: 0
kind: run_profile
metadata: { name: product }
case_path: ../cases/moisture.yaml
output_root: ../runs
datasets:
  - dataset: cfsr
    lockfile: ../locks/cfsr.lock.json
    data_roots: { met: ../data }
    reader_backend: rust
execution:
  worker_threads: 1
  memory_budget_bytes: 1073741824
  executor: cpu
  meteorology_reader: rust
```

路径相对于 RunProfile 文件解析，并保留在项目根目录内。`data_roots` 为 DatasetLock 使用的各
root ID 指定本地目录。同一个逻辑 Case 可以选择另一份 Profile，从不同存储布局读取资料。

`worker_threads` 与 `memory_budget_bytes` 会成为队列资源请求。Reader 设置选择已经实现的
气象读取后端。运行结束后，这些内容随 resolved Profile 保存在结果目录。

## DatasetLock：确切资料清单

Finalize 检查本地文件后生成 DatasetLock。它将逻辑 dataset 和 interpretation profile 绑定
到以下内容：

- 每个所选文件的 root ID 和相对路径；
- 精确字节长度与 SHA-256；
- 物理有效时间和文件 role；
- 规范化水平网格签名；
- pressure-level 或 hybrid-coordinate 签名；
- 规范化 dataset-profile SHA-256；
- 生成工具名称与版本。

Lock 中保存相对文件位置。RunProfile 提供实际 root directory，因此同一 lock 结构可以在
Windows 和 Ubuntu 上解析，只需把相同内容放入各自项目根目录的对应位置。

Finalize 会选择覆盖 Case 时段及其时间插值邻帧的资料，并检查 dataset profile 是否提供所选
population 需要的 capability。

## 三种项目状态

`project status` 派生以下状态：

| 状态 | 含义 | 后续工作 |
| --- | --- | --- |
| `draft` | 可识别的 Case 或 Profile 仍缺少必填字段 | 继续渐进编辑与校验 |
| `configured` | 逻辑文档完整，资料或 lock 尚未到齐 | 生成 data-plan 并准备气象资料 |
| `finalized` | 文档、路径、资料文件、profile identity 与 lock 相互一致 | 运行 `doctor --deep` 并提交任务 |

任一状态都可能带有 error，例如索引路径无效或文档格式损坏。读取状态名称时，同时查看其
diagnostic。

## Validation、resolution 与 finalization

三类操作处理不同层次：

1. **Validation** 检查文档形状、字段类型、引用和所选命令 intent。
2. **Resolution** 将本地组件引用展开为规范化 Case 或 RunProfile。
3. **Finalization** 打开所选资料，检查覆盖与 capability，随后创建或校验 DatasetLock。

资料延后到达的项目可以先完成 Case 与 RunProfile 审阅，保持在 `configured`。文件身份在
finalize 阶段加入项目。

## 路径范围

项目相对路径限制在项目根目录中。Discovery 和编辑会拒绝绝对路径、父级穿越、空路径、
重复逻辑名称、原始 YAML 中的重复 map key，以及指向项目外部的 symbolic link。文档、lock、
资料映射、输出和 fetch 工作目录由此位于一个可预测的范围内。

项目在计算机之间移动时，保留其相对布局。本机配置继续分开保存，因为 daemon capacity 与
catalog 位置属于接收项目的计算机。

## 一次运行保留的内容

队列接收后，attempt 拥有 resolved Case 与 RunProfile 副本，同时保存输入身份、执行设置、
数值标识和唯一 run ID。后续项目编辑用于之后的提交，不会改写已经存在的 attempt 目录。

因此，Profile 可以在研究过程中继续演进，旧结果仍保持可读。查看指定 attempt 时使用
`result inspect`，直接读取该运行记录的值，无需从当前项目反推。

## 相关概念

- [Domain-fill 水汽追踪](domain-fill.md)解释 dry-air carrier 和边界交换。
- [粒子 population](populations.md)比较 release、air-mass 和 ozone 初始化。
- [覆盖与运行身份](coverage-identity.md)说明 time anchor、job series、run 和 attempt。
- [Provenance 与结果目录](provenance.md)展示 resolved input 如何连接到 output sample。
