# Trajecta M3 气象查询引擎计划

## 1. 文档状态

本文是 M3 的目标、科学合同、实现边界和验收标准。M3 实现、评审和任务分派均以本文为准。

M3 的模型分工见 `TRAJECTA_M3_MODEL_ASSIGNMENT.md`。若本文与更早的概览文档冲突，以本文为准；修改冻结的科学公式、公共接口、真实资料锚点或容差政策时，必须同步更新验收报告并重新进行 A 级科学评审。

截至 2026-07-16，A0/A1 公共合同和数值核心已实现并通过门禁；B1 Phase 1 已接通真实 CFSR 00/06 查询与 CLI，A 又完成复合物理维度、热通量符号、FRICV/stress 替代依赖和 opt-in Explain 收尾。workspace 191 个测试和强制真实资料 113 个测试通过，裁决见 `TRAJECTA_M3_A1_CLOSURE.md`。当前仍不是 M3 完成：混合业务目录发现、CFSR 第三时次、两套 ERA5 正式链、native/跨平台差分、FLEXPART oracle、百万点终审和 A2 裁决尚未完成。

截至 2026-07-18，三套正式资料查询链、Windows native 双后端和百万点主矩阵已有真实证据；A 已冻结 `m3-a-tolerance/v1.0.0`、FLEXPART v11.1 oracle v1 原始格式、统一比较报告和分层 hard-gate/report-only 规则。当前仍不是 M3 完成：真实 reader/provider execute 零调用计数、三套真实 FLEXPART 数值 oracle、Linux 矩阵、跨平台容差测量和 A2 最终裁决仍未闭合。

截至 2026-07-19，reader/provider execute 零调用计数、百万点确定性矩阵和三套 Windows native 全场比较已有实际证据；A 已发布 `m3-a-tolerance/v1.0.1`，仅将 CFSR specific humidity 从 2 ULP 修订为 3 ULP，pressure vertical velocity 保持 2 ULP，三套 backend 在新 registry 下全部通过。当前仍不是 M3 完成：三套真实 FLEXPART 数值 oracle、Linux 全矩阵、跨平台裁决和 A2 最终认证尚未闭合。

截至 2026-07-20，A 已发布 `m3-a-tolerance/v1.0.2`，注册 pressure-coordinate adapter，并完成三套 FLEXPART 75/75 oracle 的分层科学裁决。Windows 与 WSL Ubuntu 24.04 的完整门禁、三套 Rust/native 全场比较、三套 MIT oracle hard gate、百万点长测及模拟 M4 两次 RK2 查询均已实际通过；WSL 机器汇总为 `core_status=0`、`a2_full_status=passed`。A2 最终签署见 `TRAJECTA_M3_A2_CLOSURE.md`，M3 状态改为完成。明确列入 report-only 的 FLEXPART legacy 算法差异继续保留，不构成 Trajecta 科学算法回退或数值阈值放宽。

## 2. M3 目标与完成边界

M3 的终点是：给定本地锁定的真实气象资料、物理时刻、经纬度和一种垂直坐标，返回可由 M4 直接消费的完整输送气象查询结果。

强类型 Transport 输出固定包含：

- eastward wind `U`；
- northward wind `V`；
- geometric vertical velocity `W`；
- air pressure；
- air temperature；
- specific humidity；
- moist-air density；
- geometric terrain height；
- 每点状态、逐字段有效位、质量等级、provenance 和局地垂直有效范围。

查询必须支持：

- 几何海拔 ASL；
- 相对地面高度 AGL；
- 压力 Pa；
- 每批一个物理时刻；
- 每批一种垂直坐标；
- 单域完整闭环。

M3 只完成 `Transport` 和 `NearSurfaceTransport`。以下内容不阻塞 M3：

- 粒子推进、球面 RK2 和边界策略；
- PV、PV 梯度、dry-air mass 和 domain-fill，它们作为 M4.0 前置切片实现；
- 云、降水、对流、沉降和化学派生；
- Gaussian、投影网格、重网格和实际嵌套域；
- GPU 后端、运行时远程 I/O 和公开 FLEXPART-compatible 运行模式；
- 通用 TileCache 和预取优化。

## 3. 公共合同

### 3.1 查询生命周期

稳定高层生命周期为：

~~~text
MetEngine::compile_plan(...)
  -> QueryPlan 或 TransportPlan
MetEngine::prepare(time, plan)
  -> PreparedWindow
PreparedWindow::prepare_batch(batch, workspace)
  -> PreparedBatch
PreparedBatch::execute(...)
  -> QueryOutput 或 TransportOutput
~~~

规则：

- `prepare` 可以选帧、读取文件、更新准备期缓存和固定时间权重；
- `prepare_batch` 可以定位 cell、构造 stencil、规划内存和固定 chunk；
- `execute` 不得调用 reader、provider、文件系统或改变缓存状态；
- replay 输入可包含多个时刻，但 CLI 必须稳定分组为多个单时刻批次，并恢复调用者顺序；
- RK2 由 M4 依次执行起点批次和中点批次，中点位置依赖第一次查询，M3 不提供伪双时刻批次。

### 3.2 通用输出与强类型输出

保留通用 `QueryPlan / QueryOutput`，只准备和返回请求字段；同时增加冻结的 `TransportPlan / TransportOutput`，避免 M4 在热路径按字段名拼装核心结果。

`QueryOutput` 和 `TransportOutput` 必须提供：

- 原调用顺序的 SoA 数值列；
- 每点 `SampleStatus`；
- 每字段有效 bitmask；
- 每字段 `FieldQuality`；
- 紧凑 provenance ID；
- `VerticalBounds`；
- 可选 explain 记录。

Explain 必须在 plan 编译时显式选择。默认 `Disabled` 时不得创建逐点 Explain 容器；`Full` 时其内存必须在 `prepare_batch` 规划 chunk 前计入预算。

数值列不得使用 NaN、无穷、99999 或其它数值哨兵传递状态。无效槽位的数值没有科学含义，调用者必须检查有效位。

### 3.3 VerticalBounds

普通查询无论成功还是失败，都返回同一时刻、同一 Profile 和同一 surface model 下的局地范围：

- `terrain_asl_m`；
- `minimum_transport_agl_m`；
- `minimum_transport_asl_m`；
- `available_top_asl_m`；
- `physical_model_top_asl_m`，资料无法证明时为空；
- `minimum_pressure_pa`；
- `maximum_pressure_pa`。

另提供轻量批量 bounds 查询，供释放预检、CLI 和 M4 碰地处理复用。bounds 与普通查询必须使用同一局地柱和同一有效性规则。

### 3.4 错误边界

以下属于结构错误，整批失败：

- QueryPlan 未编译或字段不存在；
- capability、Profile 或派生依赖不足；
- 缺少包围时刻所需的帧；
- 网格、垂直签名或多文件逻辑帧不一致；
- 严格模式依赖 Estimated 字段；
- 双帧和最小固定资源无法进入内存预算；
- 输入 SoA 长度不一致或执行状态损坏。

以下属于局地点状态，不影响同批其它点：

- `OutOfDomain`；
- `PolarSingularity`；
- `BelowGround`；
- `SurfaceLayerUndefined`；
- `AboveAvailableTop`；
- `AboveModelTop`；
- `InvalidVerticalColumn`；
- `NumericalFailure`。

精确纬度 `+90` 或 `-90` 返回 `PolarSingularity`；任意接近极点的位置、周期经度和日期变更线必须正常工作。

## 4. Canonical fields 与 capability

M3 必须补齐至少以下稳定字段语义：

- sample air pressure；
- geometric height 和 geometric terrain height；
- 10 m eastward/northward wind；
- 2 m air temperature 和 specific humidity；
- aerodynamic roughness length；
- boundary-layer height；
- eastward/northward surface stress；
- sensible heat flux 和 latent/moisture heat flux；
- friction velocity；
- Monin-Obukhov length。

canonical 字段维度使用共享 SI 指数向量，不能按单位字符串猜测，也不能把 density、geopotential、pressure tendency 或 energy flux 临时标为 Dimensionless。热通量统一向上为正；数据源若采用相反符号，必须通过可审计的 Profile 派生转换。

新增 `NearSurfaceTransport` capability。正式供 M4 使用的 `TransportPlan` 同时要求 `Transport` 与 `NearSurfaceTransport`；只查询上层资料的通用计划可以仅要求 `Transport`。

严格模式接受：

- 从锁定来源直接解码的 `Source` 字段；
- 由锁定源字段和冻结公式确定性计算的 `Derived` 字段。

土地类型经验表、固定中性层、常数粗糙度和其它经验补值只能在 `allow_estimated=true` 时使用，并逐字段标记为 `Estimated`。三套正式科学锚点必须在 `allow_estimated=false` 下通过。

## 5. 真实资料锚点

### 5.1 ERA5 hybrid 主锚点

主锚点使用 ECMWF 官方 CDS/MARS 服务交付的规则经纬网模式层 GRIB，不依赖 flex_extract：

- 日期：2018-12-01；
- 时次：00、03、06 UTC；
- 区域：约 55°N--40°N、5°W--15°E；
- 网格：0.25° 规则经纬网；
- 模式层：完整 1--137；
- 三维量：U、V、T、q、omega；
- 地表量：lnsp 或 surface pressure、surface geopotential；
- 近地量：10 m 风、2 m 温湿来源、roughness、PBL height、stress/flux 以及直接或可推导的 ustar/L 输入。

ERA5 原生 reduced Gaussian 网格不进入 M3。由 ECMWF 官方服务按规则网格交付属于正式科学输入，不算 Trajecta 本地派生转换。

### 5.2 ERA5 pressure 锚点

官方 CDS NetCDF4 扩展到 2018-12-01 00、06、12 UTC，包含 37 个标准压力层、逐层 geopotential、U/V/T/q/omega、surface pressure、surface geopotential 和完整近地字段。

官方 NetCDF4 是科学锚点。由它确定性转换的 classic NetCDF3 只验证格式和后端等价性。

### 5.3 CFSR pressure 锚点

官方 NCEI CFSR 低分辨率 `pgbl` GRIB2 扩展到 2009-01-01 00、06、12 UTC。

同一产品已包含逐层 HGT、U/V/T/q/VVEL，以及 surface pressure、terrain、10 m 风、2 m 温湿、PBL height、FRICV、surface roughness 和热/动量通量。Profile 必须使用这些同网格字段，不得为近地层偷偷引入未声明重网格。

### 5.4 辅助与拒绝锚点

- 现有 flex_extract ERA5 130--137 层资料保留为快速兼容回归；
- 从地面连续向上的 hybrid 子集可在其真实高度范围内查询；
- 悬空层子集只有在额外提供绝对位势锚点时才可查询；
- 子集不得冒充完整模式柱，超出其顶界返回 `AboveAvailableTop`；
- NOAA PSL NCEP R1 的层集合不一致，必须在计划阶段明确拒绝完整 Transport。

### 5.5 获取与冻结

M3 只提供可复现离线获取工具：

- 固定请求参数和来源 URL；
- `.part`、续传、size 和 SHA-256 校验；
- 原子完成；
- manifest 记录许可、attribution、变量、时次、区域、网格和哈希；
- 大型资料位于外部缓存，不提交仓库。

运行库不处理认证、下载、限流和远程重试。

## 6. 数值算法合同

### 6.1 水平定位与球面风

- v0 只实现规则经纬网；
- 全球经度按周期规范化，不复制日期变更线端点；
- 标量四角有效时使用固定顺序双线性；
- 三角有效区使用固定顶点顺序的重心插值；
- 两点及以下不得降级成边线或最近邻外推；
- U/V 在每个源点转换为地心三维切向矢量，插值后投影到查询点 east/north 基底；
- 同一物理点的结果不得依赖经度采用 `-180..180` 还是 `0..360` 表示。

### 6.2 时间

- 查询时刻必须由真实帧严格包围，不允许时间外推；
- 非整帧时刻使用前后帧分段线性权重；
- 普通字段在两个物理端点完成空间/垂直查询后再做时间插值；
- 几何 W 需要层面时间导数，恰好位于内部整帧时取左右区间斜率平均；
- 完整 Transport 的首帧和末帧整点不可查询 W，CoverageReport 必须把可用范围向内缩一帧；
- 时间方向不得改变同一物理时刻的结果。

### 6.3 Hybrid 局地柱

对查询 cell 的四个角分别构造原生柱，再把同一模式层插值到查询点；不得先把 surface pressure、温湿和系数混成一根伪平均柱后再积分。

冻结公式：

~~~text
p_half(k) = A(k) + B(k) * p_surface
p_full(k) = 0.5 * (p_half(k-1) + p_half(k))
T_virtual = T * (1 + (R_v / R_d - 1) * q)
rho = p / (R_d * T_virtual)
~~~

位势使用 ECMWF 文档化的 hydrostatic/alpha 递推，从 surface geopotential 向上积分，并采用其顶层压力约定。计算必须保留完整 A/B 半层系数和实际 `active_full_levels`。

几何高度使用：

~~~text
g0 = 9.80665 m/s2
R_e = 6371229 m
z_geometric = R_e * geopotential / (g0 * R_e - geopotential)
~~~

位势高度保留为独立字段，不得把它与几何 ASL 混名。

### 6.4 等压局地柱

- 压力层保持模型顶到近地面的严格递增 Pa 顺序；
- values 和 validity mask 使用完全相同的层排列；
- 逐层 geopotential 或 geopotential height 转为几何高度；
- 地下层保持结构化无效，不制造填充值；
- 四角有效使用双线性，三个有效角使用同一三角平面，查询点不在三角形内则该层无效；
- 局地压力和高度必须单调，否则整根局地柱返回 `InvalidVerticalColumn`。

### 6.5 垂直查询

- ASL 和 AGL 以几何高度定位；
- Pa 以 `log(p)` 定位和插值；
- 压力本身使用指数型处理；
- ASL/AGL 位于地形以下返回 `BelowGround`；
- Pa 大于局地 surface pressure 同样视为地下；
- 超出实际资料顶界和物理模式顶分别报告；
- 禁止垂直外推、顶底夹层和最近层替代；
- 时间插值完成后，由最终 p/T/q 重新计算 density，不直接插值来源 density。

### 6.6 几何垂直速度

几何 W 使用完整运动学链：

~~~text
W = dz/dt + U * dz/dx + V * dz/dy + Cdot * dz/dC
~~~

- 等压资料使用 `C = p`、`Cdot = omega`；
- eta-dot 资料直接使用原生 eta-dot；
- hybrid 资料若只有 omega，先由压力面的时间变化、水平坡度和垂直导数求原生坐标速度；
- 水平层面坡度必须对查询实际使用的同一双线性曲面解析求导；
- 三角有效区使用同一平面梯度；
- 地球曲率和经纬度到 east/north 距离的换算使用冻结球面常量；
- W 在目标时刻由目标时刻的 U/V、坐标速度和层面导数组合，不允许用 `-omega/(rho*g)` 冒充完整结果。

地面无穿透条件为：

~~~text
W_surface = U * d(terrain)/dx + V * d(terrain)/dy
~~~

不得无条件把地面 W 设为零。

### 6.7 近地层

M3 只公开 `MoninObukhovBusingerDyer/v0`。FLEXPART-compatible surface model 延期，FLEXPART 只作为外部 oracle。

严格输入包括：

- 10 m U/V；
- 2 m T/q；
- surface pressure 和 terrain；
- aerodynamic roughness；
- PBL height；
- 最低三维层 U/V/T/q/p/W；
- 直接 ustar/L，或足以按冻结公式推导它们的 stress、flux 和热力状态。

相似理论使用范围上限为最低三维层与 `0.1 * PBL height` 的较低者。该范围以上、最低三维层以下使用确定性的单调平滑连接，并保持锚点值与端点连续。压力在 surface pressure 和最低三维压力之间按受约束的 log-pressure 曲线处理，density 由最终状态重算。

AGL 等于零或低于模型最小有效高度时，完整 Transport 返回 `SurfaceLayerUndefined`。调用者通过 `VerticalBounds` 处理碰地；bounds、terrain 和地面边界 W 仍可查询。不得暗中把查询高度夹到 z0、2 m、10 m 或最低模式层。

## 7. 执行、缓存和内存

M3 硬门槛只要求：

- `FrameCache`；
- 跨批次 `ColumnStencilCache`；
- 自动 cell 分组和 chunk；
- caller-owned `BatchWorkspace`；
- 冻结的 `PreparedWindow / PreparedBatch`。

通用 TileCache、预取和 GPU 布局可以保留接口，但不得为了宣称完成而加入无消费者的空实现。

内存规则：

- 每次 `prepare` 或 `prepare_batch` 重新采样当前可用内存；
- 默认最多使用可用内存的 50%；
- RunProfile 可提供更低的硬上限；
- 双帧和最小固定资源无法进入预算时提前失败并报告所需字节；
- PreparedBatch 创建后，预算、Pin、排列和 chunk 完全冻结；
- 当前批引用的条目不得淘汰；
- 缓存命中、淘汰和分块不得改变结果；
- 执行期不得临时突破预算。
- Explain 关闭时不分配逐点记录；开启时记录、逐字段条目和动态标识的保守开销进入每点预算。

源值保留来源精度：f32 和打包资料紧凑存储，真实 f64 不得强制降为 f32；坐标、权重、派生计算和输出统一使用 f64。

同平台、同二进制、同后端下，不同线程数、分块、内部排序和缓存冷热状态必须 bitwise identical。跨平台和 Rust/native 后端使用冻结容差。

## 8. CLI 合同

M3 提供正式：

- `trajecta met probe`：单点或批量查询、bounds 和人类可读摘要；
- `trajecta met replay`：稳定流式 JSONL 输入输出；
- `--explain`：增加域、帧、时间权重、cell、水平权重、垂直括号、surface model、quality 和 provenance。

replay 始终输出一条带 schema version 的 `probe_summary`。大批输入不得先整体读入内存；多时刻输入按稳定顺序分组或分段处理，并恢复输入记录身份。

## 9. 验收矩阵

### 9.1 解析场和单元测试

必须覆盖：

- 仿射标量场的双线性精确性；
- 日期变更线、周期经度、近极点和精确极点；
- 全球切向矢量场及局地基底投影；
- 等压地下 mask、三角有效区和禁止外推；
- 等温和分层解析大气的 hybrid pressure、位势、几何高度和 density；
- 移动且倾斜层面的解析 W，分别覆盖 omega 和 eta-dot；
- ASL、AGL、Pa、整层、层间、整帧和帧间；
- Businger-Dyer 的稳定、中性、不稳定、静风和粗糙度边界；
- 相似层到最低三维层的连续性和无非物理超调；
- 贴地拒绝、VerticalBounds 和地面无穿透 W；
- 空批次、单点批次、乱序输入和原顺序恢复。

纯 f64 解析公式测试使用严格的 ULP/绝对误差门槛；不得用真实资料容差掩盖公式错误。

### 9.2 真实资料正向链

ERA5 hybrid、ERA5 pressure 和 CFSR pressure 必须全部完成：

~~~text
fetch/manifest
  -> lock/inventory
  -> frame loading
  -> QueryPlan/TransportPlan
  -> prepare
  -> prepare_batch
  -> execute
  -> probe/replay
~~~

每套资料必须：

- 使用三个连续时次；
- 在 `allow_estimated=false` 下同时满足 Transport 和 NearSurfaceTransport；
- 查询 ASL、AGL 和 Pa；
- 覆盖整帧、中间时刻、海面、平原、山地、近地层和高空；
- 验证局地上下界、地下、实际顶界和物理顶界；
- 验证所有八个 Transport 数值列、有效位、quality 和 provenance；
- 验证相同逻辑资料的 GRIB/NetCDF3/NetCDF4 等价性；
- 在可用环境中完成 pure-Rust/native ecCodes/native-netCDF 端到端差分；
- 同一 index 和 PreparedWindow 重复查询，验证 worker 和缓存生命周期。

NOAA R1 必须通过负例证明它不会错误发布完整 Transport。

### 9.3 FLEXPART oracle

GPL oracle 是独立工具，按固定 FLEXPART commit、编译器、输入哈希和 harness 版本生成冻结 JSON。MIT crate 只能读取结果，不能链接、复制或编译 GPL 实现进入发布物。

采用分层判定：

- U/V/T/q/p/terrain 等共同语义字段按版本化容差硬门控；
- 现代几何高度、完整运动学 W 和现代近地层按独立公式及物理不变量硬门控；
- 同时生成它们相对 FLEXPART 的差异统计；
- 异常量级不得仅以“现代算法不同”为由忽略，必须修复或进入带科学解释的版本化 allowlist。

容差写入冻结文件，至少按字段、算法版本、资料族和比较类型区分。首次由固定锚点校准并由 A 模型评审；以后禁止自动放宽。修改容差必须更新算法或数据理由并重新生成验收报告。

### 9.4 性能与确定性

终审至少执行一个 1,000,000 点的单时刻批次：

- 在显式受限内存预算下自动分块完成；
- 引擎记账字节不得超过预算；
- reader/provider 执行期调用计数为零；
- 1 线程与多线程结果 bitwise identical；
- 不同 chunk、冷/热缓存和输入内部排列结果 bitwise identical；
- 只记录跨机器吞吐，不冻结绝对每秒点数；
- 验证耗时和临时内存近似线性，而不是随点数超线性增长。

### 9.5 平台、CI 与 M4 接口

Windows 和 Linux 是 M3 硬门槛：

- 默认纯 Rust 门禁；
- 完整真实资料链；
- probe/replay；
- 在具备外部库的任务中运行 native 差分；
- 相同冻结资料的跨平台容差比较。

macOS 在 M3 只保证可编译或社区支持。

PR 运行解析场、小型真实夹具、现有 8 层 flex_extract 回归和默认 workspace 门禁。完整 137 层资料、三套真实矩阵、GPL oracle、native 双后端和百万点测试在夜间及 M3 终审强制运行。

M3 必须提供模拟 M4 消费测试：

~~~text
起点时刻和位置查询 Transport
  -> 测试侧计算球面半步位置和高度
  -> 中点时刻查询 Transport
~~~

该测试只验证接口足以支持 RK2，不在 M3 实现粒子状态和推进器。

## 10. 验收报告与完成定义

M3 认证生成：

- 机器可读 JSON 报告；
- 人类可读 Markdown 摘要。

报告记录：

- 数据来源、许可、哈希、Profile 和时空范围；
- 算法、常量、surface model 和容差版本；
- 解析场、真实资料、格式、后端和平台结果；
- FLEXPART oracle 差异与 allowlist；
- 百万点内存、分块、确定性和吞吐结果。

完整报告只在以下情况重新生成：

- M3 首次认证；
- 科学算法或常量变化；
- 容差变化；
- 正式锚点、Profile 或 reader 科学语义变化。

普通业务运行以及科学合同未变化的版本不要求重复生成完整报告，只需在 RunManifest 中记录算法、Profile、资料和容差版本。

只有公共合同、三套正式资料、NearSurfaceTransport、几何 W、oracle、Windows/Linux、百万点和模拟 M4 消费测试全部通过，才可声明 M3 完成。
