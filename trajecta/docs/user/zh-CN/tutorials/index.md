---
title: Trajecta 教程
description: 通过完整的 domain-fill 水汽、定时 release、air-mass 和平流层臭氧项目学习 Trajecta。
---

# 教程

本组教程从科学 Case 出发，带领四个小型项目运行到可以读取的结果。它们沿用长期研究使用
的项目布局、队列、数值引擎和结果产品。每个教程只替换问题中的一个主要部分，例如粒子
population、气象资料家族、空间域或轨迹方向。

项目位于仓库和产品包的 `examples/` 目录。页面直接引入这些文件，文档中看到的配置就是
命令行程序实际解析的配置。

## 教程使用的案例

| 教程 | 科学 population | 气象资料 | 方向 | 资料准备方式 |
|---|---|---|---|---|
| [Domain-fill 水汽追踪](domain-fill.md) | 覆盖全球域的等干空气质量粒子 | CFSR 压力层 | 向前 | 四帧演示资产 |
| [定时 release](release.md) | 从一个点释放的 1,000 个粒子 | CFSR 压力层 | 向前 | 同一四帧资产 |
| [Air-mass 工作流](air-mass.md) | 覆盖有限域的等干空气质量粒子 | ERA5 压力层 | 向前 | 资料助手与 Climate Data Store 请求 |
| [平流层臭氧](ozone.md) | 在 PV60 臭氧范围内初始化的干空气载体 | ERA5 hybrid 模式层 | 向后 | 资料助手与 hybrid 准备流程 |

四个示例均使用 1,000 个粒子、300 秒积分步长、端点输出和 1 GiB Profile 内存预算。
物理时段为十分钟，可以在较短时间内完成资料检查和结果读取，同时保留 population 与
资料差异。

## 选择起点

Domain-fill 教程适合接在快速入门之后。该页解释气象快照如何转换为等质量粒子，以及
如何读取结果中的干空气与水汽量。

Release 教程从一个明确源项开始，适合研究问题已经给出地点、时刻、垂直位置和 tracer
质量的情况。页面还会介绍 release geometry 与事件时段。

Air-mass 教程转向官方 ERA5 压力层资料和有限域，展示 data-plan coverage 如何形成
provider request，以及粒子离开非周期边界后的状态。

Ozone 教程把反向 Case 与 ERA5 的 137 层 hybrid 坐标结合起来。内容包括 prepared-file
布局、对数地面气压、hybrid 系数和命名 PV60 规则的臭氧初始化。

## 共同的项目流程

每个项目采用相同顺序：

```text
project validate
      ↓
project data-plan
      ↓
准备气象文件
      ↓
project finalize
      ↓
doctor --deep
      ↓
run --profile product
      ↓
result verify / inspect / trajectory
```

`project validate` 检查当前已有的项目文档。Data-plan 列出物理时间覆盖、公开 dataset
profile、所需 capability、本地目标目录和 lock 路径。Finalization 检查准备好的文件，
并创建 RunProfile 选择的 DatasetLock。

Run 命令把 resolved project 提交到本地队列。端点输出会在十分钟区间两端各生成一次
particle-state event。随后可通过结果命令查看汇总、完成一致性检查，或按时间读取指定
粒子的记录。

## 阅读示例文件

每个教程项目都有相同的四部分：

| 路径 | 文件回答的问题 |
|---|---|
| `trajecta-project.yaml` | 哪些 Case 和 Profile 属于当前项目？ |
| `cases/*.yaml` | 要运行什么科学模拟？ |
| `profiles/*.yaml` | 本地资料与结果放在哪里，worker 使用多少资源？ |
| `locks/*.lock.json` | 哪些气象文件满足所选 Case 与 Profile？ |

Lock 在 finalization 后出现。结果写入 Profile 的 `output_root`，每个 attempt 都有独立
run identity。

教程页面中的片段来自上述文件。需要调整案例时，直接编辑项目目录中的示例文件，再运行
`project validate`。这样不会在 notebook 或 shell script 中形成第二份配置副本。

## 从教程扩展到研究

每次扩展一个维度更容易观察资源变化。延长时间会改变 data-plan 和 provider anchors；
增加粒子主要提高计算量与内存需求；缩短输出间隔会增加 SQLite 行数和结果目录体积；
更换地理域还会改变资料请求与边界流出的物理含义。

可以保留原示例作为小型 preflight，另建项目保存正式研究。在新机器或新 reader 上先运行
十分钟版本，再逐步增加时长、population 和输出密度。
[资料家族对照](data-families.md)可用于选择匹配的 profile 与时次间隔。
