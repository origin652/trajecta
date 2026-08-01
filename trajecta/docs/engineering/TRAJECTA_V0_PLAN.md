# Trajecta v0 实施计划

## 1. 目标

Trajecta 是一个新的、MIT 许可的拉格朗日输送模型核心。FLEXPART 不再是新核心的架构中心，只承担三类角色：

- 旧配置与案例迁移来源；
- GPL 兼容工具；
- 数值行为和真实业务输入的参考基准。

Trajecta v0 的交付终点包括：

- 读取 GRIB1/2、NetCDF3、NetCDF4/HDF5 气象资料；
- 通过精确资料档案统一 ECMWF/ERA5、NOAA/NCEP CFSR 和私人资料；
- 保留原生 hybrid 或等压垂直层并提供可查询气象引擎；
- 支持滚动双帧、嵌套域、派生量、时空插值、缓存和批量查询；
- 实现最小粒子闭环，包括确定性平流、普通释放和 domain-filling；
- 建立真实资料、FLEXPART 参考、性能和正反向运行测试。

本计划不要求生成传统 FLEXPART 配置文件。

## 2. Workspace 与许可边界

在仓库顶层建立独立的 Trajecta workspace：

~~~text
trajecta/
  Cargo.toml
  LICENSE-MIT
  crates/
    trajecta-case/
    trajecta-met/
    trajecta-core/
    trajecta-cli/
  profiles/
  schemas/
  docs/engineering/
~~~

依赖方向必须保持：

~~~text
trajecta-cli
  ├─> trajecta-core
  ├─> trajecta-met
  └─> trajecta-case

trajecta-core
  ├─> trajecta-met
  └─> trajecta-case

trajecta-met
  └─> trajecta-case

GPL flexctl / flexpart-to-case
  └─> MIT Trajecta crates
~~~

MIT crate 不得反向依赖 GPL crate。

现有 Rust 代码只能在完成版权和派生关系审计后迁入 Trajecta：

- 确认由本项目拥有版权、且没有复制或逐行翻译 FLEXPART 的通用代码，可重新授权为 MIT；
- 含 GPL 派生内容的实现继续留在兼容区；
- 无法确认的代码在 Trajecta 中独立重写；
- FLEXPART 算法可以用于理解行为、建立数学规格和生成测试夹具，但不得直接复制代码、注释和原模块组织。

## 3. Case 与 RunProfile

### 3.1 文档类型

Trajecta v0 使用：

~~~yaml
schema_version: 0
kind: case
~~~

或：

~~~yaml
schema_version: 0
kind: run_profile
~~~

schema_version 0 表示开发期格式。字段变化仍需评审、同步更新 schema、转换器和夹具，但在首个业务版本前不承诺 v0 文档的长期迁移兼容。首个业务版本冻结为 v1。

### 3.2 Case 顶层

Case 使用组件化结构：

~~~text
metadata
time
meteorology
particle_population
substances
numerics
physics
outputs
~~~

组件可以内联，也可以通过 ref 引用本地文件。运行前必须生成完全展开的 resolved-case.json，并记录所有来源文件的 SHA-256。

Case 允许部分定义：

- met probe 只要求 time 和 meteorology；
- 完整运行还要求 particle_population、numerics 和所需输出；
- 对应物理模块启用时，才要求相关 substances 与 physics 配置；
- 验证必须按意图报告缺失组件，不能要求无关占位字段。

### 3.3 科学配置与机器配置分离

Case 保存：

- 时间范围和正反向方向；
- 资料产品与解释档案；
- 气象域关系；
- 粒子群体策略；
- 数值算法；
- 物理模块；
- 输出产品。

RunProfile 保存：

- Case 路径；
- 逻辑数据集到本地 lockfile 和缓存目录的映射；
- 线程与执行器；
- 内存预算；
- 预取策略；
- 临时目录和输出根目录。

### 3.4 物理量与引用

输入同时接受：

~~~yaml
step: 10 min
~~~

和：

~~~yaml
step:
  value: 10
  unit: min
~~~

resolved JSON 始终写成 value 加 unit 对象。

释放的水平几何使用 GeoJSON，垂直部分单独声明 ASL、AGL 或压力坐标。

物种改为具名 substances，不再以 SPECIES_nnn 文件编号作为新核心身份。

输出采用具名 output products 列表，物理量、采样方式、空间目标和编码器分开表达。

### 3.5 旧 YAML 迁移

实现必须遵循 [FLEXPART 到 Trajecta v0 迁移合同](FLEXPART_TO_TRAJECTA_MIGRATION_CONTRACT.md)。
该合同冻结 `--target trajecta` 的字段映射、部分 Case 行为、机器可读迁移报告和 GPL/MIT
单向依赖边界。

flexpart-to-case 增加：

~~~text
--target flexctl
--target trajecta
~~~

legacy 解析器只保留一份。

转换到 Trajecta 时：

- 可明确映射的科学语义进入新 Case；
- 暂无执行器但已有中立合同的模块进入 Case，并标记不可运行原因；
- 纯重复 legacy 镜像、raw_value、旧日期副本和传统文件边界直接丢弃；
- 未映射内容进入独立迁移报告；
- 原 YAML 的路径和哈希进入迁移报告，不进入新 Case；
- MIT trajecta-cli 不直接链接 GPL 旧 Case 解析器。

## 4. 资料档案与数据锁定

### 4.1 档案原则

资料档案是受 schema 校验的配置与计算图文件，负责：

- 文件指纹；
- GRIB 参数或 NetCDF 变量的精确匹配；
- 坐标和维度映射；
- 单位转换；
- 时间语义；
- canonical field 映射；
- 派生公式；
- 回退规则。

禁止名称模糊匹配和相似度打分。

档案名称只需在当前加载范围内唯一：

- 内置与本地档案同名时直接失败；
- 不按搜索路径顺序覆盖；
- 不强制档案名称携带人工版本号；
- resolved Case 记录档案内容 SHA-256；
- 档案历史由普通版本控制管理。

私人档案是一等输入：

- 不显示科学可信度警告；
- 与内置档案执行相同的 schema、单位、形状、有限值和物理不变量检查；
- 科学适用性由使用者和论文评审负责。

### 4.2 类型化计算图

档案公式使用类型化有向无环图。v0 至少支持：

- 常量、别名和单位换算；
- 加减乘除、幂、对数、指数、平方根；
- min、max、clamp、where；
- 垂直 midpoint、difference、gradient；
- 垂直 cumulative scan、积分与归约；
- hybrid pressure；
- hydrostatic height；
- virtual temperature、density、potential temperature；
- 湿度形式转换；
- 累计量去累计和区间率；
- full/interface 层之间的受控转换。

加载档案时必须：

- 检查图无环；
- 检查单位与维度；
- 检查输入字段存在；
- 做常量折叠；
- 拓扑排序；
- 编译成批量数组执行计划。

热路径不得解析 JSON、查字符串或逐元素解释图节点。

v0 计算图不得改变水平网格。预留 GridBackend 与 RegridPlan 接口，后续通过正式后端支持 Gaussian、投影和守恒重网格。

### 4.3 数据锁文件

正式运行必须使用 dataset lockfile。lockfile 至少记录：

- 逻辑数据集 ID；
- 数据档案名称和 SHA-256；
- 文件清单；
- 每个文件的角色、时次、大小和 SHA-256；
- 网格和垂直坐标签名；
- 数据来源 URL 和归属信息；
- 生成工具版本。

probe 可以临时扫描目录；正式 run 不允许仅依赖目录当前内容。

v0 只实现 DataProvider 接口和 LocalDataProvider。网络获取由现有数据脚本完成，运行时只读本地锁定缓存。

## 5. 气象资料支持

### 5.1 v0 格式

- GRIB1；
- GRIB2；
- NetCDF3；
- NetCDF4/HDF5；
- 单文件多变量；
- 多文件按变量、层型或时间精确组装逻辑帧。

首批内置档案：

- ECMWF ERA5 / flex_extract GRIB hybrid；
- ECMWF ERA5 CF-NetCDF pressure levels；
- ECMWF ERA5 hybrid CF-NetCDF4 transcoded；
- NOAA/NCEP CFSR pgbl GRIB2；
- NOAA PSL NCEP/NCAR Reanalysis 1 multi-file CF-NetCDF（官方多文件兼容性验证；不是 CFSR）。
- CFSR 派生多文件 CF-NetCDF 装配夹具（由官方 CFSR GRIB2 确定性转换，不得冒充 PSL 原件）。

### 5.2 网格与域

v0 正式实现规则经纬网格：

- 地区网格；
- 全球周期经度；
- 日期变更线；
- 南北极；
- 母域与任意数量嵌套域。

meteorology.domains 是通用数组。每个域包含：

- 稳定 ID；
- 逻辑数据集引用；
- 优先级；
- 可选父域；
- 默认一格安全边界；
- 可选域级档案覆盖。

域选择规则：

- 优先最高优先级域；
- 查询点必须距离域边界至少一个可插值网格单元；
- 嵌套域必须同时具备前后两个气象帧；
- 任一时间端缺帧时，两个时间端整体回退低优先级域；
- 不在时间插值两端混用不同域；
- 各域允许使用不同层数和不同垂直类型。

### 5.3 垂直层结

保留原生：

- hybrid pressure；
- pressure levels；
- full levels；
- interface levels；
- 实际资料层子集；
- 原始层号和 A/B 半层系数。

查询支持：

- ASL；
- AGL；
- Pa。

同一个批次只允许一种垂直坐标类型。

Hybrid 查询：

- 先计算查询点局地地形；
- 在每个原生层上水平插值高度、压力和字段；
- 形成局地曲线柱；
- 检查高度和压力严格单调；
- 在局地柱内定位与插值。

等压资料：

- 允许地面以下压力层使用结构化缺测掩膜；
- 四角有效时使用双线性；
- 三角有效区内使用重心插值；
- 查询点不在有效三角形内时，该层不可用；
- 不制造地下填充值。

地形以下返回 BelowGround，模式顶以上返回 AboveModelTop，不夹层、不外推。

### 5.4 Canonical fields

Canonical field 覆盖所有已知运行需求：

- 坐标与几何：terrain、land fraction、cell area、roughness；
- 层结：full/interface pressure、height、density、dry-air density；
- 输送：east/north wind、geometric vertical velocity、native vertical velocity；
- 热力：temperature、specific humidity、relative humidity、potential temperature；
- 诊断：PV、tropopause、density gradient；
- PBL：boundary-layer height、ustar、wstar、Monin-Obukhov length；
- 地表：2 m 温湿、10 m 风、surface pressure、heat flux、surface stress；
- 云与对流：cloud liquid、cloud ice、cloud total、cloud cover、cloud bounds；
- 降水：large-scale、convective、snow 和总降水率；
- 沉降辅助：snow depth、land/surface properties、radiation；
- domain-fill：cell volume、dry-air cell mass、column mass。

核心字段的含义、单位、形状和插值语义不可由档案改写。

私人档案可以发布命名空间扩展字段。扩展字段必须声明单位、形状和插值策略。

## 6. 查询引擎

### 6.1 数据模型

- 字段按 SoA 分离；
- 三维源场使用 cell-major、level-contiguous 布局；
- 原始和缓存字段使用 f32；
- 坐标、权重、层结计算和查询结果使用 f64；
- 帧准备完成后不可变；
- 查询阶段无文件 I/O。

### 6.2 生命周期

~~~text
MetCatalog
  -> MetEngine.prepare(time, capabilities)
  -> PreparedWindow
  -> prepare_batch(points, QueryPlan, BatchWorkspace)
  -> PreparedBatch
  -> execute(ExecutionContext, QueryOutput)
~~~

prepare 固定包围查询时刻的前后两帧。窗口管理器可以在预算允许时预取下一帧。

QueryPlan：

- 使用强类型 canonical fields；
- 只准备和返回请求字段；
- 编译并复用定位与派生依赖；
- 单点接口只是批量接口的一点包装。

QueryOutput 使用 SoA：

- status；
- 每个请求字段的连续数组；
- 可选 domain ID；
- 可选 provenance ID；
- row(index) 只读视图。

### 6.3 缓存

缓存分为：

- 原始帧缓存；
- 局地垂直柱缓存；
- 带一圈 halo 的派生 tile 缓存。

局地柱键包含：

~~~text
frame_id
domain_id
cell_id
capability_set
algorithm_version
~~~

规则：

- prepare_batch 阶段查找或构造；
- 当前批次引用的条目固定，不能淘汰；
- 查询阶段只读；
- 按实际字节数 LRU 淘汰；
- 帧离开窗口时关联缓存整体失效；
- 缓存命中与否不得改变结果。

需要水平导数的 PV、坡度和相关派生量使用 tile 加 halo 计算。

单批次覆盖单元过多时：

- 按 cell 自动分块；
- 每块准备、查询并写回原始输出位置；
- 不临时突破预算；
- 分块大小不得影响数值结果。

默认自动内存规划：

- 先估算必须固定的前后源帧；
- 为粒子与其它模块保留约一半可用内存；
- 剩余空间分配给局地柱、tile 和预取；
- 双帧无法安全放入时提前报告所需内存；
- RunProfile 可显式覆盖。

### 6.4 插值

- 连续标量采用字段指定的连续插值；
- 类别量采用离散或最近邻策略；
- 全球 U/V 统一转为地心三维切向矢量后插值，再投影到查询点局地基底；
- 累计量先去累计，运行主字段提供区间平均率；
- 前后帧分别完成水平和垂直查询，最后做时间插值；
- 每个点使用固定运算顺序；
- 线程数、内部排序、分块和缓存状态不得改变逐点结果。

### 6.5 地表层

提供 SurfaceLayerModel 接口和两个内置实现：

- Monin–Obukhov / Businger–Dyer；
- 独立编写的 FLEXPART-compatible 模型。

模型输入包括：

- 查询高度；
- 地形；
- 10 m 风；
- 2 m 温湿；
- surface pressure；
- roughness；
- ustar、wstar；
- Monin–Obukhov length；
- heat flux 和 surface stress；
- 最低三维模式层。

模型输出近地风、温度、湿度及质量等级。

用户可在源码工作区实现新模型并重新编译。v0 不支持动态库或 WASM 加载。

## 7. 最小粒子闭环

### 7.1 粒子状态

粒子至少保存：

- stable particle ID；
- longitude、latitude；
- ASL height；
- mass per substance；
- age；
- source/population ID；
- alive/terminated status；
- termination reason。

AGL 和压力释放在初始化时通过气象快照转换为 ASL。

### 7.2 积分器

内置 rk2_spherical/v0：

- 球面中点法；
- 使用 east/north wind 和 geometric W；
- 支持正向和反向 signed dt；
- 时间步在气象帧、释放事件和输出事件边界切分；
- 无随机湍流；
- 不依赖批次顺序。

默认边界：

- 地面反射并加入安全偏移；
- 模式顶以上终止；
- 有限域水平出界终止；
- 全球经度周期处理；
- 终止原因进入输出与运行记录。

### 7.3 粒子群体策略

ParticlePopulation 是贯穿运行全程的策略接口。

v0 实现：

- release_driven；
- domain_fill.air_mass；
- domain_fill.stratospheric_ozone。

release_driven：

- 按 GeoJSON、释放时段、质量和粒子数确定性生成；
- 支持连续和瞬时释放；
- 使用明确随机种子或确定性低差异采样。

domain_fill：

- 按干空气网格质量初始化粒子；
- 全球域初始化后持续推进；
- 有限域按边界法向风、密度和面面积计算入流空气质量；
- 质量累计达到一个粒子质量时生成新粒子；
- 不足一个粒子质量的残差跨时间步保存；
- 出流粒子删除；
- 正反向运行使用积分方向对应的入流边界；
- ozone 变体使用 PV 和具名臭氧规则分配质量。

## 8. CLI

v0 至少提供：

~~~text
trajecta case validate <case>
trajecta case resolve <case> --out resolved-case.json
trajecta data lock <run-profile>
trajecta met probe <case> --run-profile <run>
trajecta met replay <case> --run-profile <run>
trajecta run <case> --run-profile <run>
~~~

met probe：

- 支持单点参数；
- 支持 JSONL/CSV 批量点；
- 默认输出状态和字段值；
- --explain 输出域、前后帧、权重、档案、来源/派生链和缓存信息；
- --json 输出稳定结构。

## 9. 真实数据与参考工具

真实数据不提交 Git，统一由脚本下载、缓存和校验。

ERA5：

- 现有 flex_extract 8 时次 hybrid GRIB；
- CDS 直接获取的等压 CF-NetCDF；
- 若 CDS 无直接 hybrid NetCDF，则从真实 model-level GRIB 无垂直插值转码为 CF-NetCDF4；
- 转码只改变容器，不改变层号、网格或字段值；
- GRIB 解码值与转码文件逐项核对；
- A/B 系数保留为 f64；
- 文件明确标记 transcoded，不冒充机构直接发布。

CFSR：

- 现有两个时次 pgbl GRIB2；
- NOAA PSL pressure-level 多文件 CF-NetCDF；
- pressure、temperature、U/V、humidity、omega 和 height 精确组帧。

独立 GPL FLEXPART 气象参考工具：

- 调用原 readgrid/readwind、verttransform、getfields 和 interpol 路径；
- 输入真实文件、时间、坐标和字段请求；
- 输出 JSON 数值夹具；
- 记录 FLEXPART 提交、编译选项和输入哈希；
- MIT Trajecta 只读取 JSON，不链接 GPL 代码。

## 10. 测试与验收

### 10.1 快速测试

- Case/RunProfile schema；
- 单位与引用解析；
- 迁移报告；
- 档案计算图；
- GRIB/NetCDF小型夹具；
- hybrid 与等压层解析；
- 全球经度、极点和球面风；
- 嵌套域选择；
- 地形以下、模式顶以上和非法层柱；
- 累计重置；
- 地表层；
- 缓存与自动分块；
- 不同线程数和分块大小确定性；
- 粒子解析轨迹；
- domain-fill 质量守恒。

### 10.2 真实 replay

- ERA5 21 小时全时段；
- CFSR 全球案例；
- 正向和反向都必须运行；
- 同一物理时刻的气象查询值与运行方向无关；
- 覆盖 prepare、换帧、预取、LRU、tile、查询和输出；
- 不用简化粒子模型代替气象 replay。

### 10.3 数值基准

差分项分为：

- 兼容项：原始解码、单位、层压、层高和明确复刻公式，超容差失败；
- 现代项：局地曲线柱、球面风等，以解析场和物理不变量验收，同时输出 FLEXPART 差异报告。

容差：

- 按变量同时设置绝对和相对容差；
- 根据源数据量化、独立 f64 基准和 FLEXPART 差分校准一次；
- 校准表评审后提交；
- 测试不得自动扩大容差。

### 10.4 性能

硬性结构门槛：

- 工作区预热后无逐点堆分配；
- 不突破内存预算；
- 批量路径明显优于单点循环；
- 缓存命中与自动分块可观测；
- profiler 可区分解码、构柱、tile、垂直定位、插值和缓存耗时；
- 固定 CI 机器基准退化超过 20% 时失败。

CI：

- Pull Request：合成测试和小夹具；
- 夜间或手动：真实下载、CDS、NetCDF转码、GPL参考差分和性能；
- CDS 凭据只从标准用户配置或 CI secret 读取，不写入仓库和运行记录。

## 11. 实施里程碑

### M1：Workspace、许可与 Case

- 建立 MIT workspace；
- 完成 Case/RunProfile v0；
- 完成 quantity、ref、resolved document 和 dataset lock；
- 完成 flexpart-to-case 新目标和迁移报告。

### M2：档案、计算图与原生读取

- Canonical fields；
- 资料档案 schema；
- 计算图编译；
- GRIB1/2；
- NetCDF3/4；
- 多文件逻辑帧；
- 本地 DataProvider。

### M3：气象查询引擎

权威实施计划与验收标准见 `TRAJECTA_M3_MET_QUERY_PLAN.md`，A/B/C 模型任务边界见
`TRAJECTA_M3_MODEL_ASSIGNMENT.md`。

- 滚动窗口；
- 域选择；
- hybrid 与等压局地柱；
- tile 派生；
- 地表层；
- QueryPlan、SoA、缓存、自动分块和执行器。

### M4：最小粒子闭环

权威实施计划与验收标准见 `TRAJECTA_M4_PARTICLE_LOOP_PLAN.md`，A/B/C 模型任务边界见
`TRAJECTA_M4_MODEL_ASSIGNMENT.md`。

- 粒子数据结构；
- 球面 RK2；
- 边界策略；
- release-driven；
- 两类 domain-fill；
- 最小输出。

### M5：CLI、真实数据与验收

- probe/replay/run；
- FLEXPART GPL 参考工具；
- ERA5/CFSR真实矩阵；
- 正反向测试；
- 容差表；
- benchmark 和 profiler。

## 12. v0 明确不做

- GPU执行后端；
- 动态模块加载；
- Gaussian网格；
- 投影网格；
- 实际重网格；
- 运行时远程I/O；
- 传统FLEXPART配置生成；
- 湍流执行器；
- 对流执行器；
- 干湿沉降执行器；
- 化学执行器；
- 完整生产输出体系。

这些方向只保留稳定接口，不在 v0 中以空实现伪装为已支持。
