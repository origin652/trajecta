---
title: 时间覆盖、空间覆盖、job 与 attempt
description: 了解 Trajecta 中的轨迹时间、插值帧、domain support、dataset identity、job series、run ID 和 attempt。
---

# 覆盖与运行身份

Trajecta 研究中有两类连续性。气象覆盖让每次轨迹查询都位于已准备的时间与空间支持内；运行
身份则区分每次队列提交、rerun 和结果目录。Data-plan 与 DatasetLock 处理前一类关系，job
series、run 和 attempt identity 处理后一类关系。

## Case 中的物理时间

Case 记录 `start`、`end` 与 `direction`：

```yaml
time:
  start: { seconds_since_unix_epoch: 1543644000, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1543608000, nanosecond: 0 }
  direction: backward
```

`start` 是 population 初始化所在的一端。Integrator 按声明方向向 `end` 推进。Forward Case
的 end 较晚，backward Case 的 end 较早。

资料获取与 locking 会把两个时刻按实际时间排序：

```text
coverage start = min(Case start, Case end)
coverage end   = max(Case start, Case end)
```

因此，将端点对调的 forward 与 backward Case 请求相同物理时段。Direction 仍会影响轨迹
推进、boundary flux、event order 和 particle age。

## 时间插值需要相邻帧

气象场位于离散 valid time。两个 frame 之间的 query 使用包围该时刻的一对 frame 做时间插值。
因此，lock requirement 在物理 Case coverage 前后各加入一帧。

假设资料间隔为六小时，运行时段为 06:10 至 11:50 UTC：

```text
00:00   06:00   [06:10 ───────── 11:50]   12:00   18:00
          pair for early queries ───────────┘
```

06:00 与 12:00 包围 Case。额外的 before/after frame 规则可以将 00:00 和 18:00 作为 lock
coverage 的插值 buffer。确切选择取决于 dataset profile cadence 和实际 valid-time inventory。

端点正好落在某个 frame 上时，仍沿用 profile 声明的覆盖规则。文件数量适合通过 data-plan
计算，不宜只根据运行时长估算。

## Data-plan coverage

`project data-plan` 组合各个已进入索引的 Profile 与其 Case，输出以下 requirement：

- 物理 coverage start 与 end；
- 一帧 before buffer 和一帧 after buffer；
- Dataset ID 与 dataset-profile name；
- 所需 capability set；
- 项目相对 data root 与 lock path；
- 生成计划时使用的 project input SHA-256 identity。

项目内容不变时，计划也保持确定。Acquisition helper 将计划展开为 provider timestamp 和目标
路径。Case 或 Profile 变化后，project identity 随之变化，需要生成新计划。

## DatasetLock coverage

Finalize 检查实际文件中的 valid time。Lock builder 选择与 profile 匹配的文件，覆盖 Case 和
插值 buffer。Coverage gap 按以下情况返回：

| 缺口 | 含义 |
| --- | --- |
| Coverage 超出 available time | 物理 Case interval 位于 inventory 范围外 |
| Missing warmup frame | 所选区间之前缺少要求的 frame |
| Missing following frame | 所选区间之后缺少要求的 frame |
| No profile-matched file | 文件已经存在，但没有文件属于所选 interpretation profile |

DatasetLock 记录各个所选文件及其 valid time。提交时，Trajecta 根据该 lock 与 RunProfile 的
本地 data root 重建 inventory。文件字节、网格、垂直拓扑或 profile identity 发生变化后，
通过新的 finalize 建立绑定。

## 空间支持与 safe core

气象资料提供水平网格和垂直支持。Case meteorology domain 将逻辑 dataset 连接到 domain ID、
priority 和 `horizontal_halo_cells`。

Halo 为水平插值保留相邻 grid cell。应用该 reserve 后，trajectory-safe core 位于 source outer
boundary 内侧。全球周期网格在 longitude 上回绕，safe core 沿周期方向连续。有限网格的侧面
用于 continuous domain crossing 和 domain-fill exchange。

垂直支持位于本地 transport floor 与可用 model top 之间。Terrain 和 surface pressure 会使
下部有效边界随水平位置变化。Release placement、domain-fill initialization、气象 query、
reflection 和 model-top termination 都使用解析后的 support。

## 多个气象域

一份 Case 可以列出多个 domain。每项拥有唯一 ID、逻辑 dataset 和 priority。例如，高分辨率
nested domain 可以与周围较粗的资料同时使用，前提是两套资料已经按兼容科研设置准备。

在 query point 上，domain precedence 选择包含该点的最高优先级 valid safe core。Domain-fill
population 仍声明一个 domain，因为初始质量预算和侧向边界需要一套连贯网格。其他 domain
可以根据 Case 选择规则参与 trajectory meteorology。

每个逻辑 dataset 都有自己的 RunProfile binding 和 DatasetLock。Data planning 会对所选
Case/Profile pair 的 requirement 求并集。

## 四层 identity

配置、资料、执行和科研输出可以分别理解：

| 层次 | 示例 | 记录位置 |
| --- | --- | --- |
| Resolved configuration | Case SHA-256、RunProfile SHA-256、numerical model ID | Run manifest 与 resolved document |
| Input data | DatasetLock SHA-256、profile SHA-256、单文件 SHA-256 | DatasetLock、manifest 与 provenance |
| Execution | Job-series ID、run ID、attempt、resource、software version | Job catalog 与 run manifest |
| Output content | Exact SQLite SHA-256、canonical SQL digest、provenance content digest、canonical output digest | Manifest 与 verification result |

两个运行可以使用不同 run ID，同时拥有相同 resolved input 和 canonical output identity。反过来，
文件名相同而 SHA-256 更新时，它已经是不同输入，即使 Case 没有变化。

## Job-series ID

本地队列接收一项新的逻辑提交时，会创建 UUID-v7 job-series ID。`job status`、`job wait`、
`job events`、cancel、rerun 和 forget 通常都使用该 ID。

Series 将源于同一接收请求的 attempt 组织在一起。`job rerun` 之后它保持稳定，事件与 history
query 也继续连接到同一个 series。

## Run ID 与 attempt number

每个 attempt 获得新的 UUID-v7 run ID，以及从 1 开始的 attempt number。两者共同标识一个
worker execution 和一个结果目录。

```text
job series S
├── attempt 1, run R1: interrupted
├── attempt 2, run R2: cancelled
└── attempt 3, run R3: complete
```

Attempt number 表达 series 内顺序。Run ID 在全局范围内区分运行，并进入 manifest、SQLite、
provenance、particle-state join 与 result lookup。

`job rerun` 保留 series，创建下一个 run ID，并完整保留旧 attempt 目录。编辑 Project/Profile
pair 后重新提交会形成新的逻辑 series，因为请求输入已经发生变化。

## 稳定 particle identity

Particle ID 根据 population identity 和确定性 birth coordinate 生成，例如 event ordinal 或
domain-fill stratum ordinal。它不包含 run ID。Resolved scientific input 相同的任务可以在不同
worker 数和 rerun 中使用相同 particle ID。

每条 trajectory record 还包含 job-series ID、run ID 与 attempt。因此，分析可以对齐两个运行
中的 particle `42`，同时保留各自 source attempt 身份。

## Restart 与 recovery identity

Local daemon 在 catalog 中记录 worker process identity 和 attempt state。Daemon 重启后会
根据操作系统进程身份协调活动记录：

- 已确认仍存活的 worker 继续属于原 attempt；
- 已消失的 worker 进入 `interrupted`；
- queued work 继续等待 dispatch；
- terminal work 保持终态。

Recovery 可以在需要时更新 lifecycle state，不会分配新 run ID，也不会重复 complete attempt。
只有显式 rerun 或新 submission 才会创建另一次执行。

## 比较两个结果

根据比较问题选择 identity：

- 对比 Case 与 Profile digest，确认 resolved setup 是否等价。
- 对比 DatasetLock 与 dataset-content digest，确认 input byte 和 interpretation 是否等价。
- 对齐 trajectory 时比较 stable particle ID。
- 判断规范化结果是否相同时比较 canonical output digest。
- 在分析表中保留 run ID 和 attempt，使运维历史仍可区分。

`result verify --full` 会重新计算 normalized output identity，并检查 lifecycle row。其结果记录
可以作为跨运行比较的起点。
