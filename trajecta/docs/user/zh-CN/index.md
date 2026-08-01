---
title: Trajecta — 拉格朗日水汽追踪
description: 面向拉格朗日水汽追踪、domain filling 及正反向大气轨迹计算的开源框架。
---

# 使用 Trajecta 开展拉格朗日水汽追踪

Trajecta 是一个开源的拉格朗日水汽追踪与大气轨迹框架。它通过命令行工作流支持
domain filling、水汽追踪、常规释放试验，以及正向和反向大气轨迹计算。

本框架适用于需要保存数值输入、配置身份、任务生命周期和输出来源的科研运行。
每次运行生成不可变结果目录，其中包括 manifest、粒子状态 SQLite 数据库和按内容寻址的
provenance bundle。

## 选择入口

| 目标 | 建议入口 |
|---|---|
| 在约 15 分钟内获得已验证结果 | [Domain-fill 快速入门](getting-started/quickstart.md) |
| 完整学习一条科学工作流 | [教程](tutorials/index.md) |
| 配置项目或管理任务队列 | [操作指南](how-to/index.md) |
| 理解 population、覆盖范围与身份 | [概念](concepts/index.md) |
| 处理异常中断和资源压力 | [运维](operations/index.md) |
| 查看科学与性能证据 | [验证](validation/index.md) |
| 查询命令和 schema | [参考](reference/index.md) |
| 构建或参与开发 | [开发手册](developer/index.md) |

## 产品边界

`0.1.0-alpha.1` 提供 Windows x86_64 和 Ubuntu 24.04 x86_64 正式安装包。
Alpha 阶段的产品接口包括 CLI、配置文档、公开 schema 和磁盘产物。Rust crate API
属于贡献者内部接口，在稳定版发布前可能调整。

!!! note "Alpha 软件"
    请独立保存源资料和结果产物。规划正式研究前，应先阅读
    [Alpha 限制](reference/alpha.md)。
