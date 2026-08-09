---
title: 资料家族与轨迹方向
description: 对照 Trajecta 的 CFSR pressure、ERA5 pressure 和 ERA5 hybrid profile，并规划正反向资料覆盖。
---

# 资料家族与轨迹方向

Case 引用 `cfsr` 或 `era5-hybrid` 这样的逻辑数据集。项目索引把逻辑名称映射到公开
dataset profile，RunProfile 再提供本地文件。数值 Case 因此可以描述物理意图，无需写入
provider 目录或文件命名规则。

Dataset profile 涵盖的内容超过容器格式。它会识别源消息或变量，映射单位，描述帧间隔
与垂直坐标，派生其他字段，并公布 population 与边界模型选择的 capability group。

## Profile 对照

| 家族 | 公开 profile | 源容器 | 垂直坐标 | 帧间隔 | 教程区域 |
|---|---|---|---|---:|---|
| CFSR pressure | `cfsr-pgbl-pressure-v0` | GRIB2 | 37 个等压面，1–1,000 hPa | 6 小时 | 全球 144 × 73 网格 |
| ERA5 pressure | `era5-cf-pressure-netcdf-v0` | NetCDF3 或 NetCDF4 | 37 个等压面，1–1,000 hPa | 6 小时 | 北纬 53–45°、东经 0–10° 教程区域 |
| ERA5 hybrid | `era5-cds-hybrid137-v0` | 准备后的 NetCDF3 或 NetCDF4 | 137 个随地表气压变化的模式层 | 3 小时 | 北纬 53–45°、东经 0–10° 教程区域 |

三个 profile 都提供 transport、near-surface transport、domain-fill 和 diagnostics
capability。源变量与准备步骤各不相同，查询引擎会把它们转换为积分器使用的共同字段模型。

## CFSR 压力层

CFSR 教程读取低分辨率全球 `pgbl` 分析文件。压力层保存风与压力垂直速度，也包含气温、
比湿和位势高度。地表配套文件提供气压与地形，并补充 10 m 风、2 m 状态、边界层量和
瞬时通量。

全球网格的经度具有周期性。轨迹越过 180° 后从另一侧继续，因此 Case 选择
`global_periodic/v0`。源资料每六小时一帧，模拟区间外还要保留时间插值使用的相邻帧。
对于 06:00–06:10 的快速入门，资料助手会规划 00、06、12 和 18 UTC，覆盖取整后的
区间并向两侧各增加一帧。

Rust reader 直接读取 GRIB2；native reader 使用安装包中的 ecCodes。DatasetLock 会记录
profile、源文件散列、有效时次、网格签名、压力层签名和 capability set。

## ERA5 压力层

ERA5 pressure 教程请求六个三维变量和 37 个压力层。地表请求先补充气压、位势和近地风，
随后加入热力字段。边界层高度、粗糙度、摩擦速度和瞬时湍流通量位于同一组配套资料中。

准备工具生成带有 `era5_cf_pressure_netcdf` family marker 的文件。Profile 可以读取
准备后的 NetCDF3 或 NetCDF4，并将两种容器映射到相同 canonical fields。位势会转换为
位势高度；2 m 比湿从露点温度和地表气压派生；源通量符号也会转换成 Trajecta 采用的
向上为正规则。

教程区域是有限域。`limited_domain_terminate/v0` 会在轨迹与水平边界的连续交点终止
粒子。该流出记录为正常终止，因此后续输出事件中的活动粒子数可能降低。

## ERA5 hybrid 模式层

Hybrid profile 使用 ERA5 全部 137 个模式层。每层气压由层系数和本地地表气压共同决定。
准备流程会把三维模式层字段与对数地面气压、地表配套字段和官方系数表组合起来。

主要三维请求包含气温、两个水平风分量、比湿和压力垂直速度。Profile 从对数地面气压
派生地表气压，并为 ozone initializer 使用的 diagnostics capability 派生 potential
vorticity。

Hybrid anchors 相隔三小时。反向 ozone 示例覆盖 05:50 至 06:00 UTC，助手会规划
00、03、06 和 09 UTC。准备后的文件保留在每个水平位置和时刻重建垂直坐标所需的
metadata。

## 正向与反向物理时间

正向 Case 的 `time.start` 早于 `time.end`。积分器沿正时间方向推进，输出事件的物理
时间递增，release event 也按该方向到达。

反向 Case 的 `time.start` 晚于 `time.end`。配置中的步长仍写正的大小，执行方向会使其
成为负的时间增量。结果记录保留物理时刻，event sequence 则反映 worker 实际访问顺序。

Data-plan 总是按时间先后写出 `coverage_start` 和 `coverage_end`。资料助手随后按 profile
cadence 对物理区间取整，并加入相邻 anchors。正向与反向运行的 provider request 因此
可能很相似，数值执行方向仍由 Case 决定。

## 空间覆盖与 halo

每个 Case domain 都声明 `horizontal_halo_cells`。安全查询区域会向网格内部收缩相应格点，
为边缘附近的插值保留支撑。全球 CFSR 网格在经度方向衔接；有限 ERA5 网格则具有真实的
北、南、东、西边界。

两个 ERA5 教程没有可用于推导范围的 release geometry，因此助手采用已记录的教程区域：
北界 53°、西界 0°、南界 45°、东界 10°。含明确坐标的 release Case 会从 geometry
生成外扩两度的资料区域。Global periodic Case 会规划全球范围。

## 更换资料家族或方向

完整的资料家族变更包含以下步骤：

1. 把逻辑数据集映射到对应的公开 dataset profile。
2. 按新的空间覆盖调整 Case domain 与 boundary policy。
3. 重新生成 data-plan，查看 capability、区域、cadence 和 time anchors。
4. 按该资料家族的布局准备文件。
5. Finalize 项目，创建新的 DatasetLock。
6. 先运行小型 Case，再增加时长或 population。

方向变化还会影响 scheduled release 和事件顺序的解释。调整后应让 `time.start`、
`time.end` 与 `direction` 保持一致，并重新生成 plan。
[覆盖与身份](../concepts/coverage-identity.md)进一步说明这些选择如何进入 run identity。
