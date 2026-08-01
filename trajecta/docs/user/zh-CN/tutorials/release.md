---
title: 常规 release 教程
description: 使用显式质量、几何、垂直位置和可复现输出来运行点源释放实验。
---

# 常规 release 教程

Release-driven population 通过一个或多个事件创建粒子。每个事件定义时间区间、粒子数、
物质质量、几何形状和垂直位置。当研究问题包含明确源项时，可以采用该模型。

## Case 真源

--8<-- "examples/release-cfsr/cases/release.yaml"

示例在一个点释放 1,000 个粒子，并在事件中分配一千克 `water` tracer。请按具体研究
调整质量和几何参数；教程数值不代表排放清单。

## 执行与读取

将四帧演示资料复制或链接到 `examples/release-cfsr/data`，随后运行：

```text
trajecta --project examples/release-cfsr project validate
trajecta --project examples/release-cfsr project finalize
trajecta --project examples/release-cfsr run --profile product
trajecta result verify RESULT --full
```

`result trajectory` 用于读取单条路径。`result inspect` 汇总释放、终止、生命周期和质量
状态。发表分析时，应同时保存结果目录中的 resolved Case 与 Profile。
