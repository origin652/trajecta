---
title: 科学验证方法
description: 了解 Trajecta 所采用的气象对齐、粒子核算、集合指标与冻结一小时科学对比。
---

# 科学验证方法

发布的科学对比让两个独立轨迹实现运行同一个紧凑 advection Case，观察气象资料交接和粒子
集合在共同物理时刻的演变。两侧程序先完成各自生命周期与产品检查，随后才将输出用于
对比。

## 冻结 Case

| 设置 | 值 |
| --- | --- |
| 气象资料 | Official ERA5 hybrid-level source content |
| 输送起点 | 2018-12-01 03:00:00 UTC |
| 模拟时长 | 3,600 秒 |
| Transport step | 600 秒 |
| 共同输出间隔 | 1,200 秒 |
| 对比输出时刻 | 03:20、03:40 与 04:00 UTC |
| 粒子 population | 10,000 与 50,000 |
| CPU allocation | CPU set `0-3` 上的四个 thread |
| 对比中启用的过程 | Advection 和 metric set 使用的共同 trajectory state |

集合指标使用的源位置为 5.25° E、49.25° N。本 Case 关闭 wet deposition 和 convection。
Trajecta prepared dataset 中仍保留 surface-flux field，但这些 field 不进入共同 advection
quantity set。

## 气象资料对齐

两条执行路径从同一组 official source file 开始。Reader 与 staging format 各自不同，因此
对齐工作分为几个层级。

### Source identity

Source NetCDF 与 GRIB 文件通过 byte length 和 SHA-256 固定。Trajecta prepared file 与
staged FLEXPART GRIB 也有从这些 source 派生的各自 identity。完整列表位于
`M5_A5_METEOROLOGY_EQUIVALENCE.json`。

### Field 与 coordinate

对齐内容包括：

- 四个 valid time；
- 137 个 model level 和 138 个 hybrid half-level coefficient；
- Longitude 与 latitude axis；
- Horizontal wind、vertical motion、temperature 与 specific humidity；
- Logarithmic surface pressure 及其派生 surface-pressure value；
- 所选 advection setup 使用的 surface 与 near-surface field。

Trajecta-ready array 直接与 prepared source array 比较。由 logarithmic pressure 重建的
surface pressure 使用明确 round-off tolerance。为 FLEXPART staging 的 GRIB value 则在每条
message 的 ecCodes packing error 以内比较。这样可以容纳 GRIB packing 引入的 quantization，
同时保持 unit 与 physical interpretation 对齐。

### Query coverage

Reader check 覆盖 Case 所需的物理时刻、grid location 和 vertical range。Dataset safe spatial
core 内的 query 应返回指定 field 与有限值。DatasetLock 固定 production run 随后使用的文件
和 coverage。

## Trajecta run 检查

Trajecta 输出进入 cross-model metric 前，run 需要以 `complete` 收尾，abnormal termination
为零。Full verification 包含：

| 区域 | 检查内容 |
| --- | --- |
| Population | Initial count、active count 与全部 termination 保持相互一致 |
| Lifecycle | Scheduled output state 有序，每个粒子具有一条有效 state path |
| 数值 | Coordinate / height / time / population quantity 为有限值，并满足各自 constraint |
| Mass ledger | 每个 event 的 initial / active / boundary / terminated mass 闭合 |
| Boundary | Domain 与 vertical boundary classification 符合所选 Case |
| Output | Manifest count 与 SQLite 一致；integrity 通过；terminal WAL 已 truncate 或不存在 |
| Provenance | Resolved input、software identity 与 canonical output digest 完整 |
| 确定性 | Formal repetition 保持规定 normalized content、SQL 与 canonical identity |

对应 FLEXPART product run 也要成功结束，并包含预期粒子数和三个共同输出时刻。

## 集合指标定义

每个共同物理时刻只使用有限 active particle coordinate 计算指标。

### 水平质心

经纬度先转换为单位球面 Cartesian coordinate，取平均后再转回 geographic coordinate。这种
spherical mean 可以处理 longitude wraparound。水平质心距离按 great-circle distance 计算，
使用 6,371,008.8 m mean Earth radius。

### 垂直质心

高度以 metre above sea level 表示。FLEXPART particle product 在计算集合平均前，将 terrain
height 加到 stored height。发布的 vertical difference 为：

```text
Trajecta mean height ASL - FLEXPART mean height ASL
```

### 输送距离

分别计算两个质心到 5.25° E、49.25° N 的 great-circle distance。表中报告 Trajecta distance
减去 FLEXPART distance。正值表示该输出时刻的 Trajecta 质心离 source 更远。

### 水平离散

先计算每个粒子到自身集合质心的 great-circle distance，再对这些距离取 root mean square，
得到 horizontal dispersion width。发布 ratio 为：

```text
Trajecta RMS dispersion / FLEXPART RMS dispersion
```

### Occupied audit cell

Coordinate 被分配到 diagnostic grid。网格分辨率为 0.1° longitude、0.1° latitude 和 250 m
height。Occupancy 是至少包含一个粒子的 distinct cell 数量。发布值以 Trajecta count 除以
FLEXPART count。

## 发布的集合值

每档 population 的 formal repetition 得到相同 scientific row，因此各时刻 median 与下表
相同。

### 10,000 个粒子

| 经过时间 | 水平质心距离 | 垂直差 | 输送距离差 | Dispersion ratio | Occupancy ratio |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 min | 214.89 m | -22.46 m | 214.68 m | 1.00116 | 0.98148 |
| 40 min | 229.48 m | -37.87 m | 214.22 m | 1.00191 | 1.01802 |
| 60 min | 276.55 m | -49.79 m | 175.45 m | 1.00373 | 0.98291 |

### 50,000 个粒子

| 经过时间 | 水平质心距离 | 垂直差 | 输送距离差 | Dispersion ratio | Occupancy ratio |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 20 min | 60.66 m | -19.29 m | 30.45 m | 1.00288 | 0.98182 |
| 40 min | 143.45 m | -34.64 m | 22.25 m | 1.00344 | 0.99130 |
| 60 min | 271.88 m | -46.43 m | -20.33 m | 1.00514 | 0.97521 |

50,000 粒子集合减小了早期质心采样差异；60 分钟时，两档 population 的质心距离相近。
Dispersion ratio 在整个 Case 中接近一。垂直差为负，表示共同输出时刻的 Trajecta 质心略低于
FLEXPART 质心。

## 结果适用范围

该对比对应一小时 ERA5 hybrid-level advection Case，参数见本页开头表格。两个程序采用
不同随机采样与数值路径，因此使用集合统计量。一个输出中的 particle ID `42` 没有分配到
另一输出中的对应粒子。

Chemistry 与 deposition 需要单独的 aligned case 和 metric。Convection 与 turbulence
configuration 也需要另行设置。Long-duration accumulation、其他 grid 和其他 release
geometry 同样应使用符合其问题的 comparison contract。
[可比较性矩阵](comparability.md)会标出本次发布对比中的 exact、aligned、aggregate-only 与
未纳入项目。
