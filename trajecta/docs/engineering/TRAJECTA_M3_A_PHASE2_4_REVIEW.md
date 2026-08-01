# Trajecta M3 Phase 2--4 A 级终审

## 1. 状态与结论

**评审日期：** 2026-07-17  
**被评审交付：** `TRAJECTA_M3_B_PHASE2_4_HANDOFF_TO_A.md`  
**结论：** CFSR 三时次工程链通过；ERA5 pressure/hybrid 的真实资料获取与 `lock -> frame` 部分通过，但两套 ERA5 完整查询链拒绝验收；M3 尚未完成，不应提交为完成态。

| 项目 | A 级结论 | 说明 |
|---|---|---|
| CFSR 官方 00/06/12 | 通过 | 哈希、三时次、03/09 中间时刻 Transport 查询与重复查询均有真实证据 |
| CFSR 百万点主路径 | 部分通过 | 1,024,017 点和 1/4 线程一致性通过；性能终审指标仍不完整 |
| ERA5 pressure 官方资料 | 部分通过 | 官方 00/06/12、37 层、近地源变量和 frame 已接通 |
| ERA5 pressure 完整查询 | 不通过 | 单位解析阻断；classic 删除 3-D `z`，没有正式逐层高度链 |
| ERA5 hybrid 137 层资料 | 部分通过 | 官方 00/03/06、完整 1--137、A/B 系数和 frame 已接通 |
| ERA5 hybrid 完整查询 | 不通过 | 单位解析阻断；NearSurface 输入不完整；`sp` provenance 不符合冻结合同 |
| Rust/native 数值认证 | 未通过 | 尚无 A 冻结 tolerance registry；现有硬编码阈值不能作为认证 |
| FLEXPART oracle | 未完成 | stub 的 MIT/GPL 边界可接受，但没有真实数值样本 |
| Windows/Linux 终审 | 未完成 | 当前只有 Windows 的部分工程证据 |

## 2. 本次复核证据

### 2.1 已实际复跑

此前真实资料门禁已经完成以下实跑：

- CFSR 00/06/12 三时次 Transport 查询；
- CFSR pure-Rust 双载全字段一致性；
- ERA5 pressure classic 的解码、lock 和 frame；
- ERA5 hybrid 137 层的解码、lock 和 frame；
- 中央真实资料 manifest 校验；
- CFSR 1,024,017 点主路径。

本次又直接运行两套 ERA5 CLI 正向查询：

```text
trajecta met probe ... era5-cf-pressure-netcdf-v0
trajecta met probe ... era5-cds-hybrid137-v0
```

两者均以退出码 1 失败：

```text
frame load: Decode(InvalidMetadata("unknown unit symbol 'W'"))
```

因此现有 ERA5 测试名中的 `full_chain` 只代表 reader/lock/frame 链，并不代表 M3 计划规定的 `TransportPlan -> prepare -> prepare_batch -> execute -> CLI` 完整正向链。

### 2.2 官方科学依据

- [ECMWF 参数 231：Instantaneous surface sensible heat net flux](https://apps.ecmwf.int/codes/grib/param-db/231) 明确说明 ECMWF 垂直通量采用**向下为正**。
- [ECMWF 参数 232：Instantaneous moisture flux](https://apps.ecmwf.int/codes/grib/param-db/232) 明确说明向下为正，蒸发为负、凝结为正。
- [ECMWF 参数 152：Logarithm of surface pressure](https://codes.ecmwf.int/grib/param-db/152) 明确说明 `lnsp` 是 surface pressure 的自然对数。
- [ERA5 data documentation](https://confluence.ecmwf.int/display/CKB/ERA5%3A+data+documentation) 明确说明 2 m specific humidity 应由 2 m dewpoint 与 surface pressure 按 IFS CY41R2 Part IV 方程 7.4/7.5计算，并对 dewpoint 使用水面饱和参数。
- [IFS CY41R2 Part IV: Physical Processes](https://www.ecmwf.int/en/elibrary/79697-ifs-documentation-cy41r2-part-iv-physical-processes) 是上述湿度公式和常量的正式来源。

## 3. 阻断问题

### A-P0-1：CF/UDUNITS 能量通量单位不能解析

`normalize_cf_unit_string()` 会把 `W m**-2` 变为 `W m-2`，但 `GraphUnit` 的基本/派生单位表没有 `W`，导致 frame 加载在任何查询之前失败。

A 裁决：

- `W` 作为 SI 派生单位，维度固定为 `kg m2 s-3`，比例为 1；
- `W m-2` 的 canonical SI 维度等于 `kg s-3`；
- CF 拼法 `W m**-2`、`W m-2`、`W/m2` 必须归一到同一维度；
- 不得通过删除源 `units` 属性或在 Profile 中谎报源单位绕过检查。

### A-P0-2：ERA5 pressure 删除逐层 `z`

`prepare_era5_pressure_anchors.py` 在 classic 转换时主动省略 pressure 文件中的 3-D `z`，只保留 surface 文件的 2-D `z`，以绕过当前按 `variable` 单键选择造成的 `AmbiguousSource`。

这不是可接受的正式方案。等压局地柱必须有逐层 geopotential/geopotential height 才能构造几何高度并执行 ASL/AGL 查询。

A 裁决：source identity 升级为至少三元合同：

```text
logical member role + variable + expected layout
```

- pressure role 的 `z` + `Full3D` -> ERA5 扩展 geopotential -> `geopotential_height(z)`；
- surface role 的 `z` + `Horizontal2D` -> `surface_geopotential`；
- 对声明了 role 的多文件 Profile，role 缺失、role 重复、变量匹配多文件、layout 不符都必须结构化硬失败；单文件 Profile 可以不声明 role；
- classic 转换必须保留 3-D `z`，不得继续以删字段消歧。

### A-P0-3：ERA5 NearSurface capability 是不完整的假阳性

两个 ERA5 Profile 都发布 `near_surface_transport`，但缺少：

- `two_metre_specific_humidity`；
- `latent_heat_flux`。

同时 `ishf` 被直接映射为 upward-positive canonical sensible heat flux，符号与 ECMWF 官方约定相反。Profile 校验当前只验证“列出的字段存在”，不会验证 capability 的固定输入集合，因此错误 Profile 仍可发布 capability。

A 冻结公式：

```text
H_up = -ishf_down

LE_up = -Lv * ie_down
Lv = M3_CONSTANTS.latent_heat_vaporization_j_kg = 2_500_000 J kg-1
```

2 m specific humidity 使用 ERA5/IFS 的水面饱和公式：

```text
e(Td) = a1 * exp(a3 * (Td - T0) / (Td - a4))
epsilon = Rd / Rv
q2m = epsilon * e / (sp - (1 - epsilon) * e)

T0 = 273.16 K
a1 = 611.21 Pa
a3 = 17.502
a4 = 32.19 K
Rd、Rv 使用 M3_CONSTANTS 中的冻结值
```

规则：`Td`、`sp` 和中间量必须有限，`sp > e > 0`，最终 `0 <= q < 1`；mask 取输入交集；quality/provenance 必须为 `Derived`。

NearSurface Profile 合同至少必须覆盖：10 m U/V、2 m T/q、roughness、PBL height、sensible/latent heat flux，以及 `friction_velocity` 或完整 surface stress 替代组。缺任一强制组不得发布 `NearSurfaceTransport`。

### A-P0-4：hybrid 获取/准备链破坏原始冻结资料

磁盘上的 `FETCH_MANIFEST.json` 与当前文件不一致：

| 文件 | FETCH 记录 size | 当前 size |
|---|---:|---:|
| `era5_hybrid137_20181201.nc` | 2,069,784 | 2,072,136 |
| `era5_surface_base_20181201.nc` | 121,640 | 124,060 |
| `era5_surface_flux_20181201.nc` | 40,071 | 42,518 |

`PREPARE_MANIFEST.json` 还记录了自身覆盖前的旧 size/hash。根因是 preparation 对原文件原地 stamp，并在覆盖 manifest 前把 manifest 自己纳入哈希。FETCH manifest 还残留 CDS key 片段。

A 裁决：

- CDS/NCEI 服务返回的 raw 文件必须 immutable；
- 所有 stamp、合并、A/B 注入和 convenience variable 写入独立 `prepared/ready` 文件；
- FETCH 只冻结 raw；PREPARE 只冻结确定性派生物；manifest 不得自哈希；
- manifest 不得记录 API key、key 前缀或其它认证片段；
- preparation 必须逐值校验 time/latitude/longitude 坐标一致，不能只比较 shape；
- pressure 获取链也应采用同样的 raw/normalized 分离，停止对“official”文件原地 stamp；
- hybrid 的 model-level、lnsp、surface base/flux、PV 系数获取应收敛到一个可从空缓存执行的正式入口，删除或明确废弃重复脚本和手工步骤。

### A-P1-1：`exp(lnsp)` 科学上通过，当前 provenance 不通过

A 接受：

```text
sp = exp(lnsp)
```

它符合 M3 计划中“lnsp or surface pressure”的正式输入合同。当前 prepared 文件把 `sp` 物化后按 `Source` 读入，会丢失 `Derived` quality/provenance。

正式 Profile 必须把 `lnsp` 作为 Source，并通过 `surface_pressure_from_log(lnsp)` 产生 canonical `surface_pressure`。若 prepared NetCDF 为满足 CF `formula_terms` 仍保留 convenience `sp`，Profile 也不得把该变量当成 canonical Source。

### A-P1-2：hybrid omega 路径通过

A 接受 ERA5 hybrid 主锚点使用 omega (`Pa s-1`)。M3 已冻结：当 hybrid 只有 omega 时，先求原生坐标速度，再走完整运动学几何 W；禁止 `-omega/(rho*g)` 近似。

B 不需要强行获取 etadot，但必须补真实 137 层完整查询，证明生产路径确实进入 hybrid omega -> native-coordinate velocity -> kinematic W，而不是只完成字段映射。

### A-P1-3：native 容差尚不能冻结

`TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md` 已规定：没有 registry 规则即“尚未认证”。现有测试中的 `1e-4/1e-5` 等硬编码阈值不是正式规则，且对同一未打包 NetCDF 锚点可能过宽。

B 下一轮先输出每个数据族/文件/变量/时次的 exact mismatch count、max abs、max rel 和最坏索引，不自行决定通过阈值。A 根据测量结果另行冻结 registry。

### A-P1-4：百万点只通过主路径，不是性能终审

现有测试证明：

- 1,024,017 点分块完成；
- 1 与 4 线程八列结果一致；
- 热缓存第二遍没有新增 column-cache miss。

尚缺：

- 实际/记账峰值内存或 RSS；
- 不同 chunk 大小；
- 输入排列变化；
- execute 期 reader/provider 调用计数为零；
- 冷/热缓存完整矩阵；
- N 与 2N 的耗时和临时内存近似线性证据。

因此本项标记为“百万点主路径通过，性能 A2 未通过”。

## 4. B 下一轮验收门槛

B 返工后必须实际证明：

1. 两套 ERA5 CLI 不再在 frame loading 失败；
2. pressure 与 hybrid 都以三个时次完成 `fetch -> manifest -> lock -> frame -> TransportPlan -> prepare -> prepare_batch -> execute -> probe/replay`；
3. `allow_estimated=false`，同时具有 Transport 和完整 NearSurfaceTransport；
4. pressure 37 层 `z` 保留，并分别证明 3-D/2-D `z` 的 role/layout 身份；
5. hybrid canonical `surface_pressure` provenance 为 `Derived`，源记录指向 `lnsp`；
6. ERA5 感热、潜热符号有正负数值测试，2 m 比湿有独立公式测试；
7. ASL、AGL、Pa，整帧/中间时刻，海面/平原/山地/高空，bounds/地下/顶界均有真实正向或负向证据；
8. raw 与 prepared manifest 可从空目录复现并全量自校验；
9. native 先交差分测量报告，不宣称容差认证；
10. 百万点补齐内存、I/O、chunk、排列、冷热缓存和线性证据。

## 5. 最终裁决

- **可接受：** CFSR 官方三时次查询工程链；hybrid omega 科学路径；`exp(lnsp)` 科学公式；oracle stub 的许可隔离。
- **暂不接受：** 两套 ERA5 完整查询、ERA5 NearSurface、pressure 逐层高度、hybrid reproducibility、native 数值认证、百万点性能终审、真实 FLEXPART oracle、Linux 矩阵。
- **项目状态：** M3 B Phase 2--4 有实质进展，但 A2 不能签署，M3 不能标记完成，也不应在当前状态提交为阶段完成。
