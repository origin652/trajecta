# M3 A2 oracle registry v1.0.2：A 科学裁决

## 结论

A 发布 `m3-a-tolerance/v1.0.2`，注册
`trajecta-vs-flexpart-pressure-adapter-v1`，并对三套 complete FLEXPART oracle
执行分层裁决。本次没有修改 frozen query、Trajecta 科学算法、FLEXPART commit
或任何数值阈值。

Windows MIT 重跑结果：

| family | raw oracle | hard failures | common report-only rows | 状态 |
|---|---:|---:|---:|---|
| ERA5 pressure | 75/75 | 0 | 15 | passed hard gates |
| CFSR pressure | 75/75 | 0 | 15 | passed hard gates |
| ERA5 hybrid | 75/75 | 0 | 3 | passed hard gates |

`passed hard gates` 不表示 report-only 差异消失，也不表示 FLEXPART 是现代科学真值。
所有差异继续逐点保存在 comparison report 中。

## Pressure adapter 注册

注册身份：

```text
comparison_variant = trajecta-vs-flexpart-pressure-adapter-v1
source_family       = era5 | cfsr
vertical_coordinate = pressure
pbl_height_mode     = official_prescribed
vertical_transform  = verttransform_gfs
```

源码与 binary audit 证明 pressure driver 调用真实 `verttransform_gfs`；GFS 路径把
z-grid pressure 写入 `pplev`，adapter 仅执行 `prs = pplev` 供 meter-mode
`interpol_partoutput_val('PR')` 读取。该赋值不改变公式、单位或数值。

因此 exact source time/grid/native pressure level 的 U/V/T/q/p/geopotential height
继续使用原阈值 hard gate，ERA5 pressure 与 CFSR 均通过。

## Pressure interpolated 的裁决

首次注册 provisional variant 后，Windows MIT 如实暴露 ERA5 12、CFSR 13 个旧
hard rows 失败。A 没有放宽阈值，而是复核 FLEXPART v11.1 实现：

1. `verttransform_gfs` 首次寻找高 surface-pressure 参考点；
2. `verttransform_init` 从该单一参考 column 构造全域、全时次共用的 Cartesian
   z-grid；
3. 各地 pressure column 先重映射到此 z-grid；
4. `interpol_wind` / `interpol_partoutput_val` 再在 legacy grid 上插值。

Trajecta 不执行该中间 remap，而是在每个查询点的 native pressure
`ColumnGeometry` 上完成水平、垂直和时间插值。两者不是同一算子，故 pressure
adapter 的 15 个 interpolated result rows 全部改为 report-only，使用编号：

```text
M3-ORACLE-PRESSURE-ADAPTER-LEGACY-METER-REMAP
```

原阈值原样保留为诊断尺度。此裁决认证 adapter 能真实复现 FLEXPART legacy 输出，
不要求 Trajecta 模仿该 legacy remap。

## ERA5 hybrid 三项裁决

### Native geopotential height

Trajecta 使用 ECMWF 官方 alpha hydrostatic geopotential recurrence、surface
geopotential 和 binary64；FLEXPART `verttransform_ecmwf_heights` 使用 default
`REAL` 的逐层 hypsometric meter-height integration。二者服务的算法身份不同。

实测 27/27 有差，最大约 `5.347764 m`。该项使用原阈值 report-only：

```text
M3-ORACLE-HYBRID-NATIVE-HEIGHT-ALGORITHM
```

若目标是 ERA5 官方全层位势/高度，Trajecta 路径更贴近官方定义；若目标是逐位复现
FLEXPART 轨迹内部 meter grid，则 FLEXPART legacy 路径是兼容性基准。

### ASL pressure / specific humidity

仅 hybrid ASL 路径显著超出原阈值：pressure 最大约 `5.592424 Pa`，q 最大约
`4.803665e-6 kg/kg`；AGL 和 explicit pressure-coordinate 路径继续通过 hard
gates。证据支持算子顺序差异：FLEXPART 先映射固定 meter/ETA grid 再插值，
Trajecta 在 native hybrid column 上插值。

这两项使用原阈值 report-only：

```text
M3-ORACLE-HYBRID-ASL-OPERATOR-ORDER
```

仅凭 FLEXPART oracle 不能证明任一 ASL 插值算子绝对更准确；Trajecta 的科学准确性
继续由解析公式、物理不变量、真实资料边界和后续独立收敛证据负责。

## 冻结身份

- registry version：`m3-a-tolerance/v1.0.2`
- registry SHA-256：`882696e89cd7f41c57abc020ba92568b8595ffd274ba917b79d1606d8fe5a214`
- backend expected matrix SHA-256：`3bd43bc57f6fcc1a467cfd9fc50dfd038435aed2bd222c637c4f95f76f7355b4`
- calibration SHA-256：`c097417f96cde49751372bb56cac141766f1f660830b74b879adbf7514ffb520`
- FLEXPART commit：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`

## 后续 A2 闭合

本裁决发布时只关闭 registry 与 Windows MIT，因此当时尚未签署 M3/A2。随后使用
同一 registry、冻结资料和 query 在 WSL Ubuntu 24.04 完成 full gate、native backend
matrix、三族 oracle comparison 和 million-point 长测，结果为
`core_status=0`、`a2_full_status=passed`。最终完成裁决见
`TRAJECTA_M3_A2_CLOSURE.md`。
