---
title: 检查和读取 Trajecta 结果
description: 定位一次运行结果，检查生命周期与 artifact，执行 quick 或 full verify，流式读取轨迹并生成报告。
---

# 检查和读取结果

一个结果目录对应一个确切 attempt。目录中组合了 run manifest、粒子状态 SQLite 数据库、
provenance bundle、解析后的源文档，以及中断或失败时形成的 forensic 文件。进行高级 SQLite
分析前，可以先通过产品命令读取这些内容的稳定视图。

## 选择结果

命令接受以下几种 `RESULT`：

| 形式 | 示例 | 解析方式 |
| --- | --- | --- |
| Job series ID | `0198...` | 所选本机 catalog 中该 series 的当前 attempt |
| Run ID | `0198...` | 所选本机 catalog 中的确切 attempt |
| 目录路径 | `PROJECT/runs/.../attempt-1` | 确切磁盘目录 |

ID 查询使用本机配置所选的 catalog。运行目录复制到另一台机器后，直接使用目录路径更方便：

```text
trajecta --config configs/workstation.toml result inspect JOB_ID
trajecta result inspect archived-runs/study/attempt-2
```

## 检查生命周期、计数和 artifact

先读取 human 输出：

```text
trajecta result inspect RESULT
```

JSON 模式提供完整的 `trajecta.result-inspection/v1` 产品：

```text
trajecta --format json result inspect RESULT
```

Inspection 按以下区域组织信息：

| 区域 | 内容 |
| --- | --- |
| `identity` | Job series、run ID、attempt 序号与 Case 名称 |
| `lifecycle` | 状态与 run-success 标记；开始和结束时间；失败详情 |
| `software` / `inputs` / `numerical` | Manifest 中的软件身份、源资料身份与数值设置 |
| `execution` | 资源设置和运行时计数 |
| `particles` | SQLite 中的粒子、状态和事件计数，以及质量与终止计数 |
| `quality` | 风、气压和气温的 validity/quality 分组计数 |
| `mass_ledger` | Ledger 条目数与观察到的最大不平衡量 |
| `artifacts` | Manifest 与 SQLite；WAL 与 provenance；报告和 forensic 路径及实际大小 |
| `catalog` | Catalog 可用时的列表可见性、full-verification 记录和 supersession |

对于 `complete`、`completed_with_particle_errors` 和 `cancelled`，inspection 会查找 manifest
声明的 terminal SQLite 和 provenance 产品。Interrupted 或 failed attempt 的 manifest 可读
时，可以返回部分 inspection。SQLite 文件存在但无法读取时，JSON envelope 保留 manifest
字段，加入一条结构化 warning，并将 SQLite 派生区域设为 `null`。

## 区分生命周期成功与结果可读性

`lifecycle.status` 表示运行状态：

| 状态 | 含义 |
| --- | --- |
| `complete` | 运行结束，异常粒子终止计数为零 |
| `completed_with_particle_errors` | 运行完成收尾，存在一个或多个异常粒子终止 |
| `cancelled` | 安全取消形成了完成收尾的部分结果 |
| `failed` | 受控 run-level 错误形成终态 manifest |
| `interrupted` | Worker 丢失或强制停止，留下 forensic 输出 |
| `running` | 目录仍由活动 attempt 使用 |

Inspection 可以描述科研运行未成功的终态目录。下游流程只接收成功运行时，可以读取机器输出
中的 `lifecycle.run_success`。

## 执行 quick verify

Quick verify 检查不可变 artifact 的身份和结构：

```text
trajecta result verify RESULT
```

命令解析并校验 `run-manifest.json`，确认状态适合 terminal verification，检查 terminal WAL
和公开 SQLite 行数，重新计算 exact-file 与 canonical SQL digest，校验 provenance bundle，
最后重新计算 canonical output digest。

响应是 `trajecta.result-verification/v1` 记录，包含 manifest、SQLite、SQL、provenance、
normalized provenance 和 canonical output 的 SHA-256。文件传输或归档时，可以把这份紧凑
记录随日志保存。

## 执行 full verify

Full 模式增加逐行的科研与生命周期检查：

```text
trajecta result verify RESULT --full
```

检查内容包括粒子与 sample 顺序、事件覆盖，以及坐标和状态值的有限性。气象质量声明、终止
一致性和 mass-ledger 容差也在该阶段检查。响应的 `full` 对象分别列出 particle、sample、
termination、output-event 和 mass-ledger 数量。

通过 catalog ID 解析的 `complete` attempt 成功完成 full verify 后，Trajecta 将 canonical
output digest 记录在本机 catalog。这条记录用于 attempt supersession 和 dry-run prune 决策。

Verification 用于已经结束的目录。`running`、`failed` 或 `interrupted` 结果仍可用于 inspect
和 forensic 分析，但不会形成成功 verification 记录。

## 读取指定粒子的轨迹

可以请求一个或多个稳定 particle ID：

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

CLI parser 会排序 particle ID，并拒绝重复值。输出流开始前，命令先检查每个请求的 ID；任一
ID 缺失时返回 diagnostic，不会输出一半轨迹。

读取所有粒子时使用：

```text
trajecta result trajectory RESULT --all
```

大范围选择按 particle 和 sample 顺序流式输出。JSONL 适合直接连接数据管道：

```text
trajecta --format jsonl result trajectory RESULT --all
```

首个 data item 是 `trajecta.trajectory-stream/v1` header，说明 run identity、状态和选择范围。
随后，每个粒子先输出一条 `record_kind: "particle"`，再输出按顺序排列的
`record_kind: "state"`。最后一项是 stream summary。

Particle record 包含 origin、birth time、dry-air mass、sensitivity weight 和 substance mass。
State record 包含物理时间、位置、event 与 sample sequence，以及粒子状态和所选气象量。
质量标记、provenance ID 和粒子在该 sample 终止时的终止信息也在同一 record 中。

JSON 模式使用临时 spool，从而输出一个有效 CLI envelope，同时避免把所有轨迹 record 留在
内存中。下游 reader 需要边到达边处理时，JSONL 更合适。

## 生成 Markdown 运行报告

在解析后的运行目录内写入标准报告：

```text
trajecta run report --result RESULT
```

路径固定为 `RESULT/run-report.md`。命令检查结果，先整理 identity、lifecycle 与 inputs，
随后写入 execution、particles、quality 和 mass 摘要。Verification、artifact 与 forensic
pointer 位于报告后半部分。文件通过原子替换写入；输入未变化时，重复运行会生成相同字节。

报告是供人阅读的派生视图，不进入 canonical scientific output digest。因此重新生成报告不会
改变结果身份。

## 移动或归档结果

复制完整 attempt 目录；目录中存在零字节 WAL 时也一并保留。传输完成后，针对目录路径执行
两级校验：

```text
trajecta result verify archived-runs/attempt-1
trajecta result verify archived-runs/attempt-1 --full
```

复制后的目录未注册到当前所选本机 catalog 时，catalog 字段为 `null`，manifest 和 artifact
身份仍可读取。

## 为高级分析打开 SQLite

[SQLite 参考](../reference/results-sqlite.md)说明公开表和 join。Attempt 进入终态后，通过
只读连接打开 `particles.sqlite`。单粒子历史优先使用 `result trajectory`；需要跨粒子聚合
或专门查询时，再直接读取 SQL。

派生表、NetCDF、图件和其他导出内容适合写入独立 analysis 目录。当前版本尚无 export 命令，
运行目录保留为不可变的源产品。
