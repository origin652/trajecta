---
title: Trajecta 如何开展验证
description: 阅读 Trajecta 科学、确定性、产品、性能和 clean-package 验证结果，并重建发布图表。
---

# 验证总览

Trajecta 的验证从气象输入一直跟随到最终结果目录。检查内容包括积分器接收的物理变量、
粒子生命周期与质量核算、重复运行的一致性、SQLite 与 provenance product，以及 clean
release package 中的实际运行。

本章还发布一项与 FLEXPART 的受控对比。对比使用共同的短时间平流 Case，在已经声明的
共同边界上计算科学集合指标与运行时间。图表旁提供原始 JSON 和 CSV，数值可以直接读取，
也可以用于重新绘图。

## 验证层级

| 层级 | 固定内容 | 检查内容 |
| --- | --- | --- |
| 输入身份 | Source file、content hash、dataset family 与 resolved coverage | Run 是否读取预期的不可变气象内容 |
| 气象解释 | Variable name、unit、grid coordinate、time axis 与 vertical coordinate | 所需变量是否为有限值，并能按 dataset profile 完成查询和物理解释 |
| 数值生命周期 | 初始 population、output schedule、termination class 与 mass rule | 每个事件是否覆盖全部粒子，mass ledger 是否闭合 |
| 确定性 | Resolved input、worker setting 与 digest definition | 重复运行和已声明的 worker 对比是否保持规定 normalized identity |
| 结果产品 | Manifest schema / SQLite row / WAL closeout / provenance / report / CLI reader | 完成目录能否作为一个 run product 读取并完整验证 |
| 性能 | Host / CPU affinity / executable hash / workload / timing boundary / warm-up / repetition | 是否从同类 measurement 计算 median wall time、throughput、scaling 和 peak resident memory |
| 发布包 | Package manifest、bundled native library、clean extraction 与 platform | Public command 能否脱离 source checkout 运行真实项目 |

这些层级放在同一条验证链中。计时样本还要到达预期科学状态与产品状态，产品检查也要带有
输入身份和 run identity，从而找到它所包含的科学计算。

## 验证范围

Trajecta 使用几种各有侧重的验证范围。

### Unit 与 contract test

Crate test 覆盖 document parsing、interpolation 与 population update。其他测试处理 boundary、
output encoding、job scheduling、local IPC 和 result reader。Schema validator 使公开 JSON shape 和 example
与实现保持同步。这类测试运行频繁，出现 regression 时可以定位到较小行为。

### 真实资料 integration run

Integration case 会打开实际 CFSR 或 ERA5 文件，并驱动 production reader、数值路径、
output sink 和 verifier。它们可以覆盖 synthetic array 中没有的 encoded coordinate、
missing value、vertical transform、file boundary 与 reader selection。

### Product matrix

Product matrix 从 packaged CLI 开始，依次运行项目准备、daemon submission、result inspect、
verification、trajectory reading 和 report generation。Case 覆盖已支持的 dataset family、
population mode、方向与 reader 选择。Clean-package run 位于 Windows x86-64 和 Ubuntu
24.04 x86-64。

### 科学与性能对比

当前发布对比固定一个 ERA5 hybrid-level Case、两档粒子数、一小时输送和四线程 CPU
allocation。三个物理输出时刻用于计算共同集合指标，运行时间则按两个边界分别报告。
具体内容见[科学方法](science.md)、[性能方法](performance.md)与[可比较性矩阵](comparability.md)。

## 如何阅读结果

每个 formal aggregate 都有 top-level status 和一组 named check。`passed` 表示列出的检查在
该 aggregate 固定合同下成功完成。Particle count 与 simulation duration 描述运行规模和时间。
Output cadence 与 dataset 描述输入输出安排。Reader、host 和 executable identity 则定位执行环境。

发布对比提供三种形式：

| 形式 | 适合的用途 |
| --- | --- |
| 图表 | 快速查看 wall time、throughput、memory 与集合差异 |
| CSV | 重新绘图、表格分析和检查各次 repetition |
| JSON aggregate | 查看完整 contract、executable identity、measurement order、derived median、check 与 source artifact path |

[冻结数据页](data.md)提供三者的链接和本地重建命令。

## 验证自己的结果

Formal publication matrix 描述当前 release。其他机器生成的结果可以通过自己的产品验证流程
检查：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta run report --result RESULT
```

`result inspect` 汇总 manifest、artifact inventory 和可取得的 SQLite count。Full verification
检查 lifecycle coverage、finite value、mass accounting、SQLite consistency、provenance 和
canonical output identity。生成的报告会将 run identity、input、resource、quality summary
与 artifact list 写入 `RESULT/run-report.md`。

研究使用新的 dataset、duration、domain 或 physical process 时，可以另外准备符合该用途的
validation case。当前发布的一小时对比可作为所含 advection workflow 的参考点。更长积分和
附加过程仍可沿用同一组织方式：固定输入，声明 metric，重复运行，并保留原始 measurement。

## 发布材料

- [科学验证方法](science.md)
- [性能测量方法](performance.md)
- [FLEXPART 可比较性矩阵](comparability.md)
- [冻结 JSON、CSV 与图表](data.md)
