---
title: Trajecta 教程
description: 通过四个可执行科研工作流学习 domain filling、常规释放、气团和臭氧 population。
---

# 教程

每条教程对应 `examples/` 下的一个完整项目。示例文件是配置真源，站点构建时从中提取
Markdown 片段。所有路径均为项目相对路径，可用于 Windows 和 Ubuntu 24.04。

| 教程 | Population | 资料家族 | 方向 |
|---|---|---|---|
| [Domain-fill 水汽追踪](domain-fill.md) | 干空气 domain fill | CFSR pressure | 正向 |
| [常规 release](release.md) | 定时释放 | CFSR pressure | 正向 |
| [Air-mass 工作流](air-mass.md) | 干空气 domain fill | ERA5 pressure | 正向 |
| [Ozone 工作流](ozone.md) | 平流层臭氧 domain fill | ERA5 hybrid | 反向 |

这些 Case 的模拟时长较短。扩大时间覆盖或粒子数前，应确认 `project data-plan`、
`project finalize` 与 `doctor --deep` 对扩展后的研究给出一致结论。
