---
title: Domain-fill 水汽追踪教程
description: 配置、运行并解释一个可复现的 CFSR domain-fill 水汽实验。
---

# Domain-fill 水汽追踪教程

Domain filling 在指定气象域中维持代表空气质量的粒子 population。模型在初始化时分配
干空气质量，后续边界出生和正常终止通过质量账本核算。气象资料提供比湿采样，用于建立
水汽状态并开展水汽分析。

## Case 真源

--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"

在 resolved Case、RunProfile、data lock、reader 与可执行程序身份相同的前提下，固定
随机种子保证 population 采样可复现。

## 运行项目

准备快速入门资料后执行：

```text
trajecta --project examples/domain-fill-cfsr project data-plan --output data-plan.json
trajecta --project examples/domain-fill-cfsr project finalize
trajecta --project examples/domain-fill-cfsr run --profile product
```

可以从汇总和单粒子两个层级读取结果：

```text
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
```

Population 计数用于判断生命周期覆盖。Trajectory stream 按输出事件顺序展示单个粒子。
正式科研分析还应检查质量账本、正常终止原因、有限值审计和 full verify 结果。

## 扩展研究

先延长结束时间，再重新生成 data-plan，并为每个查询时刻提供完整的前后气象锚点。
测量目标输出频率下的内存占用后，再提高 `target_particle_count`。旧结果目录应作为独立
attempt 保留。
