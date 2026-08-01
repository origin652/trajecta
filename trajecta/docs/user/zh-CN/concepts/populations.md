---
title: 粒子 population 模型
description: 对照 Trajecta 的 release-driven、干空气 domain-fill 和平流层臭氧 population。
---

# 粒子 population 模型

| 模型 | 出生定义 | 携带状态 | 主要用途 |
|---|---|---|---|
| Release-driven | 含几何和质量的定时事件 | 一种或多种物质 | 源项与受体轨迹 |
| 干空气 domain fill | 气象域分层采样 | 干空气质量及湿度背景 | 水汽和气团追踪 |
| 平流层臭氧 domain fill | 气团填充加冻结臭氧规则 | 干空气与臭氧质量 | 臭氧输送研究 |

三种模型使用相同的轨迹积分器、气象查询边界、生命周期核算、输出 sink 和 provenance
系统。它们的初始化方法和携带质量存在差异。

一个 Case 选择一种 population strategy。Population ID、release event ID、substance ID、
域引用、随机种子和目标粒子数均进入 resolved 科学身份。修改这些项目需要创建新运行；
能力或覆盖随之变化时，还要生成新的 data-plan 和 DatasetLock。
