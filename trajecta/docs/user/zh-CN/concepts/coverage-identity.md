---
title: 时间覆盖、空间覆盖、任务和 attempt
description: 说明气象覆盖、插值锚点、job-series 身份、run 身份和保留的 attempt。
---

# 覆盖与执行身份

## 时间与空间

Case 记录开始、结束、方向和逻辑气象域。Data-plan 将每个 Case/Profile 组合转换为有序
物理时间覆盖和所需能力。DatasetLock 记录真实文件能够证明的覆盖范围。

空间覆盖来自气象网格检查。Case 中的逻辑域选择资料集及优先级，不授权任意本机路径。
插值 halo 和相邻时间锚点可能使资料需求超出名义轨迹区间。

## Job series、run 与 attempt

| 身份 | 含义 |
|---|---|
| Job series | 跨重跑保持稳定的用户请求身份 |
| Run ID | 一次输入不可变的已接收执行 |
| Attempt | Series 下按顺序保留的执行历史 |

`job rerun` 创建新 attempt，不覆盖旧结果。Daemon 恢复会协调已接收和运行中的任务，
已经完成的终态 attempt 不会再次提交。内容身份、源码身份和生命周期状态可以帮助运行人员
区分恢复动作与新的科研运行。
