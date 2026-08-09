---
title: 项目、案例、运行配置与资料锁
description: 了解 Trajecta 如何分别保存研究组织、科学设置、本机执行选择和实际使用的气象输入。
---

# 项目、案例、运行配置与资料锁

Trajecta 用四个相互关联的对象描述一次运行。科学设置与本机路径、计算资源分开保存，同一个研究
案例便可以在不同计算机上使用不同资料目录和资源配置。

| 对象 | 回答的问题 | 典型内容 |
| --- | --- | --- |
| 项目（Project） | 哪些试验属于同一项研究？ | 案例和运行配置的名称与路径、资料配置映射、默认运行配置 |
| 案例（`Case`） | 要进行怎样的物理模拟？ | 时间、方向、气象区域、粒子群、物质、数值方法、边界和输出 |
| 运行配置（`RunProfile`） | 这个案例在当前计算机上怎样运行？ | 案例路径、结果目录、资料目录与资料锁、读取器、线程和内存 |
| 资料锁（`DatasetLock`） | 本次运行实际使用哪些气象输入？ | 文件路径、字节数、散列、有效时次、网格、垂直坐标和资料配置摘要 |

命令行选择一个项目，再按名称选择其中的运行配置；运行配置随后指向一个案例。工作进程接收的是已经解析并
固定的案例与运行配置组合。

## 项目：组织一项研究

项目根目录包含 `trajecta-project.yaml` 和项目内的相对路径文件。一个最小索引如下：

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

左侧名称是项目内部标识。案例引用逻辑资料 `cfsr`；项目中的运行配置条目把它映射到内置公开
资料配置 `cfsr-pgbl-pressure-v0`。

一个项目可以登记多个案例和多个运行配置。常见组织方式包括：

- 一个案例对应多个线程数或读取器配置；
- 正向和反向案例共用同一套气象资料；
- 不同季节案例使用结构相同的运行配置；
- 保留正式运行配置，同时加入小型本机预检配置。

每个运行配置条目都指向一份 `RunProfile` 文档，该文档的 `case_path` 选择项目中已经登记的
案例。两个案例即使使用相同资源，也应分别登记运行配置，使提交命令能够唯一指向一组输入。

## 案例：可移植的科学设置

案例保存会改变轨迹物理含义的选择：

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

案例只使用逻辑资料名称，不保存数据服务凭据。UTC 时刻由 Unix 秒和纳秒精确表示。`direction`
决定积分从 `start` 向 `end` 的物理方向；资料覆盖计算会另行把两个时刻按先后排序。

大型区域、粒子群或输出配置可以拆到项目内的组件文件。`case resolve` 会展开引用，并输出后续
校验和运行使用的规范化案例。

## 运行配置：绑定本机路径和资源

运行配置把案例连接到当前计算机：

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

路径以运行配置文件所在目录为基准解析，并且必须留在项目根目录内。`data_roots` 将资料锁中的
根 ID 映射到本机目录。通过选择另一份运行配置，同一个逻辑案例可以使用不同磁盘布局、读取器或
工作线程。

`worker_threads` 与 `memory_budget_bytes` 会成为任务队列的资源请求。解析后的运行配置会随
结果保存，因此之后仍可查看某次执行实际申请了多少资源。

## 资料锁：固定气象文件

项目定稿时，Trajecta 检查本地文件并生成资料锁。资料锁将逻辑资料和内置资料配置绑定到：

- 每个文件的根 ID 与相对路径；
- 文件字节数和 SHA-256；
- 物理有效时次与文件角色；
- 规范化水平网格签名；
- 气压层或混合坐标签名；
- 内置资料配置的规范化 SHA-256；
- 生成工具名称和版本。

资料锁保存相对文件位置，运行配置提供实际根目录。只要锁定内容按相同相对布局放置，项目就可以
在 Windows 和 Ubuntu 上解析到对应资料。

项目定稿会选择覆盖案例时段及时间插值相邻帧的文件，并检查资料配置是否提供当前粒子群和边界
模型所需的能力。

## 三种项目状态

`project status` 根据当前内容给出：

| 状态 | 含义 | 下一步 |
| --- | --- | --- |
| `draft` | 可识别的案例或运行配置仍缺少必填项 | 继续逐项编辑并校验 |
| `configured` | 逻辑文档完整，资料或资料锁尚待准备 | 生成资料计划并准备气象资料 |
| `finalized` | 文档、路径、资料文件、内置资料配置和资料锁一致 | 运行 `doctor --deep` 后提交任务 |

状态之外还可能附带错误，例如索引路径无效或 YAML 文档畸形。查看状态时应同时阅读诊断。

## 校验、解析与项目定稿

三个操作负责不同阶段：

1. **校验**检查文档结构、字段类型、引用和当前命令要求。
2. **解析**展开项目内组件引用，形成规范化案例或运行配置。
3. **项目定稿**打开选定资料，检查覆盖与能力，并创建或核对资料锁。

因此，资料仍在下载时，项目可以先完成案例和运行配置审阅并保持 `configured`。文件大小、
内容散列和覆盖范围会在项目定稿阶段写入资料锁。

## 路径范围

项目相对路径被限制在项目根目录内。项目发现和编辑会拒绝绝对路径、父目录穿越、空路径、重复
逻辑名称、原始 YAML 中的重复映射键，以及指向项目根目录外的符号链接。

项目在计算机之间移动时，应保持相对目录布局。本机配置单独迁移或重新创建，因为守护进程容量和
任务数据库位置属于接收计算机。

!!! tip "科学设置和机器设置分别修改"

    时段、区域和粒子群放在案例中；线程、内存、读取器和本机路径放在运行配置中。

## 一次运行会保存什么

任务被队列接受后，该执行轮次会保存解析后的案例和运行配置、输入资料散列、执行设置、数值模型
标识和唯一运行 ID。之后修改项目只影响后续提交，不会回写已有结果目录。

要查看某次运行实际使用的配置，使用 `result inspect` 或直接读取结果目录中的解析后文档，不必
根据当前项目反推旧设置。

## 继续阅读

- [区域填充水汽追踪](domain-fill.md)说明干空气载体和有限区域交换。
- [粒子群模型](populations.md)比较定时释放、气团和臭氧初始化。
- [覆盖范围与运行标识](coverage-identity.md)解释资料时次、任务系列和执行轮次。
- [溯源信息与结果目录](provenance.md)说明输入文件如何与粒子样本相连。
