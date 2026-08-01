---
title: 资料家族与轨迹方向
description: 对照 CFSR pressure、ERA5 pressure、ERA5 hybrid 资料及正反向覆盖要求。
---

# 资料家族与轨迹方向

不同资料家族采用相同的科学工作流。文件身份、帧间隔、垂直坐标、能力集合和空间覆盖
各有差异。

| 家族 | 公开 profile | 垂直坐标 | 标称帧间隔 | 教程范围 |
|---|---|---|---:|---|
| CFSR pressure | `cfsr-pgbl-pressure-v0` | 等压面 | 6 小时 | 全球四帧演示资料 |
| ERA5 pressure | `era5-cf-pressure-netcdf-v0` | 等压面 | 6 小时 | 官方准备流程的有限区域 |
| ERA5 hybrid | `era5-cds-hybrid137-v0` | 137 个模式层 | 3 小时 | 官方准备流程的有限区域 |

正向运行从 `time.start` 推进到较晚的 `time.end`。反向运行在数值上推进到较早的结束
时刻。Data-plan 将物理覆盖规范化为有序的 `coverage_start` 和 `coverage_end`。当 reader
需要额外插值锚点时，provider request 必须覆盖轨迹区间之外的相邻帧。

请勿只更换文件名来复制教程。应重新生成 data-plan，选择匹配的 dataset profile，检查
所需能力，并通过显式 finalize 创建新的 DatasetLock。
