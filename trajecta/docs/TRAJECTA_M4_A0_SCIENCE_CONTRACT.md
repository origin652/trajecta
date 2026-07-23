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

A1 的生产 path sampler 必须按穿越的插值网格单元提供有序区间，并在 clearance/model-top 标量极值或 inside/outside 转换处继续切段，使每段至多包含一个 down-crossing/出域转换。root solver 只按段端点 bracket，再固定二分 64 次；B 不得自行改成固定采样点近似。

`BoundaryPathSampler::ordered_segments` 必须返回从 0 到 1 连续、无重叠、无空洞的 certified 区间；`validate_ordered_path_segments` 是公共形状校验器。每次地面碰撞后必须调用 `retarget`，把 sampler 重绑到剩余路径并恢复局部 `[0,1]`；重用碰撞前路径属于合同错误。

### 6.2 模式顶、有限域与全球域

- model top 和 limited domain 记录最早交点后正常终止；
- domain-fill 出流改记 `population_outflow`；
- 全球 longitude 周期化到 `[-180,180)`；
- 任一非有限交点或内部 expected-valid 气象失败是异常终止；
- 不允许水平/垂直 extrapolation 或 clamp。

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

`q` 是 `[0,1)` 的 specific humidity。缺失或非有限 q 失败，不可假设 0。

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
