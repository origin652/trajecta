---
title: 原始验证数据与图表
description: 下载 Trajecta 与 FLEXPART 比较使用的固定 JSON 和 CSV，查看五张图表，并在本地重新生成。
---

# 原始数据与图表

验证资源包含完整比较汇总、气象对齐记录、两份适合分析的 CSV，以及五张静态 SVG 图。文件随
文档提交，站点重新构建时，图表仍对应同一组原始行。

源汇总的 SHA-256 为：

```text
b8d0f5befcf3bcf0c9cbf9ac70faa7ff708b676d906f6c27593d7202ab2a3b39
```

## 下载原始文件

| 文件 | 内容 |
| --- | --- |
| [M5_A5_FLEXPART_COMPARISON.json](../assets/validation/raw/M5_A5_FLEXPART_COMPARISON.json) | 主机与负载条件、可执行文件版本和散列、全部运行记录、中位数、规模扩展、科学汇总和检查状态 |
| [M5_A5_METEOROLOGY_EQUIVALENCE.json](../assets/validation/raw/M5_A5_METEOROLOGY_EQUIVALENCE.json) | 源文件与准备后文件散列、数组形状、共同字段分类、GRIB 打包检查和对齐结果 |
| [M5_A5_TIMINGS.csv](../assets/validation/raw/M5_A5_TIMINGS.csv) | 每次预热或正式样本的墙钟时间、常驻内存、CPU 时间、文件系统计数、模式和运行顺序 |
| [M5_A5_SCIENCE.csv](../assets/validation/raw/M5_A5_SCIENCE.csv) | 每种粒子数、重复轮次和共同输出时刻的五项跨模型集合统计 |
| [PUBLICATION_MANIFEST.json](../assets/validation/PUBLICATION_MANIFEST.json) | 源汇总散列及每张发布图表的 SHA-256 |

`raw/original-charts` 保留正式运行直接生成的图。下方发布图根据汇总文件重新绘制，科学横轴使用
记录中的 20、40 和 60 分钟输出时刻。

## 计时 CSV 字段

`M5_A5_TIMINGS.csv` 包含：

| 列 | 含义 |
| --- | --- |
| `particles` | 请求的粒子数 |
| `phase` | `warmup` 或 `formal` |
| `repetition` | 预热为 0，正式样本为 1–3 |
| `order_index` | 当前重复中该模式的轮换位置 |
| `mode` | `flexpart-core`、`flexpart-product` 或 `trajecta-product` |
| `core_seconds` | 该模式提供输送计时边界时的秒数 |
| `product_seconds` | 该模式从启动到完整结果写出所用的秒数 |
| `peak_rss_bytes` | 进程峰值常驻内存 |
| `user_seconds`、`system_seconds` | `/usr/bin/time -v` 返回的 CPU 时间 |
| `filesystem_inputs`、`filesystem_outputs` | 同一工具返回的文件系统计数 |

核心或产品列为空，表示该模式只提供另一类计时边界。重新计算公开中位数前，先筛选
`phase == formal`。

## 科学 CSV 字段

`M5_A5_SCIENCE.csv` 的每次重复包含三个输出时刻：

| `physical_time_unix` | 已经过时间 |
| ---: | ---: |
| 1543634400 | 20 分钟 |
| 1543635600 | 40 分钟 |
| 1543636800 | 60 分钟 |

其他列按字段名采用米或无量纲比值。[科学验证方法](science.md)给出各统计定义和正负号约定。

## 发布图表

### 等效核心墙钟时间

![10,000 与 50,000 粒子的等效核心中位墙钟时间](../assets/validation/charts/core-wall-time.svg)

图中比较输送相关中位数。10,000 粒子下分别为 1.330 和 0.465 秒；50,000 粒子下分别为
1.340 和 1.469 秒。

### 完整产品墙钟时间

![10,000 与 50,000 粒子的完整产品中位墙钟时间](../assets/validation/charts/complete-product-wall-time.svg)

该图采用两个程序各自的完整结果输出流程。50,000 粒子下，粒子 NetCDF 写出用时 1.370 秒，Trajecta
SQLite 与溯源产品为 8.680 秒。

### 吞吐随粒子规模变化

![10,000 与 50,000 粒子的等效核心吞吐](../assets/validation/charts/throughput-scaling.svg)

吞吐以粒子数除以等效核心中位秒数。图中同时反映短案例的固定开销和随粒子数增加的输送成本。

### 峰值常驻内存

![核心与产品模式的中位峰值常驻内存](../assets/validation/charts/peak-rss.svg)

峰值常驻内存取三次正式运行的进程最大值中位数，并按 1 MiB = 1,048,576 字节换算。

### 科学粒子群比较

![20、40 和 60 分钟时的水平与垂直粒子群差异](../assets/validation/charts/scientific-comparison.svg)

科学图展示两种粒子数的水平质心距离和带符号垂直质心差。离散度、输送距离与空间占据的完整数值
保存在 CSV。

## 在本地重建图表

从仓库 `trajecta` 目录运行：

```text
python tools/validate_m5_1_validation_assets.py
```

命令会：

1. 读取固定汇总状态和源文件散列；
2. 在临时目录重新生成发布图；
3. 将每张新图的散列与 `PUBLICATION_MANIFEST.json` 比较；
4. 比较英文与中文站点使用的验证资源是否逐字节一致。

命令不会改写已经提交的图表。需要更新图表时，应先加入新的固定汇总或经过审阅的绘图变更，再
重新生成图表，并在同一提交中更新发布清单。

## 在其他分析中使用

CSV 可以直接交给 R、Python、Julia、电子表格或命令行表格工具。重新汇总性能时，保留
`particles`、`phase`、`repetition` 和 `mode` 作为分组列。处理科学行时，按粒子数和物理时刻
分组；即使当前三次重复数值相同，也应将它们保留为独立观测。

引用表格或派生图时，可以一并记录本页顶部的汇总 SHA-256，使引用唯一对应此次负载、可执行文件、
原始行和派生数值。
