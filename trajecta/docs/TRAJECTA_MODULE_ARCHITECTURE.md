# Trajecta v0 模块与类型架构

## 1. 文档目的

本文把 Trajecta v0 实施计划拆成可直接分派的工程模块，说明：

- crate 和 module 名称；
- Rust 中相当于 class 的 struct、trait 与 enum；
- 每个类型的职责；
- 编译期依赖；
- 运行时数据流；
- 模块间接口；
- 实现风险；
- 建议使用的模型等级。

Rust 不使用传统 class 关键字：

- struct 负责保存状态和数据；
- trait 定义可替换行为与接口；
- enum 定义封闭状态或策略集合；
- impl 为 struct 或 trait 提供方法。

## 2. A/B/C 难度定义

难度等级表示推荐承担该任务的 AI 模型能力，而不是人工开发时间。

| 等级 | 模型要求 | 适用任务 |
|---|---|---|
| A | 只能由 SOTA 模型可靠完成 | 数值算法、复杂生命周期、科学正确性、跨模块架构、许可边界、高风险并发和缓存 |
| B | 次一级高性能模型可以完成 | 明确规格下的读取器、类型系统、验证器、并行执行器、转换器和中等复杂算法 |
| C | 一般性价比模型可以完成 | CLI、序列化、脚手架、简单数据类型、文档、常规测试和机械映射 |

分派原则：

- A 模块必须先有明确数学规格、输入输出合同和验收夹具；
- B 模块必须有稳定接口和边界条件；
- C 模块可以在上游接口冻结后并行完成；
- A 模块不得仅凭局部代码补全来实现，必须理解整体数据流；
- 同一文件同时包含 A 和 C 工作时，应由 A 模型先定结构，再将机械部分拆给 C 模型。

## 3. Workspace 总览

~~~text
trajecta/
  crates/
    trajecta-case/
    trajecta-met/
    trajecta-core/
    trajecta-cli/
~~~

### 3.1 编译期依赖

~~~mermaid
flowchart TD
    CLI["trajecta-cli"]
    CORE["trajecta-core"]
    MET["trajecta-met"]
    CASE["trajecta-case"]
    FLEXCTL["GPL flexctl-core"]
    CONVERTER["GPL flexpart-to-case"]
    ORACLE["GPL FLEXPART met oracle"]

    CLI --> CORE
    CLI --> MET
    CLI --> CASE
    CORE --> MET
    CORE --> CASE
    MET --> CASE
    FLEXCTL --> MET
    FLEXCTL --> CASE
    CONVERTER --> CASE
    ORACLE -. "JSON only" .-> MET
~~~

约束：

- trajecta-case 不依赖其它 Trajecta crate；
- trajecta-met 不依赖 trajecta-core；
- trajecta-core 不向气象层泄漏粒子实现；
- GPL 工具可以依赖 MIT crate；
- MIT crate 不得链接 GPL crate；
- Oracle 与 MIT 测试只通过 JSON 数据交互。

### 3.2 运行时主数据流

~~~mermaid
flowchart LR
    CASEDOC["CaseDocument"]
    RUNPROF["RunProfile"]
    LOCK["DatasetLock"]
    PCAT["ProfileCatalog"]
    MCAT["MetCatalog"]
    ENGINE["MetEngine"]
    WINDOW["PreparedWindow"]
    LAYOUT["BatchLayout"]
    CACHE["ColumnCache / TileCache"]
    OUTPUT["QueryOutput SoA"]
    RUNNER["SimulationRunner"]
    POP["PopulationStrategy"]
    INTEGRATOR["IntegratorModel"]
    PRODUCTS["Output Products"]

    CASEDOC --> MCAT
    RUNPROF --> LOCK
    LOCK --> MCAT
    PCAT --> MCAT
    MCAT --> ENGINE
    ENGINE --> WINDOW
    WINDOW --> LAYOUT
    LAYOUT --> CACHE
    CACHE --> OUTPUT
    OUTPUT --> INTEGRATOR
    POP --> RUNNER
    INTEGRATOR --> RUNNER
    RUNNER --> PRODUCTS
~~~

## 4. Crate 难度总表

| Crate | 总体职责 | 难度 | 主要原因 |
|---|---|---:|---|
| trajecta-case | 配置合同、单位、引用、验证、锁文件 | B | 类型多、兼容边界多，但算法风险有限 |
| trajecta-met | 资料读取、层结、派生、插值、缓存 | A | 科学算法、数据格式、并发、确定性和性能同时存在 |
| trajecta-core | 粒子积分、群体生命周期、domain-fill | A | 数值推进、边界条件和质量守恒形成完整闭环 |
| trajecta-cli | 命令解析、输出和编排 | C | 主要依赖稳定库接口 |
| GPL legacy adapter | 旧 Case 迁移和兼容验证 | B | 旧语义复杂，但目标接口明确 |
| GPL met oracle | 原 FLEXPART 路径参考计算 | A | Fortran全局状态、构建依赖和数值审计风险高 |

## 5. trajecta-case

建议源码布局：

~~~text
trajecta-case/src/
  lib.rs
  diagnostic.rs
  document.rs
  quantity.rs
  reference.rs
  resolver.rs
  intent.rs
  lockfile.rs
  schema.rs
  model/
    mod.rs
    metadata.rs
    time.rs
    meteorology.rs
    population.rs
    substance.rs
    numerics.rs
    physics.rs
    output.rs
~~~

### 5.1 diagnostic

| 类型 | 种类 | 作用 | 连接模块 | 难度 |
|---|---|---|---|---:|
| Diagnostic | struct | 保存 severity、code、message、path、hint | 所有 crate | C |
| Severity | enum | Error、Warning、Info | Diagnostic | C |
| DiagnosticBag | struct | 收集、排序和合并诊断 | schema、resolver、met、core | C |
| DiagnosticPath | struct | 类型化字段路径，避免手拼字符串 | schema、转换器、CLI | B |

关键方法：

~~~text
Diagnostic::error(code, message)
Diagnostic::at(path)
DiagnosticBag::push()
DiagnosticBag::has_errors()
DiagnosticBag::into_sorted()
~~~

约束：

- 错误代码稳定；
- 诊断顺序确定；
- 不使用 panic 表示用户配置错误。

### 5.2 document

| 类型 | 种类 | 作用 | 上游/下游 | 难度 |
|---|---|---|---|---:|
| DocumentKind | enum | Case、RunProfile | schema、CLI | C |
| CaseDocument | struct | v0 Case 顶层 | resolver、core、met | B |
| RunProfileDocument | struct | 机器资源与数据位置 | resolver、CLI | B |
| Metadata | struct | 名称、描述、标签和作者信息 | CaseDocument | C |
| ResolvedCase | struct | 所有 ref 展开后的规范 Case | met、core、manifest | B |
| ResolvedRunProfile | struct | 路径和资源已解析的运行配置 | runner、met | B |

CaseDocument 字段：

~~~text
schema_version
kind
metadata
time
meteorology
particle_population
substances
numerics
physics
outputs
~~~

各组件使用 Option，是否必需由 ValidationIntent 判断。

### 5.3 quantity

| 类型 | 种类 | 作用 | 连接模块 | 难度 |
|---|---|---|---|---:|
| Quantity | 泛型 struct | value 加单位的类型化物理量 | Case全部科学字段 | B |
| Unit | enum/ID | 支持时间、长度、压力、质量等 | DSL、Case、CLI | B |
| Dimension | enum | Length、Time、Pressure等维度 | DSL编译器 | B |
| QuantityInput | enum | 对象形式或字符串简写 | serde输入 | B |
| UnitRegistry | struct | 单位解析与SI换算 | quantity、profile graph | B |

关键约束：

- Case 输入可以写字符串或对象；
- resolved JSON 只输出对象；
- 单位换算统一进入 SI；
- 不允许无单位数字隐式猜测；
- UnitRegistry 与 profile graph 使用同一维度系统。

### 5.4 reference 与 resolver

| 类型 | 种类 | 作用 | 连接模块 | 难度 |
|---|---|---|---|---:|
| ComponentRef | enum | Inline 或 RefPath | Case组件 | B |
| RefPath | struct | 受控相对路径 | resolver | C |
| RefResolver | trait | 解析组件引用 | LocalRefResolver | B |
| LocalRefResolver | struct | v0本地文件实现 | CLI、Case | B |
| SourceDigest | struct | 路径、大小和SHA-256 | resolved case、manifest | C |
| ResolutionGraph | struct | 检测循环引用和来源链 | resolver | B |

规则：

- v0 组件 ref 只允许本地相对路径；
- 拒绝循环引用；
- resolved Case 保存所有来源哈希；
- 引用解析不得访问网络。

### 5.5 intent

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| ValidationIntent | enum | MetProbe、MetReplay、Simulation、Migration | B |
| CaseRequirements | struct | 每种用途所需组件和能力 | B |
| IntentValidator | struct | 依据用途产生缺项诊断 | B |

示例：

~~~text
MetProbe:
  requires time + meteorology

Simulation:
  requires time + meteorology + population + numerics
  requires outputs when requested
  requires substances based on enabled physics modules
~~~

### 5.6 lockfile

| 类型 | 种类 | 作用 | 连接模块 | 难度 |
|---|---|---|---|---:|
| DatasetLock | struct | 一个逻辑数据集的不可变文件清单 | met inventory | B |
| LockedFile | struct | 文件角色、时次、大小、哈希 | readers | C |
| DatasetIdentity | struct | 数据集ID、来源和归属 | manifest | C |
| ProfileIdentity | struct | 档案名和内容哈希 | ProfileCatalog | C |
| GridSignature | struct | 网格摘要 | MetCatalog | B |
| VerticalSignature | enum | Hybrid或Pressure摘要 | MetCatalog | B |

正式运行必须验证：

- lockfile自身格式；
- 档案名称唯一；
- 档案哈希；
- 文件大小；
- 文件SHA-256；
- 时次覆盖；
- 网格和层结签名。

### 5.7 model 子模块

| Module | 关键类型 | 作用 | 难度 |
|---|---|---|---:|
| model::time | TimeSpec、Direction | 起止时间、正反向 | C |
| model::meteorology | MeteorologySpec、DomainSpec、DatasetRef | 逻辑气象域 | B |
| model::population | ParticlePopulationSpec | release/domain-fill配置 | B |
| model::substance | SubstanceSpec、SubstanceId | 具名物种和物性 | B |
| model::numerics | NumericsSpec、IntegratorSpec、BoundarySpec | 数值算法与边界 | B |
| model::physics | PhysicsModuleSpec、ModelId | 显式物理模块 | B |
| model::output | OutputProductSpec、EncoderSpec | 科学输出产品 | B |

## 6. trajecta-met

建议源码布局：

~~~text
trajecta-met/src/
  lib.rs
  field/
  profile/
  io/
  grid/
  vertical/
  frame/
  derive/
  surface_layer/
  query/
  provenance/
  data_provider/
~~~

### 6.1 field

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| CanonicalField | enum | 内置稳定字段ID | A |
| ExtensionFieldId | struct | 命名空间扩展字段 | B |
| FieldKey | enum | Canonical或Extension | B |
| FieldDescriptor | struct | 单位、形状、层型、插值语义 | A |
| FieldShape | enum | Scalar、Horizontal2D、Full3D、Interface3D | B |
| FieldQuality | enum | Source、Derived、Estimated | B |
| Capability | enum | Transport、PBL、Convection、WetDeposition、DryDeposition、DomainFill、Diagnostics | B |
| CapabilitySet | bitset struct | 一次运行需要的能力集合 | B |
| FieldRegistry | struct | 核心和扩展字段注册 | A |

核心风险：

- CanonicalField 语义一旦发布不能重定义；
- U/V、W和层交错必须清楚区分；
- 扩展字段不得覆盖canonical名称；
- 能力集必须能给出完整缺项报告。

### 6.2 profile::document

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| DatasetProfile | struct | 档案完整文档 | B |
| ProfileName | struct | 当前加载范围唯一名称 | C |
| DatasetFingerprint | struct | 中心、产品、全局属性等精确指纹 | B |
| SourceMatcher | enum | GRIB或NetCDF精确匹配 | B |
| FieldMapping | struct | 源变量到目标字段 | B |
| FallbackRule | struct | 允许的回退执行计划 | A |
| ProfileCatalog | struct | 内置和本地档案集合 | B |

ProfileCatalog 规则：

- 名称冲突即失败；
- 指纹必须唯一匹配；
- 自动识别无唯一结果时失败；
- Case可以显式指定档案；
- resolved Case记录最终档案哈希。

### 6.3 profile::graph

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| GraphNodeId | struct | 图节点稳定ID | C |
| GraphNode | struct | 操作、输入和输出声明 | B |
| GraphOp | enum | 白名单计算操作 | A |
| ValueType | struct | 单位、维度、形状和层型 | A |
| ComputationGraph | struct | 未编译有向无环图 | A |
| GraphValidator | struct | 环、类型和单位检查 | A |
| GraphCompiler | struct | 常量折叠、排序和分阶段 | A |
| ExecutionPlan | struct | 可执行批量kernel计划 | A |
| ExecutionStage | enum | Frame、Tile、Column、Sample | A |

GraphCompiler 必须将节点拆到正确阶段：

- Frame：解码后整帧或二维变换；
- Tile：需要水平邻域的派生；
- Column：垂直层结和整柱运算；
- Sample：最终查询高度和地表层模型。

禁止：

- 任意循环；
- 文件和网络访问；
- 系统调用；
- 改变水平拓扑；
- 未声明单位；
- 热路径字符串查找。

### 6.4 io::reader

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| MetReader | trait | 格式无关读取接口 | A |
| ReaderFactory | struct | 根据lockfile和档案选择读取器 | B |
| SourceIndex | trait | 消息或变量索引 | B |
| DecodedField | struct | 规范化字段数组和元数据 | B |
| DecodeError | enum | 类型化读取错误 | B |

MetReader 主要方法：

~~~text
inspect(file) -> SourceMetadata
build_index(file) -> SourceIndex
decode(request) -> DecodedField
~~~

读取器只负责：

- 精确定位源字段；
- 解码；
- 恢复缺测掩膜；
- 规范扫描顺序；
- 输出源单位和元数据。

读取器不负责：

- Case语义；
- 粒子查询；
- 物理回退；
- 自动猜字段。

### 6.5 io::grib

| 类型 | 作用 | 难度 |
|---|---|---:|
| GribReader | GRIB1/2读取器 | B |
| GribFileIndex | 消息偏移和精确参数索引 | B |
| GribFieldIdentity | 中心、表、参数、层型、层号 | B |
| GribGridDecoder | 规则经纬网格定义 | B |
| HybridPvDecoder | 读取ECMWF PV A/B | A |
| GribBitmap | 缺测掩膜 | B |

高风险点：

- GRIB1 IBM float；
- GRIB2 constant packing；
- JPEG2000 bitsPerValue=0；
- 参数表和本地表；
- 扫描方向；
- hybrid完整PV与数据层子集关系。

### 6.6 io::netcdf

| 类型 | 作用 | 难度 |
|---|---|---:|
| NetCdfReader | NetCDF3/4/HDF5读取 | B |
| NetCdfVariableIndex | 变量、维度和属性索引 | B |
| NetCdfAssembly | 多文件逻辑帧组装 | A |
| CfCoordinateResolver | CF axis、coordinates、formula_terms | A |
| NetCdfMissingMask | FillValue和missing_value | B |
| NetCdfTimeAxis | calendar和时间坐标 | B |

NetCdfAssembly 必须精确比较：

- time；
- longitude/latitude；
- level；
- full/interface坐标；
- grid mapping；
- variable shape；
- 文件角色。

任何不一致都失败，不自动重采样。

### 6.7 io::inventory

| 类型 | 作用 | 难度 |
|---|---|---:|
| MetCatalog | 已验证的数据集目录 | A |
| DomainCatalog | 单个气象域的时次目录 | B |
| FrameDescriptor | 一个逻辑时次的文件集合 | B |
| LogicalFrameId | 域、时次、档案和哈希身份 | C |
| CoverageReport | 时间覆盖和缺口 | B |

MetCatalog 构建阶段提前检查：

- 文件存在和哈希；
- 指纹和档案；
- 所需能力；
- 时间覆盖；
- 最大时距；
- 网格一致性；
- 垂直签名；
- 源缺测；
- 嵌套域关系。

### 6.8 grid

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| GridBackend | trait | 水平网格统一接口 | A |
| RegularLatLonGrid | struct | v0正式网格实现 | A |
| GridPoint | struct | 整数格点索引 | C |
| CellId | struct | 稳定水平单元ID | C |
| HorizontalWeights | struct | 双线性或三角权重 | A |
| DomainGeometry | struct | 范围、分辨率、周期性 | B |
| DomainSelector | struct | 嵌套域选择 | A |
| SphericalBasis | struct | 局地east/north基底 | A |
| RegridPlan | trait | 未来重网格扩展点 | A |

RegularLatLonGrid 必须支持：

- 全球经度周期；
- 经度规范化；
- 纬度边界；
- 日期变更线；
- 极点；
- 一格安全边界；
- cell定位；
- 四角和有效三角形。

球面风：

- 网格经纬度sin/cos可预计算；
- U/V转三维切向矢量；
- 对矢量插值；
- 投影回查询点east/north。

### 6.9 vertical

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| VerticalTopology | enum | HybridPressure或PressureLevels | A |
| VerticalStagger | enum | Full或Interface | B |
| HybridCoefficients | struct | A/B半层系数 | A |
| PressureLevels | struct | 固定压力列表 | B |
| VerticalValidity | struct | 每层有效掩膜 | B |
| ColumnGeometry | struct | 局地pressure/height/density | A |
| ColumnBuilder | trait | 构造局地柱 | A |
| HybridColumnBuilder | struct | hybrid局地曲线柱 | A |
| PressureColumnBuilder | struct | 等压层有效区域柱 | A |
| VerticalBracket | struct | 上下层和权重 | A |

HybridColumnBuilder：

- 水平插值地形和每层坐标；
- 计算半层压力；
- 计算全层压力；
- 计算virtual temperature；
- hydrostatic积分高度；
- 计算密度；
- 验证单调性。

PressureColumnBuilder：

- 使用pressure level和height/geopotential；
- 应用结构化地下掩膜；
- 四角双线性；
- 三角重心插值；
- 有效区域外层不可用；
- 形成可查询局地柱。

### 6.10 frame

| 类型 | 作用 | 难度 |
|---|---|---:|
| RawMetFrame | 不可变源场集合 | A |
| RawFieldStore | SoA字段存储 | B |
| FrameMetadata | 域、时次、网格、档案和provenance | B |
| FramePair | 包围查询时刻的前后帧 | B |
| PreparedWindow | 已固定帧和时间权重 | A |
| WindowManager | 换帧、预取和淘汰 | A |
| FrameCache | 按字节管理原始帧 | A |

硬约束：

- RawMetFrame 构造后不可变；
- PreparedWindow查询期间不得换帧；
- 查询线程不得触发I/O；
- 两帧必须来自同一个选定域；
- 时间方向不改变同一物理时刻的结果。

### 6.11 derive

| Module | 关键类型 | 作用 | 难度 |
|---|---|---|---:|
| derive::thermo | ThermodynamicDeriver | 温度、湿度、密度 | A |
| derive::pressure | PressureDeriver | hybrid/full/interface压力 | A |
| derive::height | HydrostaticHeightDeriver | 位势和高度 | A |
| derive::vertical_velocity | VerticalVelocityDeriver | omega/eta-dot/geometric W | A |
| derive::pv | PotentialVorticityDeriver | PV和梯度 | A |
| derive::precipitation | AccumulationDeriver | 去累计和率 | A |
| derive::cloud | CloudDeriver | 云水和云界 | A |
| derive::surface | SurfaceFieldDeriver | ustar/wstar等回退 | A |
| derive::domain_fill | AirMassDeriver | cell/column干空气质量 | A |

每个Deriver输出：

- 字段数组；
- FieldQuality；
- provenance记录；
- 使用的源字段；
- 回退原因。

### 6.12 surface_layer

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| SurfaceLayerModel | trait | 近地层统一接口 | A |
| SurfaceLayerInput | struct | 查询高度和所有源量 | B |
| SurfaceLayerOutput | struct | 风温湿和质量等级 | B |
| MoninObukhovBusingerDyer | struct | 现代默认模型 | A |
| FlexpartCompatibleSurfaceLayer | struct | 独立兼容实现 | A |
| SurfaceLayerRegistry | struct | 模型ID到实现 | B |

SurfaceLayerModel 必须：

- 纯函数；
- 批量接口；
- 无共享可变状态；
- 不依赖批次整体统计；
- 结果不依赖线程和分块；
- 明确报告回退质量。

### 6.13 query::request

| 类型 | 作用 | 难度 |
|---|---|---:|
| QueryPlan | 请求字段和依赖计划 | A |
| QueryPlanBuilder | 编译字段、能力和派生依赖 | A |
| QueryBatch | 同类垂直坐标批次 | B |
| VerticalQuery | ASL、AGL或Pressure | B |
| QueryPointArrays | longitude、latitude、vertical SoA | B |

QueryPlan 编译阶段必须提前失败：

- 字段不存在；
- 能力缺失；
- 档案无法提供；
- 禁止的Estimated字段；
- 不支持的插值策略；
- surface layer模型缺输入。

### 6.14 query::layout

| 类型 | 作用 | 难度 |
|---|---|---:|
| BatchLayout | 后端中立分组结果 | A |
| DomainGroup | 按域分组 | B |
| CellGroup | 按水平cell分组 | B |
| TileGroup | 按派生tile分组 | B |
| Permutation | 内部顺序与原始顺序映射 | B |
| ChunkPlan | 受预算自动分块 | A |

BatchLayout字段：

~~~text
domain_ids
cell_ids
group_offsets
point_permutation
inverse_permutation
chunk_boundaries
~~~

它是未来GPU后端可复用的主要接口。

### 6.15 query::cache

| 类型 | 作用 | 难度 |
|---|---|---:|
| MemoryBudget | 字节预算和固定资源 | A |
| CacheKey | frame/domain/cell/capability/algorithm | B |
| ColumnCache | 局地柱LRU | A |
| TileCache | halo派生tile LRU | A |
| CacheEntry | 不可变缓存条目 | B |
| PinGuard | 当前批次固定条目 | A |
| CacheMetrics | 命中、淘汰和字节统计 | B |

缓存关键不变量：

- 结果不能依赖缓存命中；
- 查询阶段不改变LRU；
- 当前批条目不能淘汰；
- 淘汰顺序确定；
- 条目实际大小必须可计量；
- 帧失效时相关条目全部失效；
- 不允许临时突破预算。

### 6.16 query::interpolate

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| InterpolationPolicy | trait | 字段插值接口 | A |
| ScalarLinearPolicy | struct | 连续标量 | B |
| CategoricalPolicy | struct | 类别或最近邻 | B |
| SphericalVectorPolicy | struct | 全球风矢量 | A |
| MaskedTrianglePolicy | struct | 等压坡地有效三角形 | A |
| VerticalPolicy | struct | height或log-pressure | A |
| TimePolicy | struct | 瞬时、区间率和离散状态 | A |

Canonical field在FieldDescriptor中固定选择策略。资料档案不能将核心类别量改成普通线性量。

### 6.17 query::engine

| 类型 | 作用 | 难度 |
|---|---|---:|
| MetEngine | 气象运行时总入口 | A |
| BatchWorkspace | 可复用临时内存 | A |
| PreparedBatch | 已固定缓存和分块计划 | A |
| QueryExecutor | 执行各chunk | A |
| ExecutionContext | 调用方并行执行器trait | B |
| RayonExecutionContext | CLI默认实现 | B |

主要调用：

~~~text
MetEngine::prepare()
PreparedWindow::prepare_batch()
PreparedBatch::execute()
~~~

MetEngine拥有：

- MetCatalog；
- WindowManager；
- FrameCache；
- ColumnCache；
- TileCache；
- ProfileCatalog；
- FieldRegistry；
- SurfaceLayerRegistry；
- Metrics。

### 6.18 query::output

| 类型 | 作用 | 难度 |
|---|---|---:|
| QueryOutput | SoA结果容器 | B |
| FieldColumn | 一个字段的连续f64数组 | B |
| StatusColumn | 每点状态 | C |
| SampleStatus | Ok及类型化失败原因 | B |
| SampleView | row(index)只读视图 | C |
| ExplainRecord | 域、帧、权重和provenance | B |

SampleStatus 至少包括：

~~~text
Ok
OutOfDomain
BelowGround
AboveModelTop
InvalidVerticalColumn
MissingAtmosphericCoverage
NumericalFailure
~~~

禁止使用奇异值、魔法大数或NaN表示状态。

### 6.19 provenance

| 类型 | 作用 | 难度 |
|---|---|---:|
| ProvenanceId | 紧凑引用ID | C |
| ProvenanceRecord | 来源、公式、回退和档案 | B |
| TransformRecord | 单次单位/派生步骤 | B |
| ProvenanceTable | 帧级去重表 | B |

热输出只保存ProvenanceId。CLI的 --explain 再解析成人类文本，避免每个点重复字符串。

### 6.20 data_provider

| 类型 | 作用 | 难度 |
|---|---|---:|
| DataProvider | 远程或本地资料提供接口 | B |
| LocalDataProvider | v0唯一实现 | C |
| DatasetLocation | 本地root和lockfile | C |
| ProviderError | 类型化数据定位错误 | C |

v0 DataProvider 不下载文件。未来 HTTP、CDS 或对象存储实现不得改变 MetEngine 接口。

## 7. trajecta-core

建议源码布局：

~~~text
trajecta-core/src/
  lib.rs
  clock.rs
  particle.rs
  rng.rs
  integrator/
  boundary/
  population/
  release/
  runner/
  output/
  manifest.rs
~~~

### 7.1 particle

| 类型 | 作用 | 难度 |
|---|---|---:|
| ParticleId | 稳定粒子ID | C |
| ParticleState | 单粒子状态 | B |
| ParticleBatch | SoA粒子数组 | A |
| ParticleStatus | Alive或Terminated | B |
| TerminationReason | 地面、顶层、域外等原因 | B |
| SubstanceMassStore | 粒子物种质量SoA | B |

ParticleBatch 建议使用 SoA：

~~~text
id[]
longitude[]
latitude[]
asl_height[]
age[]
status[]
population_id[]
mass[substance][particle]
~~~

### 7.2 clock

| 类型 | 作用 | 难度 |
|---|---|---:|
| SimulationClock | 正反向模拟时钟 | B |
| SignedDuration | 带方向时间步 | B |
| StepBoundary | 气象帧、释放和输出边界 | B |
| StepPlanner | 将大步拆到事件边界 | A |

同一物理时刻的气象查询不得依赖运行方向。

### 7.3 integrator

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| IntegratorModel | trait | 粒子推进接口 | A |
| IntegratorInput | struct | 当前粒子、时间、步长 | B |
| IntegratorContext | struct | MetEngine、QueryPlan、执行器 | A |
| StepResult | struct | 新位置和状态 | B |
| Rk2Spherical | struct | v0球面中点积分器 | A |

Rk2Spherical 流程：

~~~text
查询起点风
计算半步球面位置和高度
查询半步风
用半步风推进整步
应用边界策略
更新粒子状态
~~~

要求：

- 经纬度推进在球面完成；
- vertical使用geometric W；
- signed dt支持反向；
- 批次和线程不改变结果；
- 不使用随机湍流。

### 7.4 boundary

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| BoundaryPolicy | trait | 粒子边界接口 | A |
| SurfaceReflect | struct | 地面反射 | A |
| ModelTopTerminate | struct | 模式顶终止 | B |
| LimitedDomainTerminate | struct | 有限域出界终止 | B |
| GlobalPeriodicBoundary | struct | 全球周期处理 | A |
| BoundaryContext | struct | 地形、域和时间 | B |

气象层只返回状态，粒子层决定反射、终止或其它行为。

### 7.5 rng

| 类型 | 作用 | 难度 |
|---|---|---:|
| CounterRng | 与线程顺序无关的计数器随机数 | B |
| RandomKey | seed、particle ID、event index | B |
| LowDiscrepancySampler | 释放和domain-fill确定性采样 | B |

即使v0没有随机湍流，也需为释放位置和未来物理预留可重复随机合同。

### 7.6 population

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| PopulationStrategy | trait | 粒子群全生命周期接口 | A |
| PopulationContext | struct | 时间、气象、域和执行器 | A |
| ReleaseDrivenPopulation | struct | 普通释放 | A |
| DomainFillAirMass | struct | 干空气质量填充 | A |
| DomainFillStratosphericOzone | struct | PV/臭氧填充 | A |
| PopulationState | enum | 各策略私有状态 | A |
| BoundaryMassAccumulator | struct | 边界入流残差质量 | A |
| ParticleSeeder | struct | 将质量转成粒子 | A |

PopulationStrategy 生命周期：

~~~text
initialize()
before_step()
emit_particles()
after_advection()
apply_boundary_maintenance()
finalize()
~~~

DomainFillAirMass：

- 计算网格干空气质量；
- 按目标粒子质量或粒子数铺设；
- 全球域不做边界补充；
- 有限域计算法向空气质量通量；
- 入流累计达到粒子质量时生成；
- 出流粒子删除；
- 残余质量跨步保存；
- 正反向按积分方向选择入流。

DomainFillStratosphericOzone：

- 复用air-mass生命周期；
- 查询PV和相关诊断；
- 应用具名臭氧质量规则；
- 与GPL Oracle和公开公式做差分。

### 7.7 release

| 类型 | 作用 | 难度 |
|---|---|---:|
| ReleaseSchedule | 时间计划 | B |
| ReleaseEvent | 一个释放事件 | B |
| GeometrySampler | GeoJSON水平采样 | B |
| VerticalSampler | ASL/AGL/Pressure采样 | B |
| ReleaseAllocator | 质量和粒子数分配 | A |

### 7.8 runner

| 类型 | 作用 | 难度 |
|---|---|---:|
| SimulationRunner | 完整运行编排 | A |
| SimulationState | 时钟、粒子、population和输出状态 | A |
| RunnerBuilder | 从resolved Case创建运行器 | B |
| RunError | 类型化运行错误 | B |

主循环：

~~~mermaid
sequenceDiagram
    participant R as SimulationRunner
    participant P as PopulationStrategy
    participant M as MetEngine
    participant I as IntegratorModel
    participant O as OutputProduct

    R->>P: before_step
    P->>M: domain-fill/release所需查询
    P-->>R: 新粒子
    R->>M: prepare(time)
    R->>M: prepare_batch(particles)
    M-->>I: QueryOutput
    I-->>R: 新粒子位置
    R->>P: boundary maintenance
    R->>O: sample/write
~~~

### 7.9 output

| 类型 | 种类 | 作用 | 难度 |
|---|---|---|---:|
| OutputProduct | trait | 科学输出接口 | B |
| ParticleStateProduct | struct | 粒子状态输出 | B |
| MeteorologyReplayProduct | struct | 气象replay输出 | B |
| JsonlEncoder | struct | v0稳定测试编码 | C |
| CsvEncoder | struct | 人工分析 | C |
| OutputScheduler | struct | 采样和平均时间 | B |

v0 不实现完整生产浓度和沉降输出，但接口必须允许后续产品独立加入。

### 7.10 manifest

| 类型 | 作用 | 难度 |
|---|---|---:|
| RunManifest | 运行记录 | B |
| SoftwareIdentity | crate版本和Git提交 | C |
| InputIdentity | Case、RunProfile、lock和profile哈希 | C |
| ExecutionSummary | 时间、线程、缓存和退出状态 | B |
| NumericalSummary | 模型ID、容差和确定性信息 | B |

## 8. trajecta-cli

建议源码布局：

~~~text
trajecta-cli/src/
  main.rs
  cli.rs
  envelope.rs
  render.rs
  command/
    case.rs
    data.rs
    met.rs
    run.rs
~~~

### 8.1 CLI 类型

| 类型 | 作用 | 难度 |
|---|---|---:|
| Cli | 顶层参数 | C |
| CaseCommand | validate、resolve | C |
| DataCommand | lock、inspect | C |
| MetCommand | probe、replay | B |
| RunCommand | 最小粒子运行 | B |
| CommandEnvelope | 统一JSON输出 | C |
| HumanRenderer | 人类可读输出 | C |
| ExplainRenderer | provenance和插值解释 | B |

CLI 只做：

- 参数解析；
- 文件加载；
- 调用库API；
- 人类/JSON渲染；
- 退出码。

CLI 不实现：

- 气象解析；
- 数值插值；
- 粒子推进；
- 迁移语义；
- 缓存算法。

## 9. GPL 兼容与参考组件

### 9.1 legacy adapter

| 组件 | 作用 | 难度 |
|---|---|---:|
| flexpart-to-case legacy parser | 读取pathnames和传统文件 | B |
| TrajectaTargetRenderer | 生成Trajecta v0 Case | B |
| MigrationReportBuilder | 已映射、保留和未支持项 | B |
| flexctl met adapter | 使用旧Case调用MIT met库 | B |

不得把旧 raw 字段直接塞进 Trajecta Case 逃避建模。

### 9.2 FLEXPART met oracle

| 类型/组件 | 作用 | 难度 |
|---|---|---:|
| met_oracle_driver | 初始化原FLEXPART气象路径 | A |
| OracleQuery | 时间、坐标和字段请求 | B |
| OracleResult | JSON参考结果 | B |
| OracleFixtureManifest | FLEXPART版本、输入哈希和编译参数 | B |
| OracleRunner | 构建并运行Fortran工具 | A |

Oracle必须保持：

- GPL；
- 与MIT crate无链接；
- 输入和输出可审计；
- 参考版本固定；
- 失败时报告原FLEXPART阶段。

## 10. 缓存与所有权关系

~~~mermaid
flowchart TD
    ENGINE["MetEngine"]
    WM["WindowManager"]
    FC["FrameCache"]
    CC["ColumnCache"]
    TC["TileCache"]
    PW["PreparedWindow"]
    PB["PreparedBatch"]
    PG["PinGuard"]
    QE["QueryExecutor"]

    ENGINE --> WM
    ENGINE --> FC
    ENGINE --> CC
    ENGINE --> TC
    WM --> PW
    PW --> PB
    PB --> PG
    PG --> CC
    PG --> TC
    PB --> QE
~~~

所有权规则：

- MetEngine 独占可变缓存管理器；
- PreparedWindow 持有不可变 FramePair；
- prepare_batch 可短暂修改LRU；
- PreparedBatch 持有 PinGuard；
- execute 期间缓存只读；
- QueryOutput由调用方提供并复用；
- BatchWorkspace由调用方或Runner持有，避免反复分配。

## 11. 错误边界

### 11.1 构建期错误

在 MetCatalog 或 QueryPlan 阶段失败：

- 档案冲突；
- 指纹不唯一；
- 文件或哈希错误；
- 时间缺口；
- 网格变化；
- 垂直签名变化；
- 必需字段缺失；
- 计算图错误；
- 能力不满足。

这些错误不得拖到粒子查询阶段。

### 11.2 逐点状态

逐点可恢复状态：

- OutOfDomain；
- BelowGround；
- AboveModelTop；
- MissingAtmosphericCoverage；
- InvalidVerticalColumn。

批量中一个点失败不能阻断其它点。

### 11.3 运行器错误

完整运行失败：

- 无法准备气象窗口；
- 输出写入失败；
- population质量不守恒超过容差；
- 模块执行器不存在；
- 粒子容量或资源约束无法满足。

## 12. 关键实现难点与推荐模型

| 任务 | 难度 | 原因 |
|---|---:|---|
| Case serde与CLI脚手架 | C | 规格明确、机械实现为主 |
| 单位系统与ref解析 | B | 需要稳定错误和规范化输出 |
| GRIB1/2消息索引 | B | 格式复杂但已有库和测试 |
| NetCDF多文件精确组帧 | A | 坐标、层型、时间和缺测组合复杂 |
| 类型化计算图编译器 | A | 单位、形状、阶段拆分和性能同时存在 |
| Hybrid压力/高度局地柱 | A | 科学公式、层子集和数值稳定性 |
| 等压坡地有效三角形 | A | 几何、掩膜和垂直定位耦合 |
| 全球球面风插值 | A | 极点、日期变更线和确定性 |
| 双帧WindowManager | A | I/O、预取、方向和生命周期 |
| Column/Tile LRU | A | 预算、PinGuard、并行和确定性 |
| QueryPlan与SoA输出 | A | 字段依赖、布局和未来GPU边界 |
| Monin–Obukhov模型 | A | 稳定度函数和近地边界 |
| FLEXPART兼容地表层 | A | 独立重写与差分校准 |
| CLI met probe | C | 依赖稳定库接口 |
| Meteorology replay | B | 编排、输出和夹具 |
| 球面RK2粒子积分 | A | 球面几何、时间插值和边界 |
| Release-driven population | B | 调度和几何采样 |
| Domain-fill air mass | A | 质量守恒和边界持续补充 |
| Stratospheric ozone domain-fill | A | PV、臭氧公式和参考验证 |
| RunManifest | B | 多来源身份与可复现信息 |
| GPL met oracle | A | Fortran全局状态、构建和许可隔离 |
| 文档和普通单元测试 | C | 稳定接口下的常规工作 |

## 13. 实施顺序与任务分派

### 阶段 1：接口和基础类型

先由 A 模型冻结：

- crate边界；
- 依赖方向；
- Case顶层；
- CanonicalField；
- Query API；
- ParticlePopulation生命周期。

随后可分派：

- C：CLI、简单serde、文档骨架；
- B：quantity、diagnostic、ref、lockfile；
- B：迁移报告结构。

### 阶段 2：资料与档案

- A：计算图类型系统和ExecutionStage；
- B：GRIB Reader；
- A：NetCDFAssembly；
- B：ProfileCatalog和精确匹配；
- C：内置档案数据文件的机械录入；
- B：档案schema测试。

### 阶段 3：气象数值核心

必须由 A 模型完成：

- GridBackend与球面风；
- HybridColumnBuilder；
- PressureColumnBuilder；
- Deriver系统；
- SurfaceLayerModel；
- DomainSelector；
- WindowManager；
- QueryPlan；
- Column/Tile缓存。

可由 B/C 模型辅助：

- 数组容器；
- 指标采集；
- JSON explain渲染；
- 合成夹具生成器。

### 阶段 4：粒子闭环

必须由 A 模型完成：

- Rk2Spherical；
- BoundaryPolicy；
- SimulationRunner；
- DomainFillAirMass；
- DomainFillStratosphericOzone。

B 模型可以完成：

- ReleaseSchedule；
- GeometrySampler；
- RunManifest；
- 基础OutputProduct。

### 阶段 5：验收

- A：FLEXPART Oracle、容差分类、科学差异审计；
- B：真实数据脚本、replay和CI编排；
- B：benchmark与缓存指标；
- C：测试报告和使用文档。

## 14. 每个模块的完成定义

一个模块只有同时满足以下条件才算完成：

- 公共类型和方法有文档；
- 无未解释的FLEXPART变量名泄漏；
- 错误使用类型化诊断；
- 单元测试覆盖正常与边界路径；
- 与上下游模块有集成测试；
- 所有数组形状和单位明确；
- 并行结果可重复；
- 无魔法缺测值；
- 无隐藏文件I/O；
- 性能指标可观测；
- A模块有独立数值基准或解析测试；
- 许可来源和参考资料记录清楚。

## 15. 最重要的接口冻结点

实现过程中最不应轻易变化的合同：

1. Case 与 RunProfile 的职责分离；
2. CanonicalField 的物理语义；
3. DatasetLock 的内容身份；
4. DatasetProfile 的精确匹配和计算图边界；
5. MetEngine prepare 与 query 分离；
6. QueryBatch 单一垂直坐标；
7. QueryOutput SoA；
8. BatchLayout 后端中立；
9. SampleStatus 不使用奇异值；
10. SurfaceLayerModel 纯批量函数；
11. PopulationStrategy 全生命周期；
12. GPL 到 MIT 的单向依赖。

这些接口冻结后，读取器、档案、CLI、输出和未来GPU后端才可以安全并行开发。
