---
title: 粒子 population 模型
description: 比较 Trajecta 中的 scheduled release、dry-air domain fill 和 stratospheric-ozone population。
---

# 粒子 population 模型

Particle population 定义粒子的出生方式、携带的质量、稳定身份构造，以及轨迹积分前需要的
气象 capability。一份 Case 选择一套完整 population strategy。

Trajecta `0.1.0-alpha.1` 提供三种内置策略：

| Strategy | 初始或 scheduled source | 粒子权重或质量 | 常见研究形式 |
| --- | --- | --- | --- |
| `release_driven` | 显式 release event | Event substance mass 在粒子间分配 | 源到受体输送与受体释放 |
| `domain_fill_air_mass` | 一个气象域内的干空气质量 | 相等 dry-air carrier mass | 水汽归因、停留时间与 air-mass 输送 |
| `domain_fill_stratospheric_ozone` | 一个气象域内符合条件的干空气质量 | Dry-air carrier 加派生 ozone mass | 平流层臭氧输送 |

三种策略共用球面轨迹 integrator、气象 query engine、boundary policy、output schedule、SQLite
sink 和结果生命周期。初始化方式与携带质量各有不同。

## Release-driven population

Release population 包含一个或多个具名 event：

```yaml
particle_population:
  strategy: release_driven
  id: release
  events:
    - id: source-a
      start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
      end: { seconds_since_unix_epoch: 1230793200, nanosecond: 0 }
      particle_count: 1000
      mass:
        water: { value: 10, unit: kg }
      geometry:
        source: inline
        geometry: { type: Point, coordinates: [8.5, 47.4] }
      vertical:
        coordinate: above_ground
        lower: { value: 100, unit: m }
        upper: { value: 500, unit: m }
```

每个 event 定义 inclusive time interval、确切粒子数、水平 geometry、垂直坐标，以及各项已
声明 substance 的总质量。

### Birth time

Instantaneous event 的 `start` 与 `end` 相等，全部粒子在同一时刻出生。Interval event 会按
粒子数把时段分成 integer-nanosecond stratum，每个 stratum 内确定性采样一个 birth time。
出生由此分布在指定时段内，不依赖线程顺序。

Integrator 让每个 cohort 从自己的确切 birth time 开始。Macro-step 中途出生的粒子只推进该
step 剩余的物理时长。

### 水平与垂直放置

Release geometry 接受 Point、MultiPoint、line、multiline、polygon 和 multipolygon GeoJSON。
Geometry 可以直接写在 Case 中，也可以从相对于 Case 的本地文件加载。Point 采用等权重，
line 使用 geodesic length，polygon 使用球面面积并处理 hole。

垂直位置可以使用 mean sea level 以上高度、本地 ground 以上高度或 atmospheric pressure。
只有 lower 值时表示固定坐标；增加 upper 后在所选坐标的区间内均匀采样。Above-ground 和
pressure release 通过所选气象状态解析。

### Substance mass

每个 event 声明的总质量按确切粒子数分配。最后一个粒子采用浮点补偿份额，使存储的 share
重新求和后回到 event total。Release particle 的 `dry_air_mass_kg` 为零，声明的 substance
保存在 `particle_mass`。

同一个 population 可以包含多个 event，并共享 substance 定义。Event ID 在 population 中
保持唯一，也会进入 particle origin 与稳定身份。

## Dry-air domain-fill population

Air-mass strategy 从一个气象域创建粒子：

```yaml
particle_population:
  strategy: domain_fill_air_mass
  id: regional-air
  domain_id: limited
  target_particle_count: 50000
```

初始化根据气象网格推导干空气质量，再通过确定性分层采样放置等 carrier-mass 粒子。Case
可以指定确切初始粒子数，也可以指定每个粒子的目标干空气质量。

有限域在 inward boundary flux 累计到一个 carrier mass 时增加粒子，离开域的粒子则终止。
全球周期域没有侧向交换。逐 step mass ledger 跟踪 active、incoming、outgoing、terminated
与 residual dry-air mass。

[Domain-fill 概念](domain-fill.md)进一步说明该生命周期及其水汽分析方式。

## Stratospheric-ozone domain fill

Ozone strategy 包含一份 dry-air domain-fill 设置。`air_mass` 提供 population ID、domain，
以及 target count 或 carrier mass。`ozone_rule` 选择具名内置 assignment rule；
`ozone_substance` 指定接收 derived ozone mass 的 Case substance column。随软件提供的 ozone
示例包含完整配置。

该策略先推导 dry-air
snapshot，再把初始化范围限制到当前规则的 stratospheric eligibility mask。规则要求 geometric
height 严格高于 3,000 m，且按南北半球统一符号后的 potential vorticity 严格高于 2 PVU。

Mask 内的各粒子携带相等 eligible dry-air mass。Ozone mole fraction 按每 PVU 60 parts per
billion by volume 推导，再结合 dry-air carrier mass 与 molar-mass ratio 转换为 ozone mass。
结果保存在 `ozone_substance` 对应的质量列。

有限域 birth 只发生在 inflow face 的 eligible 部分，并在出生位置使用同一规则赋予 ozone
mass。Dry-air mass ledger 继续负责 population conservation；ozone mass 是粒子携带的
substance。

该策略需要 domain-fill capability，以及推导 potential vorticity 所需的 diagnostic field。
ERA5 hybrid 示例给出了此流程使用的完整 137-level 输入路径。

## 稳定 particle identity

Particle ID 采用确定性构造，与 worker 调度无关。不同 origin 使用以下组成信息：

| Origin | Identity 组成 |
| --- | --- |
| Release | Population ID、event ID 与 event-local ordinal |
| Initial domain fill | Population ID、domain ID 与 mass-stratum ordinal |
| Boundary domain fill | Population ID、domain ID、boundary-face ID、lifecycle event index 与 birth ordinal |

数值 particle ID 和 origin details 一起保存在 SQLite。Resolved scientific input 相同的 rerun
会得到相同稳定 particle identity；run ID 与 attempt 则会更新。

## Population 与方向

Direction 位于 Case time specification。Population 在 Case start 或 scheduled birth time 创建
粒子，随后 integrator 沿所选方向向 end 推进。

Release event time 位于 Case 的物理区间内。Domain filling 则按积分方向解释 boundary inward
与 outward flux。同一个有限域 face 在一个方向中可能是 inflow，在另一个方向中可能成为
outflow。

## 气象 capability

每类 population 都需要 transport field，并按策略增加以下要求：

| Population | 额外 capability |
| --- | --- |
| Release-driven | 所选 release coordinate 需要的 geometry 与 vertical-resolution field |
| Dry-air domain fill | 用于 mass、terrain、vertical support 和 boundary flux 的 domain-fill snapshot field |
| Ozone domain fill | Domain-fill field 加 potential-vorticity diagnostic |

`project data-plan` 写出推导后的 capability set。`project finalize` 在创建 DatasetLock 前，对照
dataset profile 和实际检查的文件确认这些能力。

## 选择 strategy

可以从研究中需要表示的物理对象选择：

- 已知 source geometry 和 release period 对应 `release_driven`。
- 按大气质量加权的区域或全球空气对应 `domain_fill_air_mass`。
- 携带内置 ozone proxy 的平流层 air-mass population 对应
  `domain_fill_stratospheric_ozone`。

Population 变化会影响 particle origin、carrier 语义、所需气象量和 scientific identity。修改
strategy 或其 domain 后，重新生成 data-plan 并 finalize 项目。
