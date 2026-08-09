---
title: Trajecta — 拉格朗日水汽追踪
description: 面向拉格朗日水汽追踪、domain filling 及正反向大气轨迹计算的开源框架。
---

# 使用 Trajecta 开展拉格朗日水汽追踪

Trajecta 在网格化气象场中追踪随气流运动的空气质点。每个积分步都会在粒子当前位置
读取风场、气压、温度和湿度等变量。轨迹积分器据此推进粒子，随后处理地表、模式顶和
水平边界，并在指定时刻写出粒子状态。时间可以从源区向后发展，也可以从受体时刻向前
回溯。

同一套执行核心支持两类常见研究。释放试验在指定地点和时间创建粒子，可用于研究源区
排放的输送路径，也可用于受体轨迹。Domain fill 试验使用一组粒子表示气象区域中的
空气质量，并跟踪水汽或臭氧状态。水汽源汇分析通常从 domain fill 工作流开始。

Trajecta 通过命令行项目组织研究。Case 保存可移植的科学设置，RunProfile 提供本机
资料路径、输出位置、读取器、线程数和内存预算。运行前，`project finalize` 会检查
气象文件并生成 DatasetLock。Worker 只接收已经解析且固定的输入，每次执行写入独立的
结果目录。

## 可以运行哪些研究

| 研究类型 | Trajecta 创建的 population | 常见问题 | 可用资料路径 |
|---|---|---|---|
| Domain-fill 水汽追踪 | 分布在全球或有限区域内的气团粒子 | 某区域内的水汽来自哪里，又从哪里离开？ | CFSR pressure 或 ERA5 pressure |
| 定时释放 | 由一个或多个时空源事件创建的粒子 | 源区释放的物质会输送到哪里？ | CFSR pressure 或 ERA5 pressure |
| 气团追踪 | 对有限区域干空气质量进行采样的粒子 | 气团如何进入区域、在区域内运动并最终离开？ | ERA5 pressure |
| 平流层臭氧追踪 | 使用 PV60 臭氧规则初始化的 domain-fill 气团 | 选定时段内的平流层臭氧如何输送？ | ERA5 137 层 hybrid 资料 |

`0.1.0-alpha.1` 中的四条工作流使用同一个球面 RK2 轨迹积分器。Case 指定积分方向、
时间步长、边界策略、population、随机种子和输出计划。气象 profile 负责解释源资料，
并把其中的变量转换为积分器使用的标准物理量。

## 从 Case 到结果

```text
Case + RunProfile + 项目索引
                 │
                 ▼
             data-plan
                 │  准备或下载所需时次
                 ▼
          project finalize
                 │  检查文件并创建 DatasetLock
                 ▼
           本地队列与 worker
                 │  加载气象场，积分粒子，写出结果
                 ▼
              结果目录
```

项目可以在资料到齐前建立。`project data-plan` 会列出每个 Case/Profile 组合所需的
物理时间范围、dataset profile、能力集合、lock 路径和资料目录。独立的 Python 资料
助手可以先预览请求，确认后再下载。下载不会自动改变可运行输入；DatasetLock 只由显式
执行的 finalize 创建。

`run` 默认在前台等待。加入 `--detach` 后，命令会在本地 daemon 接收任务后返回。
Daemon 保存队列状态，并按 CPU 和内存资源池启动 worker。机器重启后，已经完成的
attempt 仍保持完成状态。`job rerun` 会在同一 job series 下创建新的 attempt。

## 一次运行会产生什么

每次执行都会在 RunProfile 的 `output_root` 下获得独立目录。`particles.sqlite` 保存
输出事件、粒子信息、按时间排列的粒子状态、随粒子携带的质量以及终止记录。
`run-manifest.json` 汇总本次执行使用的输入、数值设置、记录数量、质量统计和终态。
目录中还会保存 resolved Case、resolved RunProfile 与 `provenance-bundle.json`。

常规读取从以下命令开始：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

`result inspect` 可直接查看粒子数、输出时次、读取器、终止原因和主要文件位置。
`result trajectory` 按时间顺序输出指定粒子的记录。需要批量分析时，可以按照只读方式
访问 SQLite；相关表结构在参考手册中单独说明。

## 从哪里开始

| 当前目标 | 建议阅读 |
|---|---|
| 安装软件并了解安装包目录 | [入门](getting-started/index.md) |
| 使用配套 CFSR 资料完成一次小型 domain-fill 运行 | [十五分钟快速入门](getting-started/quickstart.md) |
| 学习水汽、释放、气团或臭氧完整流程 | [教程](tutorials/index.md) |
| 修改配置、准备资料或管理队列 | [操作指南](how-to/index.md) |
| 理解 Case、Profile、population、覆盖范围和结果目录 | [概念](concepts/index.md) |
| 处理关机、内存压力或磁盘故障后的任务 | [运维](operations/index.md) |
| 查询命令、字段、诊断码和 SQLite 表 | [参考](reference/index.md) |
| 构建 workspace 或修改实现 | [开发手册](developer/index.md) |

`0.1.0-alpha.1` 提供 Windows x86_64 和 Ubuntu 24.04 x86_64 安装包。远程调度、
插件加载、实际删除式 prune 和通用结果导出命令仍未发布。当前范围见
[Alpha 限制](reference/alpha.md)。
