# Trajecta M4-A0：科学、数值与公共合同冻结

状态：A 模型冻结版，已通过 M4-A0 门禁；后续实现与验收必须同时满足本文、机器可读 registry 和编译期类型。本文不代表 M4 完成。

权威顺序：

1. `testdata/M4_NUMERICAL_CONTRACT.v1.json` 中的 ID、常量和容差；
2. `trajecta-core::science` 中的编译期同值常量；
3. 本文中的公式、运算顺序和失败语义；
4. `TRAJECTA_M4_PARTICLE_LOOP_PLAN.md` 的总体范围。

如果上述来源不一致，门禁必须失败，由 A 修订；B/C 不得自行选择其中一个。

## 1. 版本化交付物

- 粒子闭环：`trajecta/particle_loop/m4/v1`；
- manifest：`trajecta.run-manifest/v1`；
- SQLite：`PRAGMA user_version = 1`；
- 数值 registry：`trajecta.m4.numerical-contract/v1`；
- Case/RunProfile 仍处于仓库开发 schema 0，但 M4 新增字段的含义按本文冻结；
- M4 内不得无版本地改变字段含义、公式、严格/非严格阈值或失败分类。

机器可读文件：

- `testdata/M4_NUMERICAL_CONTRACT.v1.json`；
- `testdata/M4_NUMERICAL_CONTRACT.schema.json`；
- `testdata/M4_RUN_MANIFEST.schema.json`；
- `testdata/M4_SQLITE_SCHEMA.v1.sql`。

可执行自校验：`python tools/validate_m4_a0_contracts.py`。

## 2. Case 与 RunProfile

### 2.1 Seed

`NumericsSpec.random_seed` 可选。用户提供的 `u64` 原样使用；缺省值必须来自操作系统熵。生成 seed 必须在任何粒子 ID、出生位置或出生时刻生成前写入 running manifest。

随机算法为 `philox4x32-10/v1`。完整逻辑键包括：

~~~text
seed
SHA256(population_id)[0..8] big-endian
SHA256(lifecycle_event_id)[0..8] big-endian
particle_id / stable ordinal
sampling_dimension
draw_index
~~~

字段以 big-endian 字节和固定 domain separator 再做 SHA-256，前 16 byte 作为 Philox counter，后 8 byte 作为 key。Philox 输出 word 0/1 组成 big-endian `u64`；`f64` 使用最高 53 bit 乘 `2^-53`，范围严格为 `[0,1)`。

release sampling dimension 冻结为：birth time=0、horizontal component=1、horizontal u=2、horizontal v=3、vertical=4。不同 sampler 必须使用这些公共常量和 `draw_index`，不得自行复用 birth-time 维或按调用顺序分配维度。

`ReleaseSamplingRequest` 必须显式携带 seed、population、event 和连续 ordinal 范围；采样器不得把这些信息藏在可变对象状态或全局流中。

A1 勘误：`ReleaseAllocator` 不能只接收 event。正式 `ReleaseAllocationRequest` 还必须携带 population、seed、首 ordinal、已采样水平坐标和已解析 ASL 高度，否则无法同时兑现稳定 ID、精确 birth time、chunk 独立性和垂直硬失败。水平/原生垂直采样留给可替换 sampler；ID、出生时刻和质量补偿只由 allocator 决定。

### 2.2 显式 release

`ReleaseDrivenSpec.events` 是唯一 release schedule。事件物理时间恒满足 `start <= end`，与正反向积分方向无关。

连续事件持续时间按整数纳秒和粒子数 `N` 划分。第 `i` 个粒子的时段边界为：

~~~text
lo_i = floor(i * duration_ns / N)
hi_i = floor((i + 1) * duration_ns / N)
~~~

出生偏移使用 `integer_stratified_birth/v1`：

~~~text
width = hi_i - lo_i
within = (random_u64 * width) >> 64
offset = lo_i + within
~~~

乘法在 `u128` 中完成。`duration_ns = 0` 时所有粒子在 start 出生。该公式不经过浮点数，不依赖积分步、worker、chunk 或排序。

每种 substance 的事件总质量按粒子数精确等分；最后一个粒子接收补偿余数，使固定顺序求和精确回到账面事件总质量。禁止负质量、非有限质量和全零事件。

### 2.3 稳定粒子 ID

SQLite 使用非负 signed 64-bit ID，因此 M4 粒子 ID 使用 SHA-256 摘要前 8 byte 的低 63 bit：

~~~text
release:        SHA256("release\0", population, event, ordinal)
domain initial: SHA256("domain-initial\0", population, domain, ordinal)
boundary birth: SHA256("domain-boundary\0", population, domain, face-layer-id,
                       lifecycle-event-index, ordinal)
~~~

所有字符串先写 UTF-8 byte length，再写原始 UTF-8 bytes。发生 63-bit 碰撞时构建/运行硬失败，不允许线性探测或依赖插入顺序重编号。

### 2.4 GeoJSON

支持 Point、MultiPoint、LineString、MultiLineString、Polygon、MultiPolygon。边是短大圆弧，不是经纬度平面直线。

规范化规则：

- longitude 输出到 `[-180, 180)`；
- ring 必须闭合、至少四点，连续重复点删除后仍须有效；
- 每条边选择绝对经差不大于 180° 的短弧；恰好 180° 的边因方向不唯一而拒绝；
- exterior 统一为球面正向，hole 为反向；ring 起点旋转到按 `(lon,lat)` 字典序最小的顶点；
- Multi 组件按 canonical encoded bytes 排序；
- antimeridian 交点由大圆所在平面与 `lon = ±180°` 半平面求交；切分结果为规范 MultiPolygon；
- 包含任一极点、面积为零、自交、hole 越界或面积歧义的 Polygon 拒绝；
- 切分前后球面面积相对差不得超过 `1e-12`；
- 点等权、线按大圆长度、面按球面面积减洞加权。

面积使用同一 M3/M4 球半径 6,371,229 m。规则格单元面积为：

~~~text
R² * Δλ * (sin φ_north - sin φ_south)
~~~

### 2.5 垂直 release

- ASL/AGL 以 geometric metre 表示；
- pressure 以 Pa 表示；
- `upper = null` 是固定值，否则在数值闭区间均匀采样；
- AGL 可以为 0，pressure 必须严格大于 0；
- 任何样本地下、越顶、域外或垂直柱无效，整个 event 失败；
- 禁止重采、clamp、丢弃单粒子或缩减质量。

`ReleaseVerticalResolver` 必须在每个粒子的精确 birth time 和已采样水平位置把 AGL/pressure 转成 geometric ASL。仅 ASL event 可使用 direct resolver；AGL/pressure 不得机械透传。

### 2.6 Domain-fill 与输出

Domain-fill 必须给单一 `domain_id`，并在 `target_particle_count` 与 `target_dry_air_mass_per_particle` 中二选一。臭氧配置另给 `ozone_rule` 与 `ozone_substance`。

输出 schedule 只有：

~~~text
endpoints
interval { interval, origin? }
~~~

未声明 outputs 时注入 `particle_state/v1` + `particle_state_sqlite/v1` + endpoints。RunProfile 必须提供 `output_root`；它可尚不存在，但解析后必须是规范绝对路径。

## 3. Particle 与终止状态

每粒子固定保存：ID、population、origin、birth time、lon/lat/ASL、signed integration offset、非负 elapsed age、非负 dry-air carrier mass、substance mass、可选独立 sensitivity weight 和 typed status。

正常终止：`model_top`、`outside_domain`、`population_outflow`、显式用户终止边界。

异常终止：`numerical_failure`、`invalid_meteorology`、`reflection_limit`、`non_finite_state`。

异常粒子不使其它粒子失败，但最终状态必须为 `completed_with_particle_errors`。结构、资料准备、输出或质量守恒失败是 run-level fatal error。

## 4. 时间与 StepPlanner

- Timestamp 比较使用 UTC `(seconds,nanosecond)`；
- signed step 必须与 Direction 一致；
- boundary 必须按积分方向排序，相同时刻允许多个事件；
- 相同时刻不生成零长度步；
- meteorology frame、birth、output、end 都是不可跨越边界；
- End 是硬上限，可截短请求步；
- 全部加减先用 i128 纳秒并检查 Timestamp 与 i64 duration 溢出。

`PopulationContext` 对每个 per-step 回调显式携带 direction、signed step、step index 和 random seed；初始化/收尾时 step 与 step index 为 null。domain-fill 实现不得从时刻差、调用次数或可变全局状态猜这些值。

## 5. `rk2_spherical/v0`

位置由单位球向量 `r` 表示。经纬度局地基：

~~~text
east  = (-sin λ, cos λ, 0)
north = (-sin φ cos λ, -sin φ sin λ, cos φ)
tangent(u,v) = u * east + v * north
~~~

给定 signed `dt`：

~~~text
r_half = normalize(r0 + tangent(u0,v0) * dt/(2R))
z_half = z0 + w0 * dt/2

在 (r_half, z_half, t0 + dt/2) 正式查询 u_half/v_half/w_half

r1 = normalize(r0 + tangent_half(u_half,v_half) * dt/R)
z1 = z0 + w_half * dt
~~~

运算顺序与 `trajecta_core::reference::spherical_rk2_step` 一致。W 必须是 geometric `m s-1`。精确极点因 local east 不唯一而产生 typed invalid sample；不得任意选择方向。若 signed `dt` 为奇数纳秒，无法表示的半纳秒按整数除法朝步首取整；正反向都使用同一“朝步首”规则。输出时气象必须在 `(r1,z1,t1)` 重新查询，midpoint 值不可冒充。

有限域有一个受限例外 `rk2_domain_exit_euler_bridge/v1`：若步首正式查询为 `Ok`，而 RK2 中点唯一失败原因是水平 `OutOfDomain`，积分器不得先记 `invalid_meteorology`。它使用步首速度构造完整单侧退出 proposal，仅供连续 limited-domain path solver bracket 最早出口；最终粒子位置仍由边界交点决定，不把 Euler endpoint 当作轨迹结果。中点的任何其它 status 仍是异常气象终止。

连续边界路径使用 `great_circle_atan2_oriented_basis/v1`。给定两个单位球端点 `a,b`：

~~~text
omega = atan2(norm(cross(a,b)), clamp(dot(a,b), -1, 1))
n     = normalize(cross(a,b))
t     = normalize(cross(n,a))
r(f)  = normalize(cos(f*omega) * a + sin(f*omega) * t)
~~~

`omega <= 1e-15` 的退化短段直接返回步首；其它段必须使用有向法向量构造切向量。禁止改回
`acos(dot(a,b))` 或 `(b-cos(omega)*a)/sin(omega)`，因为米级路径会分别丢失弧长有效位和发生灾难性相消；
正对跖等无法定义唯一短弧基底的输入仍是 typed failure。

解析夹具的观测二阶收敛率必须不低于 1.9。

## 6. 边界

### 6.1 地面

clearance 为 `particle ASL - terrain ASL`。策略必须拥有完整 start/proposal 和连续 path sampler；只有 endpoint 的接口不合格。

- 定位最早的正到非正 crossing；
- 碰撞点保留水平位置；
- 对碰撞后的剩余局地垂直位移取反；
- 最终 clearance 至少为 `max(1e-6 m, 64 * positive_ulp(terrain))`；
- 单步最多四次反射；
- 无法 bracket/converge 或超过四次时，该粒子异常 `reflection_limit`；
- 禁止把 endpoint 直接 clamp 到 terrain。

A1 的生产 path sampler 必须按穿越的插值网格单元提供有序区间，并在 clearance/model-top 标量极值或 inside/outside 转换处继续切段，使每段至多包含一个 down-crossing/出域转换。若 outward-rounded 的整段 clearance/model-top-gap 值区间严格不含零，可直接证明该段无对应边界 crossing，不必为与 crossing 无关的内部极值继续切段；这不是采样或容差放行。root solver 只按段端点 bracket，再固定二分 64 次；B 不得自行改成固定采样点近似。

`BoundaryPathSampler::ordered_segments` 必须返回从 0 到 1 连续、无重叠、无空洞的 certified 区间；`validate_ordered_path_segments` 是公共形状校验器。每次地面碰撞后必须调用 `retarget`，把 sampler 重绑到剩余路径并恢复局部 `[0,1]`；重用碰撞前路径属于合同错误。

### 6.2 模式顶、有限域与全球域

- model top 和 limited domain 记录最早交点后正常终止；
- domain-fill 出流改记 `population_outflow`；
- 全球 longitude 周期化到 `[-180,180)`；
- 任一非有限交点或内部 expected-valid 气象失败是异常终止；
- 不允许水平/垂直 extrapolation 或 clamp。
- 地面和模式顶搜索遇到同一路径更早的水平出域时必须在域出口停止，不得因出口外 surface/model-top 字段为空而抢先写 `invalid_meteorology`；随后 limited-domain 策略记录该出口。

### 6.3 Runner 生命周期

`SimulationRunner::run` 的强制顺序是：先验证并持久化 `running` manifest，再 begin outputs、initialize population、在当前物理时刻 emit、按 release/met/output/end 合并边界拆步、before-step、RK2 proposal、boundary chain、after-advection、population maintenance、exact-time output，最后 population finalize、output finish 和 terminal manifest。

- `running` manifest 持久化失败时不得创建首粒子；
- abnormal particle 只改变最终 `RunOutcome` 为 `CompletedWithParticleErrors`，不终止其它粒子；
- fatal population/integration/boundary/output 错误必须写 `failed` manifest；
- output 需要气象时由独立 `QueryPlan` 在输出时刻和最终粒子位置正式执行，不接收 RK2 midpoint 缓存；
- `RunnerBuilder`、原子 manifest store、生产 boundary sampler 和 SQLite sink 属 B 工程接线，不得修改上述顺序。

## 7. 干空气质量与 boundary flux

### 7.1 Pressure interfaces

pressure centres 必须按 top-to-bottom 严格递增。内部 interface 是几何平均：

~~~text
p[k+1/2] = sqrt(p[k] * p[k+1])
~~~

顶/底 interface 使用相邻 centres 的相同半 log-spacing 外推。有效底界裁到 surface pressure；完全地下层不计。单层 pressure 数据因 interface 不可辨识而拒绝。

Hybrid interface 使用资料原生 `A + B*surface_pressure`，不重建、不平滑。

### 7.2 Dry-air mass

按 top-to-bottom 固定顺序 Neumaier 求和：

~~~text
cell dry mass = area/g0 * Σ((1-q[k]) * effective_dp[k])
~~~

`q` 使用显式双轨语义：

- raw q 是查询与审计输出；所有有限且 `< 1` 的源值均原样保留，包括有限负值；
- physical q 只供 density、hydrostatic geometry、surface layer 与 domain-fill 质量等物理消费者使用；
  每个源时次先执行 `specific_humidity_nonnegative_projection/v1`，将有限负 q 投影为 0，之后才按
  冻结权重做时间插值。

raw `SpecificHumidity` provenance 不得声称该投影；`AirDensity` 以及 surface/mixed route 的物理输出
必须记录该算法。缺失、非有限或 `q >= 1` 仍然整列/整点失败，禁止通过 valid-triangle 降级，
也禁止在时间插值或最终质量公式之后静默 `max(q, 0)`。

近地层 roughness 同样区分 source 与 physical 值。冻结的 CFSR SFCR 三个时次均无 missing，
但以 `1e-4 m` 十进制量化步长在海洋格点编码 exact zero。raw roughness 保留 exact zero；
对数 surface-layer 物理消费者执行 `aerodynamic_roughness_zero_projection/v1`，仅将 exact zero
替换为 `1e-4 m`。任何正值（包括小于 `1e-4 m`）保持不变；负值和非有限值失败。

近地风使用 `surface_layer/monin_obukhov_businger_dyer/v1` 与
`ten_metre_anchored_businger_dyer_wind/v1`。在 similarity range 内，稳定度形状函数记为
`F_m(z)`，冻结公式为 `U_vec(z) = U_vec(10 m) * F_m(z) / F_m(10 m)`；因此 10 m 向量是精确
锚点、风向保持、风速非负。`u*` 仍参与 surface exchange scales 与 Monin-Obukhov 稳定度，
不得再作为独立加法修正与 10 m 风重复约束，也不得在得到负风速后静默 `max(speed, 0)`。
similarity range 以上仍以匹配该归一化剖面导数的单调桥连接最低有效三维层。

温度和比湿使用 `two_metre_anchored_businger_dyer_scalar/v1`。对任一标量锚点 `S(2 m)` 与
surface exchange scale `s*`：

~~~text
S(z) = S(2 m) + s*/k * [ln(z/2 m) - psi_h(z/L) + psi_h(2 m/L)]
~~~

这是锚点差公式；动量粗糙度 `z0` 必须严格相消，不得拿 `z0` 拒绝合法的 2 m 温湿度锚点。
因此即使 CFSR 的动量 `z0 > 2 m`，只要查询高度满足完整 transport 下界且所有物理输入有限，
标量剖面仍可定义。最终比湿必须在 `[0,1)`；无效值仍失败，禁止为修绿静默 clamp。

surface/mixed route 的三维连接层使用 `lowest_complete_transport_anchor/v1`：从底向上寻找同一层，
要求 U、V 和派生 geometric W 在相关时间端点及冻结时间插值上同时完整。结构上最低的 pressure/hybrid
层若缺任一 transport 分量，不得只用其 T/q 或单独 U/V 冒充三维锚点；只有最低完整锚点以下的合法
高度区间走 surface route，锚点及其上方走 upper-air route。provenance 必须记录所用的联合锚点算法。

`dry_air_finite_volume_grid/v1` 的安全核心与几何边界冻结如下：

- x/y 索引先剔除 Profile 声明的 halo；非周期方向的外边缘停在最外安全网格点，内部边缘取相邻网格点中点，禁止向可插值凸包外再外推半格；
- 周期经度方向使用网格点两侧各半格的完整控制体；M4 v1 将 `periodic_longitude=true` 解释为无水平开放边界，因此不生成水平补充面；
- 球面水平面积为 `R² Δλ (sin φ_n - sin φ_s)`；顺序固定为 y、x、native level；
- full-level 几何高度必须 top-to-bottom 严格下降；内部高度界面取相邻中心算术中点，几何模式顶固定为最高有效 full level（查询引擎禁止向其上外推），最低有效界面落在本地 terrain；pressure interface 的顶端质量仍按 7.1 的半 log-spacing 定义，但其几何支撑不得越过可查询模式顶；
- pressure-level 地下层按 surface pressure 裁剪，hybrid 只使用原生 A/B interfaces；hybrid full-level 几何高度必须用与查询引擎 `ColumnGeometry` 相同的 surface-geopotential 向上 hydrostatic recurrence 生成，不得改用旁路三维 height 字段。

初始 `dry_air_equal_mass_stratified/v1` 先在固定顺序累计干空气质量上分层，再在控制体内采样：经度均匀、`sin(latitude)` 均匀、层内 pressure 均匀。粒子只保存 ASL 高度，因此 sampled pressure 用界面间 log-pressure 线性关系转换为高度；不得改成几何高度均匀后仍宣称等质量采样。

每一列的干空气质量下界不是 terrain，而是查询引擎的最低完整 transport floor：

~~~text
z_floor = z_terrain + max(0.5 m, 2*z0_physical)
f       = (z_floor - z_terrain) / (z_lowest_full - z_terrain)
p_floor = p_surface * (p_lowest_full / p_surface)^f
~~~

`dry_air_transport_floor_log_pressure/v1` 使用压力比幂式，避免 `exp(lerp(ln p))` 在
`p_lowest_full == p_surface` 时把结果推高约 1 ULP；结果只允许在解析舍入界内钳回
`[p_lowest_full,p_surface]`，越界仍失败。

水平位置确定后执行 `dry_air_transport_floor_following_horizontal_relocation/v1`：保持粒子相对源列
transport floor 的高度，并把源 layer 的上下界整体平移到目标经纬度的双线性 local transport floor。
不得只保持 terrain-relative AGL，因为 roughness 与完整 U/V/W 下界也会随位置改变；不得保持旧 ASL
将粒子放到不可查询的近地缺口，也不得用随机重抽、地面 epsilon clamp 或改变载体质量掩盖冲突。
固定质量模式未满一个载体的初始 residual 是独立的全域账本质量，不与任一 boundary-face residual 混合。

边界 dry-air partial density：

~~~text
R_m = (1-q) R_d + q R_v
rho_total = p / (R_m T)
rho_dry = (1-q) rho_total
~~~

边界面层入流质量：

~~~text
rho_dry * inward_normal_velocity * geometric_face_area * abs(dt)
~~~

只累计积分方向的正入流。face/layer residual 以稳定 ID 跨步保存。

`dry_air_boundary_flux/v1` 使用边界控制体的原生层高度厚度乘球面水平边长作为 face area；密度取上下 pressure interface 的算术平均 pressure，并使用该 full level 的 T/q 与 inward-normal wind。正向 inward normal 为 west:+u、east:-u、south:+v、north:-v；反向积分整体翻转法向。

一个已经被 frame/release/output/end 边界静态截断的区间内，`dry_air_static_midpoint_flux/v1` 只在区间物理中点准备一次完整 snapshot，并把每个 face/layer 的 rate 视为区间内常数。动态出生只继续切分此区间，不重新取新中点或改变原计划 rate。

`dry_air_mass_threshold_birth/v1` 不随机提前出生：对每个 face/layer，累计 `opening residual + rate * elapsed` 第一次达到下一份完整 carrier mass 的物理时刻即为出生时刻；时间量化为“不早于阈值”的最小整纳秒。随机数只决定 face 内切向位置与几何高度位置。这样每个动态子步结束时 residual 始终在 `[0, carrier mass)`，且不会暂时创造载体质量。

### 7.3 Mass gate

每步相对容差 `1e-12`，最终累计 `1e-11`。绝对门限为：

~~~text
max(relative_tolerance * accounting_scale,
    64 * positive_ulp(accounting_scale))
~~~

超限是 fatal `run.mass_conservation`，Case 不可覆盖。

## 8. `ertel_pv_spherical/v1`

~~~text
theta = T * (100000 Pa / p)^(Rd/Cpd)

zeta_p = 1/(R cos(phi)) * [d(v)/d(lambda) - d(u cos(phi))/d(phi)]

PV_SI = -g * [
    (zeta_p + 2 Omega sin(phi)) * d(theta)/dp
    + d(u)/dp * 1/R * d(theta)/d(phi)
    - d(v)/dp * 1/(R cos(phi)) * d(theta)/d(lambda)
]

PV_PVU = PV_SI * 1e6
~~~

水平导数在 native grid 上使用三点二阶 Lagrange 导数；longitude 使用周期邻点或冻结 halo，latitude 必须有真实 halo。垂直导数对实际 pressure 使用非均匀三点 Lagrange：内部 centred，顶底 one-sided。少于三层、非单调 pressure、缺 halo、精确极点或非有限输入硬失败。先在 native grid 计算 PV，再走正式查询插值。

`Omega = 7.2921150e-5 s-1` 与 reference pressure 100000 Pa 均已进入编译期和机器 registry；B 不得从 FLEXPART 输出反推差分格式。

## 9. `flexpart_stratospheric_ozone_pv60/v1`

严格 mask：

~~~text
height_asl > 3000 m
hemisphere_pv = pv              latitude >= 0
hemisphere_pv = -pv             latitude < 0
hemisphere_pv > 2 PVU
~~~

质量：

~~~text
mole_fraction = hemisphere_pv * 60e-9
ozone_mass = dry_air_carrier_mass * mole_fraction * 48/29
~~~

3 km 和 2 PVU 均为严格大于。规则外返回 0 ozone mass，不删除 carrier。该规则是经验代理，quality 为 derived，provenance 必须写规则 ID。

Eligibility 与诊断失败的顺序也冻结：先按 3 km 几何阈值裁掉无 eligible 厚度的 layer/face，
边界再裁掉零 inward flux，最后才要求剩余 eligible 支撑存在有限 PV。完全低于 3 km 或零入流的
face 缺 PV 不得误报 fatal；任何真正跨入 eligible 区间的质量缺 PV 仍必须硬失败，禁止静默回退或
把缺测当作 0 PV。

## 10. Manifest 与 SQLite

运行目录：

~~~text
<output_root>/<sanitized-case-name>/<uuid-v7>/
  run-manifest.json
  resolved-case.json
  resolved-run-profile.json
  particles.sqlite
  provenance-bundle.json
~~~

running manifest 必须在首粒子前原子写入；final manifest 使用同目录临时文件和原子替换。崩溃留下 running，由读取端解释为 interrupted，不自动改写。

SQLite 必须为 WAL/NORMAL、single writer、允许 concurrent reader。每个 output event 一个事务，禁止逐粒子 commit。公开表、列、约束和索引以 `M4_SQLITE_SCHEMA.v1.sql` 为准；实现不得用 JSON blob 替代核心数值列。

Particle ID、boundary face ID 和 elapsed age 写 SQLite 前必须 `<= i64::MAX`。输出 U/V/W、pressure、temperature 必须来自存储位置和时刻的正式查询；无效终止点使用 SQL NULL 加 validity/reason。

SQLite v1 的单个 `provenance_id` 不能表达 U/V/W/P/T 五条独立来源链，不得取最小/首个 ID 冒充整行 provenance。正式来源合同是 `trajecta.provenance-bundle/v1`，详见 `TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md` 与 `M4_PROVENANCE_BUNDLE.schema.json`：五字段逐项映射、完整 sources/transforms/parameters/profile/fallback、record/field-set 内容寻址、sample↔SQLite 全覆盖、同目录原子写，并由终态 manifest 固定 bundle SHA 与最终 SQLite SHA。`complete`/`completed_with_particle_errors` 缺 bundle 或任一交叉校验失败均不得发布终态 manifest。

## 11. 稳定错误码

各公开错误枚举的 `code()` 是精确机器合同；同一枚举变体在 M4 内不得改码。下表冻结分类与命名空间，`*.not_implemented` 仅用于阶段开发，M4 最终支持路径不得可达。

| Code | 分类 |
|---|---|
| `case.release.*` | Case shape/cross-component hard failure |
| `case.population.domain_unknown` | Case hard failure |
| `run_profile.output_root_empty` | RunProfile hard failure |
| `rng.*` | build/run fatal deterministic-sampling failure |
| `release.*` | event-level hard failure |
| `clock.direction_mismatch` | build/run fatal |
| `clock.unordered_boundaries` | build/run fatal |
| `clock.overflow` | run fatal |
| `particle.invalid_coordinate` | abnormal particle termination |
| `particle.invalid_mass` | build/run fatal；若步中产生则异常粒子终止 |
| `integrator.*` | batch fatal，明确可逐粒子分类的数值失败除外 |
| `boundary.invalid_path_segments` | build/run fatal boundary contract failure |
| `boundary.root_finding_failed` | abnormal particle termination |
| `boundary.reflection_limit` | abnormal particle termination |
| `population.*` | population fatal；逐粒子异常须先转 typed termination |
| `output.*` | run fatal |
| `manifest.*` | manifest build/finalize fatal |
| `run.meteorology` | run fatal preparation/provider failure |
| `run.output` | run fatal |
| `run.mass_conservation` | run fatal |
| `run.resource_limit` | run fatal |

错误 message 可改进，code 和分类在 M4 内不得无版本变化。

## 12. A0 退出门禁

- Case、RunProfile、Particle、Runner、manifest、SQLite 和插件合同可编译；
- registry 与编译期常量逐值一致；
- Philox Random123 零向量通过；
- signed StepPlanner 正反向、同刻事件、End cap 和错误负例通过；
- RK2、球面积、pressure interface、dry mass/density、ozone 和 mass tolerance 独立公式测试通过；
- manifest/registry schema 与 SQLite SQL 可自检；
- fmt、clippy `-D warnings`、workspace tests 和 doc 全绿；
- A0 结束只表示合同可交给 B/C，不表示 A1–A4 或 M4 完成。
