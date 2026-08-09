---
title: 性能测量方法
description: 阅读冻结 Trajecta 与 FLEXPART 对比中的计时边界、repetition、throughput、scaling 和 peak memory。
---

# 性能测量方法

发布 benchmark 将数值输送和完整结果产品分开计时。Trajecta 默认产品包含可查询的粒子历史
数据库、lifecycle closeout、provenance 和 verification work。Transport timing 与 finished-
product timing 对应两个不同的运行问题。

## Benchmark 环境

| 项目 | 冻结值 |
| --- | --- |
| 操作系统 | WSL2 中的 Ubuntu 24.04.1 LTS |
| Architecture | x86-64 |
| Formal run CPU affinity | Logical CPU `0-3` |
| Worker 或 thread 数 | 4 |
| 气象资料 | 已对齐的 ERA5 hybrid-level Case |
| 模拟时长 | 3,600 秒 |
| Transport step | 600 秒 |
| Product output interval | 1,200 秒 |
| 粒子数 | 10,000 与 50,000 |
| Warm-up | 每档粒子数下，每种 mode 运行一次 |
| Formal sample | 每档粒子数下，每种 mode 运行三次 |

三次 formal repetition 采用轮换顺序。在 10,000 粒子下，三种 mode 会依次位于第一项；
50,000 粒子使用相同轮换。First-run、cache 和邻近进程造成的影响可以分散到各测量 mode。

## Executable identity

对比记录准确 executable byte：

| 程序 | Identity |
| --- | --- |
| Trajecta benchmark binary | SHA-256 `266a3c31dfb98e5206ee3e1c1d885d207b667a0ecb9f6e6caee438435a81b24f` |
| Trajecta source tree | SHA-256 `5fe6b857c1c59072b1eae3603fe6a51b77f01171481cdbe1e8313cba74e225c1` |
| FLEXPART | Version 11.1，commit `dace3affa2ba71677f12f3858b04aaf59f8ee51e` |
| FLEXPART binary | SHA-256 `5b6aacf1ac653c26dca2fd14e498d5e705bfdf7a38896a789869c3c11d24c629` |
| FLEXPART toolchain | GNU Fortran 13.3.0，ecCodes 2.34.1，netCDF-Fortran 4.5.4 |

Formal benchmark binary 带有内部 pre-release version `0.0.0`。随后 source 标记为
`0.1.0-alpha.1`；benchmark source identity 与该次版本更新之间没有数值或性能改动。本页
表格继续绑定上面的 hash。

## 计时边界

三种 mode 各自独立运行：

| Mode | 计时工作 | 生成产物 |
| --- | --- | --- |
| FLEXPART core | 以输送为中心的 executable path，将对比输出降到最少 | Core-run status 与 timing record |
| FLEXPART product | Transport 加 particle NetCDF product | 一个 particle NetCDF 与 run record |
| Trajecta product | Packaged product run 的正常结果生命周期 | Manifest、SQLite、provenance、verification input 与 run record |

### Equivalent core

Trajecta 将 production runner 记录为 named stage。Equivalent-core time 为：

```text
runner_total - runner_output
```

该边界保留 meteorological query、population work、integration 和 boundary handling，移除
测得的 output stage。它与 FLEXPART core run 对比。Formal ratio 为：

```text
Trajecta equivalent-core median / FLEXPART core median
```

冻结 acceptance limit 为 `1.5`，两档粒子数都低于该 ratio。

### Complete product

Complete-product wall time 沿用各程序的普通 product path。FLEXPART 写入 particle NetCDF。
Trajecta 写入 indexed SQLite particle history，并关闭 result command 使用的 manifest 与
provenance product。表格并排报告两侧数值，不套用 core ratio limit。

!!! tip "先确定计时边界"

    比较输送计算时使用 equivalent-core；估算用户等待时间时使用 complete-product。

## Sample 汇总方式

Warm-up run 用于在 formal measurement 前建立 code path 与 data path，不进入发布 median。
每种 formal mode 和粒子数采用以下处理：

1. Runner 可提供时按 nanosecond precision 收集 wall time，同时使用 `/usr/bin/time -v` 包裹
   process。
2. 保留 peak resident set size、user CPU time、system CPU time 与 filesystem counter。
3. 将三次 wall-time sample 排序。
4. 中间值作为发布 median。
5. Throughput 按 `particle count / median seconds` 计算。

Raw CSV 包含全部 warm-up 与 formal sample。例如，一次 50,000 粒子 FLEXPART core
repetition 为 2.87 秒，另外两次为 1.31 与 1.34 秒，因此发布 median 为 1.34 秒。

## Median wall time

| 粒子数 | FLEXPART core | Trajecta equivalent core | Core ratio | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 1.330 s | 0.465 s | 0.350 | 1.310 s | 2.195 s |
| 50,000 | 1.340 s | 1.469 s | 1.097 | 1.370 s | 8.680 s |

10,000 粒子时，Trajecta measured transport path 短于 FLEXPART core median。50,000 粒子
时，两侧 core path 接近，Trajecta 约为 FLEXPART median 的 1.10 倍。

Trajecta complete product 随粒子数增长得更明显，因为 output row 也随 population 增加。
50,000 粒子下，equivalent-core boundary 占 1.469 秒，完整产品为 8.680 秒。其余时间包含
SQLite 与 provenance output path，以及周围的 product closeout。

## Throughput

| 粒子数 | FLEXPART core | Trajecta equivalent core | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: | ---: |
| 10,000 | 7,519 particles/s | 21,486 particles/s | 7,634 particles/s | 4,556 particles/s |
| 50,000 | 37,313 particles/s | 34,029 particles/s | 36,496 particles/s | 5,760 particles/s |

Throughput 对这个固定一小时 workload 有参考意义。它并非每个 model step 或每个 output row
的速率。更长模拟会增加 transport step，更短 output interval 会增加 product row，两者对
计时边界的影响不同。

## 从 10,000 扩展到 50,000 粒子

| 边界 | 50k / 10k wall-time ratio |
| --- | ---: |
| FLEXPART core | 1.008 |
| FLEXPART product | 1.046 |
| Trajecta equivalent core | 3.157 |
| Trajecta product | 3.955 |

两档 FLEXPART 时间接近。这个短 Case 只有六个 step，固定 startup、meteorology preparation
和 step-level array work 占据测量区间的较大部分。在这台 host 上，将 particle array 从
10,000 增至 50,000 只给 median 增加少量时间。

Trajecta equivalent-core time 在粒子数增加五倍时增长 3.16 倍。Batching 与 in-memory
numerical path 使增长低于线性。Product time 增长 3.95 倍，因为 SQLite state row 与
provenance record 随 population 和 output event 增加。

这些 ratio 对应当前 step count 与 output cadence。模拟时间加长时，fixed startup 在总时间
中的比例会降低。输出变得频繁时，product writing 所占比例会提高。

## Peak resident memory

| 粒子数 | FLEXPART core | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: |
| 10,000 | 354.0 MiB | 354.6 MiB | 84.0 MiB |
| 50,000 | 361.8 MiB | 368.5 MiB | 163.1 MiB |

数值是 formal process peak RSS observation 的 median。Trajecta product path 在该 matrix
中使用较少 peak resident memory，同时表现出更清楚的 population-dependent 增长。
RunProfile memory budget 仍可依据计划使用的 dataset、domain、duration、reader 与
concurrency 设置；queue 使用该 declared budget 进行 admission。

## 将数值用于其他 workload

与其将一项表格数值直接乘到不同研究上，更实用的方法是做一次较小的本地 calibration。
保持目标 dataset 与 output cadence，运行两档粒子数，并记录：

```text
trajecta result inspect RESULT
trajecta run report --result RESULT
```

可以比较 runner time、output row、SQLite size、peak RSS 和 output event 数量。长时间研究还
可将 duration 与 particle count 分开改变，从而区分每个 transport step 和每条 stored
particle state 的成本。

[冻结数据与图表](data.md)页面提供全部 repetition，以及静态 wall-time、throughput 和
memory plot。
