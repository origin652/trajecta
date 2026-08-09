---
title: 气象资料与数值路径
description: Trajecta 中的资料锁定、气象准备、插值、球面 RK2 积分、边界处理和 domain-fill population。
---

# 气象资料与数值路径

科学计算链从不可变输入文档开始，最终得到等待定时输出的有序粒子 batch。`trajecta-met` 负责
解释资料，`trajecta-core` 负责粒子生命周期。两者通过带类型的 prepared meteorological query
连接。

本页沿生产路径展开，并保留关键 module 与 type 名，方便贡献者直接定位源码。

## 输入绑定

生产运行接收已经解析的 Case、RunProfile，以及每个已选 Dataset Profile 对应的 DatasetLock。
项目在进入队列前通过 finalize 建立这组绑定。

Lock 固定运行期间不能漂移的资料选择：

| Lock 内容 | 运行时用途 |
| --- | --- |
| Dataset Profile 与 lock schema 身份 | 选择预期的资料解释合同 |
| 有序文件清单与 SHA-256 | 发现文件被替换、截断或选择发生漂移 |
| 时间覆盖 | 检查起点、中点、release、边界和输出查询都有 frame bracket |
| 水平与垂直 signature | 将 Case domain 绑定到已索引网格和 level |
| Capability set | 确认能够生成 transport 与 population 所需字段 |
| Source metadata | 将资料集身份写入 run manifest 和 provenance |

`trajecta-case` 定义 lock 结构。生产 loader 验证 lock 后，只访问声明的数据根目录。Attempt 启动后
不会临时寻找另一份更方便的文件。

## Profile 编译与源资料清单

Dataset Profile 描述源记录如何生成 canonical field。直接映射会指定准确的 GRIB 或 NetCDF
身份、源单位、时间语义，以及顺序明确的允许替代项。推导字段使用带类型的 computation graph。

Profile compiler 会在粒子查询开始前完成静态检查：

- Graph node ID 与 output 保持唯一；
- Dependency graph 中没有 cycle；
- 每个 operation 的单位和物理 dimension 一致；
- Array shape 与 vertical stagger 相容；
- Operation 只在合法的 frame、tile、column 或 sample stage 执行；
- 所需 source alternative 和 capability 可以解析。

Operation 集合是有名称的封闭集合，涵盖算术、单位换算、累计量差分、热力函数、hybrid pressure
构造和 native-grid diagnostic。Profile 中的 computation node 不能运行任意代码，也不能自行
访问文件。

Source inventory 主要依据 metadata 建立，不会只看文件名。Inventory 记录 container format、
valid time、grid description、vertical topology、field identity 和 dataset attribute。Profile 中的
exact matcher 从中选择记录。出现重复候选或不相容候选时，loader 会生成诊断，不采用隐式的
last-file-wins 规则。

## Frame 选择

Runner 会枚举模拟期间所有可能查询的物理时刻。其中包括运行端点、RK2 stage midpoint、连续
release window、population operation 和 scheduled output time。Builder 随后选择能够 bracket
整个时间区间的连续 source frame。

查询恰好落在 source timestamp 上时，推导字段仍可能需要相邻 frame，例如累计量差分。所需
warm-up depth 取自 capability dependency closure，因此累计字段会保留 graph 要求的前序支持。

每个已选 frame 会经过以下处理：

1. 解码 source-native array 和 validity mask。
2. 规范化单位，同时保留 source identity 与 quality。
3. 执行 frame-stage 和 tile-stage graph node。
4. 为 pressure-level 或 hybrid-level 资料构造 vertical geometry。
5. 将 canonical field 与 provenance assignment 存入 `RawMetFrame`。
6. 把 frame 放入有内存上限的 `FrameCache`，并为查询时刻组成 `PreparedWindow`。

水平 topology 保持源资料原生形式。准备阶段不会暗中建立全局重网格资料。规则经纬网资料通过
自己的 grid backend 查询，风矢量插值会处理球面 basis。

## Prepare 与 execute

气象查询分为两个清晰阶段：

```text
MetEngine::prepare_for_domain(time, domain)
    -> PreparedWindow
PreparedWindow::prepare_transport_batch(plan, points, workspace)
    -> PreparedTransportBatch
PreparedTransportBatch::execute(execution_context, workspace)
    -> TransportOutput
```

Prepare 阶段可以选择 cached frame，构造 column，计算 horizontal placement，并 pin 所需
stencil。Execute 阶段只接收这些已经固定的结构。它没有 reader 或 provider handle，因此粒子
采样时不会打开气象文件。

`BatchWorkspace` 保存可复用的 scratch vector。Caller 可在多个 batch 之间保留它，减少重复
allocation。Execution context 提供 worker count；少于 256 个 point 的 transport batch 串行执行，
较大的 batch 使用按 worker count 复用的 Rayon pool。

Cache 只承担查询准备和执行层面的复用：

| Cache | Key 的核心内容 | 用途 |
| --- | --- | --- |
| Frame cache | Dataset frame identity | 在内存预算内保留解码后的 canonical field |
| Tile 与 column cache | Field、frame、空间支持和 vertical request | 复用推导 neighborhood 与 column geometry |
| Boundary stencil session | 精确的 path-query support | 边界路径计算期间保持 stencil pinned |
| Last exact transport query | Window identity、query plan、domain、point array 与浮点 bit | 只复用逐字节等价的 transport request |

Exact transport key 包含 frame window、temporal weight、plan、vertical coordinate 和 point value。
Cohort、坐标、时刻或 domain 发生变化都会生成新 key。Cache hit 返回与该精确请求重新执行相同
的 typed output 和 provenance assignment。

## Transport 采样

积分器在海拔高度上查询三个 transport component：

- 东向风，单位为米每秒；
- 北向风，单位为米每秒；
- 几何垂直速度，单位为米每秒。

Query engine 查找水平 support，构造与源资料相符的 vertical column，应用 surface-layer rule，
随后混合两个 temporal frame。风矢量插值会考虑 spherical basis。Scalar field 与 vertical
velocity 遵循注册表中的 interpolation 和 derivation semantics。

每个 output row 同时包含数值、`SampleStatus`、field quality、bounds 和 provenance。Status 能够
区分有效资料、地下采样、surface-layer limit、model-top exit、domain exit 与 source support
无效。积分器不会依靠某个浮点 sentinel 推断有效性。

与边界有关的 status 可以交给连续边界路径处理。例如 predictor 已离开 domain，或者超过当前
垂直支持时，仍可能形成一条有明确首个物理交点的 segment，并正常终止在边界。确实缺少 transport
support 的 row 会记为异常 `invalid_meteorology`。

## 球面 midpoint RK2

生产积分器 `Rk2Spherical` 位于 `trajecta-core/src/integrator`。粒子从时刻 \(t\) 出发，带符号
step 为 \(\Delta t\) 时，会进行两次 batch transport query：

1. 在起始位置和时刻 \(t\) 查询速度。
2. 使用一半 \(\Delta t\) 构造 midpoint position。
3. 在 midpoint 位置和 \(t + \Delta t/2\) 查询速度。
4. 以 midpoint velocity 从原始位置推进完整 step。

水平位移在球面上计算，并规范化 longitude。垂直运动使用 geometric vertical velocity。正向和
反向运行共用一组方程，通过 \(\Delta t\) 的符号区分方向；particle age 按 step 绝对时长增加。

在 macro step 内出生的 row 可以拥有不同的精确 start time。Timed integrator 会按稳定顺序组合
相同 query time，每个 row 只推进一次，终点统一落在 macro-step endpoint。反向积分会逆序遍历
time group；同一个 batch 不允许混合两个积分方向。

坐标非有限、数值溢出或球面位移失败时，受影响的粒子按 numerical failure 终止。其余 live row
继续执行，因此终态 manifest 能够记录完整 population outcome。

## 边界顺序

Integrator 先返回尚未应用 boundary policy 的 proposal batch。Runner 随后检查 previous state
到 proposal 之间的连续 path。这样的分层使生命周期顺序不受积分器具体实现影响。

生产 policy 覆盖以下边界：

| 边界 | 处理结果 |
| --- | --- |
| 地表 | 按配置的 surface policy 反射，并受 reflection limit 约束 |
| Model top | 在首个 top intersection 终止 |
| 有限水平 domain | 在首个 domain intersection 终止 |
| 全球 longitude | 应用 periodic longitude handling |

同一段路径可能出现多个 intersection 时，ordered path segment 和 intersection fraction 共同确定
首个事件。Termination time 与 position 写在交点上，从而在两个积分方向中保持 lifecycle sample
单调。

## Population 生命周期

`PopulationModel` interface 管理 advection 前后的工作：

```text
initialize
pre-step and emission
advection by the shared integrator
post-advection accounting
boundary maintenance
finalize
```

### Release population

Release population 按配置的 release event 创建 row。Release coordinate 需要解释 pressure、terrain
或 model level 时，vertical resolver 会查询气象资料。Row 加入 integration cohort 前，birth time、
origin event、mass 和 stable particle identity 已经确定。

### Domain-fill air mass

Domain filling 从已选 native grid 和 vertical geometry 推导有限 atmospheric cell。有效 layer 的
dry-air mass 由气象状态计算，再按 Case 分配给粒子。初始 cohort 使用 `domain_initial` origin。

有限 domain 的 boundary-face layer 会携带 dry-air flux。积分方向决定哪个 signed flux 属于 inflow。
不足一个 particle share 的 mass 会作为 residual 保留到后续 step，不会在每次 boundary event
直接舍入。新 row 带有 `domain_boundary` origin、face identity、exact birth time 与 stable ID。

### Stratospheric ozone

Ozone population 共用 domain-fill transport 和 dry-air accounting，并采用 ozone-specific seeding
rule。它的 meteorological capability set 会包含判定 stratospheric source 与分配 ozone mass 所需
的字段。Population 特有科学规则位于 seeding 和 accounting 层；粒子运动仍使用公共 transport
query 与 RK2 implementation。

## 确定性与 provenance

Parallel query 开始前已经建立稳定顺序。Particle ID、birth identity、output event sequence 和排序
后的 SQLite insert 不依赖 Rayon task 的完成次序。相同 resolved input 改变 worker count 时，
wall time 可以变化，规范化科学内容应保持一致。

采样值与推导值都有 provenance assignment。最终 bundle 将 source record 和 transformation 独立于
particle table 记录，manifest 再绑定 bundle 与 SQLite identity。终态处理详见[输出与运行时](runtime.md)。

## 审查科学路径修改

根据修改的规则选择检查范围：

| 修改 | 最小聚焦覆盖 |
| --- | --- |
| Profile mapping 或单位换算 | Profile compile test 与真实 source inventory |
| Reader metadata 或 array layout | 覆盖已选 time 和 level 的 native/pure differential test |
| Derived field | Graph type check、quality/provenance assertion、reference value |
| Interpolation 或 vertical support | Domain 内数值、各类 typed boundary status、正反向 replay |
| Integrator | Convergence case、signed-time symmetry、极区/日界线、异常分类 |
| Boundary | 精确交点、事件顺序、reflection limit、单调 lifecycle output |
| Domain fill | Layer mass、residual carry、boundary inflow direction、stable birth identity |
| Query performance | Exact-key audit、execute-phase I/O counter、output digest、scaling matrix |

真实资料测试宜选择仍能覆盖生产 reader 和相关 topology 的最小 fixture。性能修改完成后，还需确认
规范化输出与本次科学调整的预期一致。
