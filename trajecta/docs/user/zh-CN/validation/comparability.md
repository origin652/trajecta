---
title: FLEXPART 可比较性矩阵
description: 查看 Trajecta 与 FLEXPART 的输入、coordinate、output、ensemble measure 与 runtime component 如何建立共同对比边界。
---

# FLEXPART 可比较性矩阵

轨迹对比先要确定每个 field 代表的物理量。两个产品都描述 atmospheric transport 时，file
format、variable name、vertical convention、output cadence 和 particle sampling 仍可能不同。
下表记录发布的一小时 Case 所采用的关系。

## 分类名称

| 分类 | 在本次对比中的含义 |
| --- | --- |
| `exact` | 两侧选择相同 source quantity 或 contract value，并直接检查相等性 |
| `aligned` | 经过已声明 conversion 或 packing tolerance 后，value 具有相同 unit 与 physical interpretation |
| `aggregate-only` | 对比 particle ensemble 的 statistic，不配对 individual record |
| `not-comparable` | 本 matrix 中的产品没有共同 quantity 或 cost boundary |

分类逐行适用。同一个 run 可以同时包含 exact input time、aligned height、aggregate-only
particle statistic 和 implementation-specific output work。

## 矩阵

| 维度 | 分类 | 使用的对齐方式 |
| --- | --- | --- |
| Official meteorological source file | `exact` | 两条 staging path 从同一组 frozen source byte 开始 |
| Trajecta prepared meteorology | `exact` | Prepared array 与 source array 比较；derived surface pressure 使用 bounded round-off |
| FLEXPART staged GRIB value | `aligned` | 每个 common field 保持在 source value 的 ecCodes packing error 内 |
| Valid time | `exact` | 固定四个 source frame 及其 UTC instant |
| Hybrid vertical definition | `aligned` | 对齐 model level、half-level coefficient、pressure interpretation 与 unit |
| Wind、temperature、humidity 与共同 surface field | `aligned` | Staging 后的 common physical field 使用相同 quantity 与 unit |
| Output instant | `exact` | 两侧 product 都包含 20、40 与 60 分钟 state |
| Particle count | `exact` | 每档 population 含选定的 10,000 或 50,000 个粒子 |
| Released mass contract | `exact` | 共同 Case 使用相等 total released mass 与 population size |
| Longitude 与 latitude | `aligned` | Geographic degree 转入共同 spherical metric calculation |
| Height | `aligned` | 两侧以 metre above sea level 进入 ensemble metric |
| Ensemble centroid | `aggregate-only` | 计算 spherical ensemble mean 之间的 great-circle separation |
| Horizontal dispersion | `aggregate-only` | 以 ratio 对比粒子到各自 ensemble centroid 的 RMS great-circle distance |
| Spatial occupancy | `aggregate-only` | 以 ratio 对比共同 0.1° × 0.1° × 250 m audit grid 中的 distinct cell |
| Individual particle trajectory | `not-comparable` | Random source sampling 与 integrator 不会产生共同 particle-ID assignment |
| SQLite 与 provenance work | `not-comparable` | 本 matrix 中的 FLEXPART product 没有对应 indexed history 与 provenance product |
| Particle NetCDF layout | `not-comparable` | Trajecta default product 为 SQLite，schema 与 lifecycle 不同 |

## 气象 value 对齐

气象 conversion 有两条路径：

```text
official source → Trajecta-ready NetCDF → Trajecta reader
official source → staged GRIB → FLEXPART reader
```

Trajecta-ready exact check 包括 coordinate axis、valid time、hybrid coefficient 与所需四维
field。FLEXPART staged value 从 GRIB 重新读取，并与共同 source quantity 比较。该路径使用
GRIB packing error 作为每条 message 的 tolerance。

Wet deposition 与 convection 已关闭。两个 zero precipitation message 用于满足所选
FLEXPART input structure。Trajecta surface-flux field 位于共同 advection quantity set 之外，
其存在不会给本 Case 增加 cross-model metric。

## 时间对齐

模拟从 2018-12-01 03:00 UTC 开始。两侧 product 在以下时刻读取：

| 经过时间 | UTC instant | Unix time |
| ---: | --- | ---: |
| 20 分钟 | 2018-12-01 03:20:00 UTC | 1543634400 |
| 40 分钟 | 2018-12-01 03:40:00 UTC | 1543635600 |
| 60 分钟 | 2018-12-01 04:00:00 UTC | 1543636800 |

程序可以在 600 秒 transport step 内部的其他时点执行计算。对比只读取这些共同 physical
output state。

## Coordinate 对齐

Longitude 与 latitude 进入 spherical mean 和 great-circle distance calculation。Height 以
metre above sea level 进入计算。取 vertical mean 前，FLEXPART stored particle height 会与
terrain field 相加；Trajecta 已按该 convention 存储 `height_asl_m`。

Occupancy grid 是本次对比使用的 diagnostic binning scheme，不会替代任一程序的
computational grid，也不会改变 trajectory。

## Individual particle ID 未配对的原因

两个程序初始化相同 population count 与 total mass，但 random generator 和 source-sampling
order 各自不同。随后，integrator 会通过独立 numerical implementation 推进这些 sample。
因此，numeric particle ID 只标识自己 product 内的 record。

不使用 ID 对应关系后，ensemble centroid、dispersion、vertical mean、transport distance 与
occupancy 仍可计算。这些指标描述共同输出时刻的 population 位置与展开程度。

## Runtime 可比较性

[性能方法](performance.md)提供 transport-oriented boundary 和 complete-product boundary。
Equivalent-core ratio 对比共同 transport 问题。Complete-product time 则显示各程序所选
default output 的实际运行成本，并保留 product 差异。

SQLite 与 provenance time 在本 matrix 中没有匹配的 FLEXPART component。因此，complete-
product column 用于描述，不计算 equivalence ratio。

## 扩展矩阵

研究改变以下条件时，可以建立新的 comparison contract：

- Simulation duration 或 transport step；
- Meteorological family、resolution 或 vertical coordinate；
- Release geometry 与 particle sampling；
- Turbulence、convection、chemistry 或 deposition setting；
- Output cadence 或 product format；
- Worker count、CPU allocation 或 host platform。

新 contract 可以沿用这些分类，同时为自身 workload 声明准确 field、conversion、time、
metric 与 timing boundary。当前 matrix 的 raw value 位于[数据与图表页](data.md)。
