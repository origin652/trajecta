# Trajecta M4：最小粒子闭环实施与验收计划

状态：M4-A0、M4-A1、M4-A2、M4-A3 已完成；M4-A4 尚未完成。本文不代表 M4 已完成。
日期：2026-07-24
上游基线：M3 已完成并提交，气象查询引擎、三套真实资料与 Windows/WSL 验收可供 M4 复用。
模型分工：见 `TRAJECTA_M4_MODEL_ASSIGNMENT.md`。
M4-A0 冻结公式、常量、错误码与机器 schema：见 `TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`。
M4-A1 A 级核心交付：见 `TRAJECTA_M4_A1_A_DELIVERY_REPORT.md`；B 工程接续任务见 `B_PROMPT_M4_A1_ENGINEERING.md`。

## 1. 目标

M4 交付一个可由 Rust 库直接调用的最小拉格朗日粒子模拟闭环：

- 球面 RK2 正向和反向输送；
- 普通 release-driven population；
- 干空气质量 domain-fill；
- 平流层臭氧 domain-fill；
- 地面、模式顶、有限域和全球周期边界；
- 可扩展的积分器、臭氧规则和输出 sink；
- 默认 SQLite 粒子轨迹输出；
- ERA5 pressure、ERA5 hybrid、CFSR 的真实资料链；
- Windows 与 WSL 的实际运行证据。

M4 首先交付库闭环和测试/示例入口。公开 `trajecta run` CLI 留到 M5。

## 2. 明确不做

M4 不实现：

- 随机湍流执行器；
- 对流、干沉降、湿沉降和化学执行器；
- 生产浓度或沉降网格输出；
- checkpoint/resume；
- 动态 DLL、SO、WASM 或不稳定 ABI 插件；
- 直接读取资料原生臭氧场的正式规则；
- 包围南极或北极的 release Polygon；
- 长时段业务矩阵和 M3 百万点默认测试；
- 公开运行 CLI。

上述方向只能保留明确扩展接口，不得以 `NotImplemented` 的公开实现冒充支持。

## 3. 完成定义

M4 只有同时满足以下条件才算完成：

1. 普通 release、air-mass domain-fill 和 ozone domain-fill 三条链全部闭合；
2. 支持范围内没有可达的 `NotImplemented`；
3. 解析解、物理不变量、边界、质量守恒和输出合同全部通过 hard gate；
4. 三套真实资料的正向和反向矩阵实际运行；
5. Windows 与 WSL 门禁实际运行；
6. Rust/native 小规模轨迹差分实际运行；
7. WSL ERA5 hybrid 10 万粒子端到端长测实际运行；
8. SQLite schema、manifest、数据哈希和性能报告可自校验；
9. 异常数值终止计数为 0；
10. A 模型完成最终科学审计并书面签署。

普通出流、有限域离开和模式顶终止属于正常粒子生命周期，不计为异常数值终止。

## 4. Case 与 RunProfile 合同

### 4.1 随机种子

`NumericsSpec` 增加可选 `random_seed: u64`：

- 用户给定 seed 时必须精确复现；
- 缺 seed 时使用系统熵生成；
- 生成值必须在释放首个粒子前写入 RunManifest；
- seed 不得来自线程 ID、系统时间迭代顺序或全局顺序随机流。

内置计数器随机算法冻结为 `philox4x32-10/v1`。随机键至少包含：

~~~text
seed
population_id
release_event_id / domain-fill lifecycle event
particle_id or stable particle ordinal
sampling_dimension
draw_index
~~~

字符串 ID 使用稳定摘要，不得使用 Rust 进程随机化 Hash。

### 4.2 普通 release

删除 `ReleaseDrivenSpec.schedule: String` 占位合同，改为显式 event 列表。每个 event 至少包含：

- 稳定 event ID；
- start/end 物理时刻；
- 粒子数；
- 各 substance 的总释放质量；
- 水平 GeoJSON 几何源；
- 垂直坐标与固定值或上下界。

规则：

- start=end 表示瞬时释放；
- start<end 表示连续释放；
- M4 不内建 recurrence、cron 或日历重复；
- 上层工具可把重复源展开成显式 event；
- 每个 event 只填各物质总质量，不填质量速率；
- 各物质共用同一批粒子，每个粒子携带该物质总质量的等份；
- 需要不同位置、时段或垂直分布时必须拆成不同 event；
- event 必须完整落在模拟物理时间范围内；
- duplicate ID、零粒子、负质量、非有限质量和全零质量硬失败。

连续释放将时段按粒子数等分，在每个时间格内用 `integer_stratified_birth/v1` 的 64-bit 乘高映射采样一次。出生时刻预先固定，不受积分步、线程或分块影响。

### 4.3 GeoJSON

一个 event 的几何可内联或引用外部 `.geojson`，二选一：

- 外部路径相对 Case 解析；
- 解析后的绝对路径、size 和 SHA-256 进入 manifest；
- 禁止远程 URL 和运行时下载。

支持：

- Point / MultiPoint；
- LineString / MultiLineString；
- Polygon / MultiPolygon。

拒绝：

- GeometryCollection；
- 非有限坐标；
- 退化环、自交环和零面积面；
- 包含南极或北极的 Polygon。

跨越 ±180° 的 Polygon 必须由 M4 自动消歧和切分：

- 用户不需要预切 MultiPolygon；
- 原始输入不改写；
- resolved Case 保存规范化后的 MultiPolygon；
- 切分前后球面面积必须在冻结容差内相等；
- 洞的归属、规范排序和哈希必须稳定；
- 采样点必须证明始终位于原始球面区域内。

采样规则：

- MultiPoint 各点等权；
- Line/MultiLine 按球面长度加权；
- Polygon/MultiPolygon 按球面面积加权并排除洞；
- 不把经纬度直接当作平面距离或平面面积。

### 4.4 垂直释放

支持以下坐标：

- ASL geometric height；
- AGL geometric height；
- pressure Pa。

每个 event 可声明固定值，或在声明坐标内均匀采样。非均匀垂直源通过多个分层 event 及质量比例表达，不在单个 event 内加入自动密度加权或复杂剖面。

水平位置和出生时刻确定后，必须查询当地地表和模式顶：

- AGL 转换为 ASL；
- Pa 通过正式垂直柱转换为 ASL；
- 任一生成样本落入地下、模式顶以上、无效气象柱或域外时，整个 event 硬失败；
- 禁止 clamp、重采、丢弃粒子或减少释放质量。

### 4.5 Domain-fill

`DomainFillAirMassSpec` 必须增加 `domain_id`，并且只允许：

~~~text
target_particle_count
target_dry_air_mass_per_particle
~~~

二选一。

M4 只填充该单一气象域扣除水平 halo 后的完整安全核心：

- 全球周期域不做水平边界补充；
- 有限域在完整网格边界面计算入流；
- 不支持任意 GeoJSON 子域或斜切边界；
- 嵌套域不得猜测并集或重复计算重叠空气质量。

臭氧 spec 复用 air-mass 配置，并增加：

- `ozone_rule`；
- `ozone_substance`。

臭氧 `target_particle_count` 表示筛选后平流层的精确初始粒子数，不复刻 FLEXPART “先生成候选再丢弃”的不确定数量行为。

### 4.6 输出配置

输出 schedule 改为明确枚举：

~~~text
endpoints
interval { interval, origin? }
~~~

语义：

- `endpoints` 保存出生/模拟开始和结束/终止；
- `interval` 保存所有仍存活粒子在每个输出时刻的状态；
- interval 默认从模拟 start 对齐；
- 可显式给 UTC origin；
- start、end、birth 和 termination 始终保存；
- 重合记录去重；
- particle-state 输出拒绝 averaging interval。

Case 未声明 outputs 时，解析器自动补充 `particle_state_sqlite/v1 + endpoints`，补充后的配置进入 manifest。

`RunProfileDocument` 增加必填 `output_root`。

## 5. Core 公共 API

### 5.1 粒子状态

`ParticleBatch` / `ParticleState` 至少增加：

- stable particle ID；
- population ID；
- typed origin：release event、domain initial、domain boundary；
- birth physical time；
- longitude、latitude、ASL geometric height；
- 有符号 `integration_offset_ns`；
- 非负 `elapsed_age_ns`；
- dry-air carrier mass；
- substance-major mass；
- typed alive/termination status。

反向运行只改变物理时钟和 integration offset 符号，不产生负物理质量。未来源—受体灵敏度的正负权重必须使用独立字段。

### 5.2 插件合同

冻结以下编译期接口：

- `IntegratorModel`；
- `BoundaryPolicy`；
- `PopulationStrategy`；
- `OzoneAssignmentRule`；
- `OutputProduct`；
- `ParticleStateSink`。

`RunnerBuilder` 提供注册表接入，用户可在自己的 Rust crate 实现并重新编译。M4 不加载动态库，不引入动态 ABI 或 unsafe 插件边界。

内置 ID：

~~~text
rk2_spherical/v0
surface_reflect/v0
model_top_terminate/v0
limited_domain_terminate/v0
global_periodic/v0
release_driven/v1
integer_stratified_birth/v1
dry_air_domain_fill/v1
stratospheric_ozone_domain_fill/v1
ertel_pv_spherical/v1
flexpart_stratospheric_ozone_pv60/v1
particle_state/v1
particle_state_sqlite/v1
~~~

未知 ID、重复注册、规则依赖字段缺失或不兼容组合必须在 Runner 构建阶段失败。

### 5.3 Runner 与状态

`SimulationRunner` 生命周期固定为：

~~~text
resolve and validate
create run directory
write running manifest
initialize population
emit births at exact event times
plan step boundaries
prepare/query meteorology
advance RK2
apply boundaries
maintain population/domain-fill
write scheduled output
finalize population and sink
write final manifest
~~~

`run()` 返回类型化 `RunOutcome`：

- `Complete`；
- `CompletedWithParticleErrors`；
- fatal `RunError`。

正常粒子终止：

- outside domain；
- population outflow；
- model top；
- 用户显式选择的正常边界终止。

异常粒子终止：

- numerical failure；
- invalid meteorology inside an expected-valid domain；
- reflection/root-finding limit；
- non-finite particle state。

异常粒子不得拖垮其它粒子，但 manifest 不能写普通 complete；M4 验收要求异常计数为 0。结构错误、资料失败、质量守恒失败和输出失败属于 fatal run error。

现有 `SimulationState` 不再声称可 checkpoint；M4 中断后只能使用冻结 Case、seed 和输入从头重放。

## 6. 球面 RK2 与边界

### 6.1 RK2

内置 `rk2_spherical/v0`：

- 水平位置使用三维单位球向量表示；
- 步首风生成半步单位向量并归一化；
- 在半步位置、半步高度和半步物理时刻正式查询 U/V/W；
- 使用中点风推进整步并归一化；
- vertical 使用 geometric W；
- signed dt 支持正反向；
- 不依赖 batch、chunk、worker 或 storage order；
- 不使用随机湍流。

`time_step` 是最大实际步长，只允许在以下事件处分割：

- meteorology frame；
- release birth；
- output；
- simulation end。

M4 不加入 CFL、自适应误差或自动子步。精度通过多步长二阶收敛测试和运行诊断约束。

### 6.2 地面

默认地面策略为连续反射：

- 沿本步提议轨迹定位 `height - terrain = 0` 的首次交点；
- 反射剩余的局地垂直位移；
- 加入 max(固定微小量, 地表高度 ULP 预算) 的安全偏移；
- 单步最多四次反射；
- 求根失败或超过限制时只异常终止该粒子。

禁止把终点简单 clamp 到 terrain。

### 6.3 模式顶和有限域

- 模式顶以上逐粒子终止并记录交点；
- 有限域出界逐粒子终止并记录交点；
- RK2 中点仅因水平出域而无值时，用步首速度构造只供出口 bracket 的单侧 proposal；不得先写 invalid meteorology，也不得把该 Euler endpoint 当作最终轨迹；
- surface/model-top 搜索在更早的水平域出口停止，域外缺字段不得抢先污染终止原因；
- domain-fill 的正常出流使用 `population_outflow`；
- 全球域经度周期处理；
- 不夹值、不外推、不让单粒子拖垮整场。

## 7. Air-mass domain-fill 科学合同

### 7.1 网格质量

- 水平网格面积使用球面经纬单元精确面积；
- hybrid 使用原生 A/B interface pressure；
- hybrid 几何高度复用查询引擎相同的 surface-geopotential hydrostatic recurrence，不读取旁路三维 height 冒充 native column；
- pressure-level 使用冻结的对数中点 interface 构造；
- 地下层不计入；
- 模式顶只计算资料实际覆盖的空气柱；
- q 缺失或非有限时不得假设干空气。
- 安全核心先剔除 halo；非周期外边缘停在最外安全网格点，内部取相邻中点，禁止向插值凸包外外推半格；
- 周期经度使用完整半格控制体，并在 M4 v1 视为无水平开放边界；
- full-level 高度界面使用内部算术中点、顶部最高有效 full level、底部 terrain；pressure 顶界质量不得导致粒子落到查询模式顶以上；
- 算法身份冻结为 `dry_air_finite_volume_grid/v1`。

层干空气质量按以下物理量计算：

~~~text
cell_area / g0 * integral((1 - q) dp)
~~~

使用固定顺序补偿求和。算法 ID 与常量进入 manifest/provenance。

### 7.2 初始化

- target count 模式精确生成指定数量；
- 单粒子载体质量 = 有效域总干空气质量 / target count；
- 固定质量模式生成完整质量粒子；
- 不足一个粒子质量的余数保存在初始化残余账本；
- 网格、层和单元内位置按等质量分层随机采样；
- 层内随机量是 pressure，不是几何高度；sampled pressure 通过界面间 log-pressure 映射成 ASL；
- 固定质量模式的初始 residual 独立留在全域账本，不并入任一边界面；
- 分布不依赖线程和 chunk。

### 7.3 有限域入流

每个边界面/垂直层计算：

~~~text
dry_air_density * normal_wind * face_area * abs(dt)
~~~

- 只累计积分方向对应的入流；
- 残余质量按稳定 boundary-face/layer ID 跨步保存；
- `residual + inflow` 每积满一个载体质量生成一个粒子；
- 每个静态截断区间只在物理中点计算一次 face/layer rate，并在动态子步中保持不变；
- 出生时刻是累计质量达到下一份完整载体的阈值时刻，向上量化到最小整纳秒，禁止随机提前创造质量；
- 新粒子的切向位置和层内几何高度按面通量分布随机；
- 出流粒子正常删除；
- 正反向使用对应的入流边界。

### 7.4 守恒门禁

每步账本至少包含：

~~~text
opening active carrier mass
opening residual mass
incoming mass
outgoing particle mass
normal terminated carrier mass
abnormal terminated carrier mass
closing active carrier mass
closing residual mass
~~~

门禁冻结为 `domain_fill_mass/v1`：

- 每步相对容差 `1e-12`；
- 最终累计相对容差 `1e-11`；
- 另加 64 ULP 表示下限；
- 固定顺序补偿求和；
- Case 不得覆盖或放宽；
- 超限为 fatal `MassConservation`。

## 8. 臭氧 domain-fill 科学合同

### 8.1 PV

生产 PV 使用 `ertel_pv_spherical/v1`：

- pressure 与 hybrid 共用公开 Ertel PV 物理定义；
- 使用球面度量和冻结 halo/差分规则；
- 单位为 PVU；
- 缺层、缺 halo、非单调柱或非有限输入硬失败；
- 不为匹配 FLEXPART 输出反向拟合公式。

### 8.2 首个具名规则

`flexpart_stratospheric_ozone_pv60/v1` 完整冻结 legacy 规则：

~~~text
height_asl > 3000 m
hemisphere-normalized PV > 2 PVU
ozone mole fraction = PV_PVU * 60 ppbv/PVU
ozone mass = carrier dry-air mass * mole_fraction * 48/29
~~~

规则必须明确标记为经验代理，不宣称等于观测或再分析臭氧场。

### 8.3 数量与生命周期

- 先计算满足规则的平流层载体质量；
- target count 精确生成筛选后的平流层粒子数；
- 固定质量模式的 target 指 dry-air carrier mass，不指 ozone mass；
- 边界入流只对满足规则的平流层空气生成臭氧粒子；
- 粒子日后穿越对流层顶时不重新赋值、不清零、不删除臭氧质量；
- FLEXPART 候选丢弃行为只形成差异报告。

`OzoneAssignmentRule` 必须声明 required canonical fields，并返回臭氧质量、quality 和 provenance。M4 正式注册 PV 规则，同时用合成第二规则证明扩展接口可用。资料原生臭氧场以后作为新规则加入，不改变 Case 或 Runner。

## 9. SQLite 输出

新增独立 workspace crate `trajecta-output-sqlite`，使用 bundled `rusqlite`。SQLite 为 public domain，Rust wrapper 必须满足 workspace 许可要求。

### 9.1 路径与生命周期

~~~text
<output_root>/<sanitized-case-name>/<uuid-v7>/
  run-manifest.json
  resolved-case.json
  resolved-run-profile.json
  particles.sqlite
~~~

- 每次运行生成唯一 run ID；
- 禁止覆盖旧目录；
- 在首粒子产生前创建目录和 running manifest；
- 输出失败保留部分数据库和 manifest；
- M4 不从部分数据库续跑。

### 9.2 SQLite 模式

默认设置：

~~~text
journal_mode = WAL
synchronous = NORMAL
foreign_keys = ON
user_version = 1
single writer
concurrent read allowed
~~~

不支持 WAL 的文件系统必须硬失败，不静默回退。

公开核心表：

- `run`；
- `particle`；
- `particle_mass`；
- `output_event`；
- `particle_state`；
- `termination`。

辅助 provenance 表可以按 schema version 演进，但不得改变公开核心列的含义。

`particle_state` 至少保存：

- particle ID 和 sample sequence；
- physical timestamp；
- integration offset 和 elapsed age；
- longitude、latitude、ASL height；
- alive/termination status；
- U、V、W；
- pressure、temperature；
- 每个气象值的 validity/quality。

规则：

- 数值直接存数值列，不用 JSON blob；
- 静态身份与时间状态分表；
- substance mass 使用规范化子表；
- 轨迹索引支持 `particle_id, sequence`；
- 时间切片索引支持 `time, particle_id`；
- 每个输出时刻使用一个事务；
- prepared statements 流式插入；
- 禁止逐粒子 commit；
- 正常结束执行 WAL checkpoint 和 integrity check。

### 9.3 气象取值

输出中的 U/V/W、pressure 和 temperature 必须在保存后的粒子位置与物理时刻正式查询：

- 禁止用不同位置或时刻的 RK2 midpoint 值冒充；
- 完全相同的查询键和字段超集允许复用；
- 终止点若已处于气象无效位置，数值列为 NULL，并写 validity/termination reason；
- 默认保存紧凑 provenance；
- 完整角点/层/时间权重通过显式 explain 重放获得，不默认写入全部轨迹。

模拟完成后，按 particle ID 和 sample sequence 查询即可得到设定 interval 下的完整离散轨迹。domain-fill 中途生成的粒子从 birth 开始，提前终止的粒子以 termination 结束。

## 10. RunManifest

RunManifest 必须 serde 化并拥有稳定 schema/version，至少记录：

- run ID、case name 和生命周期状态；
- resolved Case/RunProfile SHA；
- dataset lock/Profile/content SHA；
- Git commit 与 crate versions；
- worker 数、内存预算、executor、reader backend；
- 生成或声明的 seed；
- integrator、boundary、population、ozone rule 和 sink ID；
- 自动规范化的 GeoJSON 摘要；
- 自动补齐的默认输出配置；
- SQLite schema 与 journal/synchronous 策略；
- 正常/异常终止统计；
- domain-fill 每步与最终质量账本摘要；
- wall time、peak RSS、I/O counters 和输出行数；
- complete、completed_with_particle_errors 或 failed 状态。

写入使用同目录临时文件加原子替换。进程崩溃留下的 running manifest 由读取工具解释为 interrupted；不得自动改写成 complete。

## 11. 实施阶段

### M4-A0：合同冻结

- 写入本文和模型分工；
- 修改 Case/RunProfile schema；
- 冻结算法 ID、常量、错误码、manifest 和 SQLite schema；
- 提供解析场和独立参考公式；
- B/C 不得提前实现会决定科学含义的逻辑。

### M4-A1：普通粒子闭环

- 已完成并由 A 验收：ParticleBatch/typed origin、Philox、exact birth/质量分配、StepPlanner、spherical RK2、连续 boundary policies、ReleaseDriven lifecycle、SimulationRunner、GeoJSON 日期线自动切分与球面采样、生产 RunnerBuilder、manifest/provenance bundle、SQLite typed sink、真实 CFSR E2E 和确定性 digest 矩阵。
- 阶段提交：`18664f112d7d7cd3155dd10c71b7fecdc7d4ca17`。

### M4-A2：Air-mass domain-fill

- 状态：已完成并由 A 裁决；该阶段实现、诊断、平台实跑、artifact 汇总和报告均由 A 独立完成；当前 B 使用规则见模型职责文档；
- AirMassDeriver；
- pressure/hybrid 干空气网格质量；
- 初始化质量分层；
- 有限域边界通量；
- 残余质量；
- 出流删除；
- 每步/最终守恒账本；
- 三套真实资料主矩阵。
- 正式证据：Windows 6/6、WSL Ubuntu-24.04 6/6，共 12/12；每格 10,000 粒子、正反向、600 秒、2 个数值步、`Complete`、abnormal=0；见 `TRAJECTA_M4_A2_A_COMPLETION_REPORT.md`。
- 边界：Windows/WSL normalized output digest 尚不相同；A2 退出条件不要求 bitwise 跨平台一致，差异量化保留给 M4-A4。

### M4-A3：Ozone domain-fill

- 状态：已完成并由 A 裁决；B 仅可由用户通过 A 编写的书面 Prompt 另行启动，A 不调用子代理；本阶段实现、真实资料、oracle、native 差异和报告均由 A 完成；
- Ertel PV；
- `OzoneAssignmentRule` 注册表；
- PV60 legacy 规则；
- 三套真实资料；
- 公开公式与 GPL oracle；
- 现代算法与 FLEXPART 差异报告。
- 正式证据：Windows 6/6、WSL Ubuntu-24.04 6/6，共 12/12；每格 1,000 粒子、正反向、600 秒、2 个数值步、`Complete`、abnormal=0；GPL PV60 scalar oracle 10/10；Windows Rust/native 6/6 pair、0 blockers；见 `TRAJECTA_M4_A3_A_COMPLETION_REPORT.md`。

### M4-A4：平台、性能与终审

- 状态：尚未开始；中难/高难与最终签署由 A 完成，若有合同已冻结的中等及以下机械任务，只通过书面 Prompt 由用户另行交给 B；
- Windows/WSL 完整门禁；
- Rust/native 轨迹差分（A3 已提前取得三族双方向 1,000 粒子 evidence，A4 可复用并做终审）；
- WSL 10 万粒子长测；
- SQLite 并发读取、完整性与体积；
- 确定性和无执行期 I/O；
- A 最终审计、修订状态并提交。

## 12. 测试矩阵

### 12.1 日常测试

默认 `cargo test` 只运行小型解析场和合成夹具：

- zero wind stationary；
- constant east/north wind；
- solid-body spherical rotation；
- constant geometric W；
- 正向/反向；
- 多步长二阶收敛；
- 日期线与高纬；
- event boundary 切分；
- release 时间、水平和垂直分布；
- 地面连续反射；
- 模式顶和有限域终止；
- domain-fill 初始化和边界账本；
- ozone scalar rule；
- SQLite schema、事务、索引和并发读取；
- manifest running/complete/failed；
- 线程、chunk、排列确定性。

M3 百万点测试不得带入 M4 默认测试。

### 12.2 真实资料功能矩阵

Air-mass 主矩阵：

- ERA5 pressure：1 万粒子，正向/反向；
- ERA5 hybrid：1 万粒子，正向/反向；
- CFSR：1 万粒子，正向/反向；
- Windows 与 WSL 均实际运行。

普通 release 与 ozone：

- 三套资料各 1,000 粒子；
- 正向/反向；
- Windows 与 WSL 均实际运行。

Rust/native：

- 三套资料各 1,000 粒子；
- 正向/反向各 2 步；
- 比较状态、终止、U/V/W、pressure、temperature、quality 和逻辑 SQLite 行；
- 可复用上述小矩阵，不重复主长测；
- SQLite 文件字节不要求相同，使用规范 SQL 导出摘要比较。

### 12.3 WSL 10 万粒子长测

固定场景：

- ERA5 hybrid 137 层；
- air-mass domain-fill；
- 100,000 初始粒子；
- 正向和反向各一条；
- 10 分钟最大步长；
- 1 小时模拟；
- 每 10 分钟 SQLite 输出；
- 最多约 700,000 条状态行，另含 birth/termination；
- 输出与下一步首查询的完全相同键必须复用；
- 六步最多 13 个唯一批量气象查询。

该测试只在显式/夜间门禁运行，不进入普通 workspace tests。

### 12.4 性能门禁

- peak RSS ≤ 1 GiB；
- SQLite 最终大小 ≤ 512 MiB；
- 5 万到 10 万耗时比 ≤ 2.4；
- 气象预加载后 reader/provider I/O delta = 0；
- 1 worker 与 4 workers 的规范轨迹摘要一致；
- WSL 固定 runner 第一次合格结果冻结为 baseline；
- 后续同 runner wall time 不得回退超过 25%；
- 不设置跨不同硬件的统一分钟硬限。

## 13. Oracle 与差异政策

Hard gate：

- 解析解；
- 二阶收敛；
- 物理不变量；
- release 质量与数量；
- domain-fill 质量守恒；
- ozone scalar formula；
- schema、coverage、finite、hash 和输入身份。

FLEXPART 只承担：

- 同输入下的版本化行为参考；
- legacy 臭氧规则标量 oracle；
- 普通轨迹和 domain-fill 的差异报告。

完整轨迹数值差异默认 `report_only`，但报告缺失、coverage 不完整、非有限值或 schema 无效必须失败。不得为了“修绿”修改现代 RK2、Ertel PV、几何 W、边界或守恒公式。

GPL harness 保持独立进程/工具边界，不链接进 MIT crates。

## 14. 交付物

M4 最终至少交付：

- 冻结后的 Case、RunProfile、manifest 和 SQLite schema；
- `SimulationRunner` 与内置注册表；
- release/domain-fill/ozone 三种 population；
- SQLite 官方 sink；
- 合成解析测试与真实资料矩阵；
- Windows/WSL/native/10 万性能 artifact；
- FLEXPART 差异报告；
- A 分阶段实现、执行与验收报告；历史 B/C 报告只保留为既有证据；
- 最终 A 科学验收结论。
