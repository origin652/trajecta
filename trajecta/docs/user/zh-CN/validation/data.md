---
title: 冻结验证数据与图表
description: 下载冻结 Trajecta 与 FLEXPART 对比 JSON 和 CSV，阅读五张发布图，并在本地重建。
---

# 冻结数据与图表

Publication bundle 包含 aggregate comparison、meteorological alignment record、两份便于分析
的 CSV 和五张静态 SVG。文件随文档提交，因此站点重新构建后，图表仍对应同一组 row。

Source aggregate 的 SHA-256 为：

```text
b8d0f5befcf3bcf0c9cbf9ac70faa7ff708b676d906f6c27593d7202ab2a3b39
```

## 下载 raw file

| 文件 | 内容 |
| --- | --- |
| [M5_A5_FLEXPART_COMPARISON.json](../assets/validation/raw/M5_A5_FLEXPART_COMPARISON.json) | 完整 host 与 workload contract、executable identity、全部 run record、median、scaling、scientific summary 与 check status |
| [M5_A5_METEOROLOGY_EQUIVALENCE.json](../assets/validation/raw/M5_A5_METEOROLOGY_EQUIVALENCE.json) | Source 与 staged file identity、array shape、common-field classification、GRIB packing check 与 alignment result |
| [M5_A5_TIMINGS.csv](../assets/validation/raw/M5_A5_TIMINGS.csv) | 每次 warm-up 或 formal timing sample 一行，包含 wall time、RSS、CPU time、filesystem counter、mode 与 order |
| [M5_A5_SCIENCE.csv](../assets/validation/raw/M5_A5_SCIENCE.csv) | 每个 population、repetition 和共同 output time 一行，包含五个 cross-model ensemble measure |
| [PUBLICATION_MANIFEST.json](../assets/validation/PUBLICATION_MANIFEST.json) | Source aggregate hash 和每张 publication chart 的 SHA-256 |

`raw/original-charts` 目录保留 formal run 直接生成的图。下面的发布副本由 aggregate 重新生成，
scientific x-axis 使用记录中的 20、40 与 60 分钟 output instant。

## 读取 timing CSV

`M5_A5_TIMINGS.csv` 包含以下 column：

| Column | 含义 |
| --- | --- |
| `particles` | Requested population size |
| `phase` | `warmup` 或 `formal` |
| `repetition` | Warm-up 为零；formal sample 为一至三 |
| `order_index` | Mode 在该次 repetition 轮换顺序中的位置 |
| `mode` | `flexpart-core`、`flexpart-product` 或 `trajecta-product` |
| `core_seconds` | 该 mode 提供 transport-oriented boundary 时的时间 |
| `product_seconds` | 该 mode 提供 complete-product boundary 时的 wall time |
| `peak_rss_bytes` | Process 报告的 maximum resident set size |
| `user_seconds`、`system_seconds` | `/usr/bin/time -v` 记录的 CPU time |
| `filesystem_inputs`、`filesystem_outputs` | 同一工具报告的 filesystem counter |

Core 或 product 中的空 cell 表示该 mode 只提供另一个 timing boundary。重新计算发布 median
前，需要筛选 `phase == formal`。

## 读取 science CSV

`M5_A5_SCIENCE.csv` 的每次 repetition 含三个 output row。Unix time 与 elapsed time 对应为：

| `physical_time_unix` | 经过时间 |
| ---: | ---: |
| 1543634400 | 20 分钟 |
| 1543635600 | 40 分钟 |
| 1543636800 | 60 分钟 |

其他 column 按名称采用 metre 或 dimensionless ratio。定义与正负号 convention 见
[科学方法](science.md)。

## 发布图表

### Equivalent-core wall time

![10,000 与 50,000 粒子的 equivalent-core median wall time](../assets/validation/charts/core-wall-time.svg)

该图对比 transport-oriented median。10,000 粒子数值为 1.330 秒与 0.465 秒；50,000
粒子数值为 1.340 秒与 1.469 秒。

### Complete-product wall time

![10,000 与 50,000 粒子的 complete-product median wall time](../assets/validation/charts/complete-product-wall-time.svg)

该图沿用各程序的 selected product path。50,000 粒子下，particle NetCDF product 为
1.370 秒，Trajecta SQLite 与 provenance product 为 8.680 秒。

### Throughput scaling

![10,000 与 50,000 粒子的 equivalent-core throughput](../assets/validation/charts/throughput-scaling.svg)

Throughput 以粒子数除以 equivalent-core median seconds。图中同时呈现短 Case 的固定工作
和 population-dependent transport cost。

### Peak resident memory

![Core 与 product mode 的 median peak resident memory](../assets/validation/charts/peak-rss.svg)

Peak RSS 是三次 formal run 中 process maximum 的 median。Byte 以每 MiB 1,048,576 byte
换算。

### 科学集合对比

![20、40 与 60 分钟的水平和垂直集合差异](../assets/validation/charts/scientific-comparison.svg)

Scientific chart 显示两档粒子数的 horizontal centroid separation 与 signed vertical
centroid difference。Dispersion、transport-distance 与 occupancy 的详细值保留在 CSV 中。

## 重建图表

在仓库 `trajecta` 目录下，使用已经为 workspace tool 准备的 Python environment：

```text
python tools/validate_m5_1_validation_assets.py
```

命令执行四项检查：

1. 读取 frozen aggregate status 与 source artifact hash。
2. 在临时目录重新生成 publication chart。
3. 将每张生成图的 hash 与 `PUBLICATION_MANIFEST.json` 比较。
4. 对 English 与 Chinese validation asset tree 做 byte-for-byte comparison。

命令不会改变已经提交的 chart。计划更新图表时，可以从新的 frozen aggregate 或经过审阅的
rendering change 开始，随后重新生成，并在同一 commit 更新 publication manifest。

## 在其他分析中使用数据

CSV 可以由 R、Python、Julia、spreadsheet 或 command-line table tool 直接读取。重新计算
performance summary 时，可保留 `particles`、`phase`、`repetition` 与 `mode` 作为 grouping
column。Scientific row 按 particle count 与 physical time 分组；即使当前值相同，各次
repetition 仍是单独 observation。

引用表格或 derived plot 时，可以同时记录本页顶部的 aggregate SHA-256。该值定位当前
publication bundle 使用的准确 workload、executable、raw row 与 derived value。
