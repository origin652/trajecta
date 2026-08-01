---
title: 气象资料与数值路径
description: Trajecta 的源资料索引、Profile 选择、查询准备、插值、domain filling 和粒子积分路径。
---

# 气象资料与数值路径

## 气象资料加载

`project finalize` 向运行时提供 DatasetLock，其中包含 Profile 身份、文件摘要、时间覆盖、
网格签名、垂直签名和能力。Worker 会在加载资料前校验该绑定。

随后执行以下气象资料路径：

1. 只扫描声明的数据根目录；
2. 将 container 与请求的 dataset Profile 匹配；
3. 索引源生坐标、role、有效时刻和字段；
4. 使用显式质量与 provenance 派生规范字段；
5. 在粒子执行前准备 frame window 和查询结构。

粒子循环只读取已准备的内存。性能合同禁止 execute 阶段隐藏访问源资料文件。

## 查询与积分

查询引擎在源资料拓扑上执行空间与时间插值，并返回有类型的状态、质量、数值和
provenance 归属。Core integrator 使用该公开结果，按所选方向和时间步推进粒子状态。
边界策略负责分类越界与终止条件。

冻结 cache 合同允许复用完全相同的查询。复用不能合并科学含义不同的 key，也不能改变
规范化输出身份。

## Domain filling

Domain-fill population 从气象资料构造原生大气网格单元，计算有效层质量，并根据所选
air-mass 或 stratospheric-ozone 规则分配粒子。边界入流可以建立后续 cohort。稳定粒子
身份与 worker 调度无关，因此确定性输出不依赖线程数。

修改该路径时，需要执行真实资料 replay、质量账本验证、确定性检查和相应数值合同。
