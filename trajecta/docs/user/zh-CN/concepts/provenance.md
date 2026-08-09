---
title: Provenance 与结果目录
description: 了解 Trajecta 的 run manifest、SQLite 输出与 resolved input，以及 field provenance、digest 和 interrupted artifact。
---

# Provenance 与结果目录

每个已接收的 attempt 都有独立结果目录。数值输出与输入身份、field lineage 一起保存在目录
中，用于说明该结果的生成过程。项目后续修改不会作用于已经保存的 attempt。

## 目录布局

完成的正式运行通常包含：

```text
attempt-1/
├── run-manifest.json
├── resolved-case.json
├── resolved-run-profile.json
├── particles.sqlite
├── particles.sqlite-wal
├── provenance-bundle.json
└── run-report.md                 # 运行 `run report` 后出现
```

Interrupted attempt 还可能包含 worker log、部分数据库状态，或名称带有 `forensic-aborted` 的
文件。Manifest status 和 artifact list 可以用于判断它到达了哪条终态路径。

## Run manifest

`run-manifest.json` 是目录索引。它以 `running` 状态创建，并在生命周期信息变化时原子替换。
Terminal manifest 包含以下部分：

| 区域 | 内容 |
| --- | --- |
| Identity | Job series、run ID 与 attempt；Case name；开始和结束时间 |
| Software | Package 与 crate version，以及可用时的 source revision |
| Inputs | Case 与 RunProfile；DatasetLock 与 dataset profile；dataset-content hash |
| Execution | Worker 数与内存预算；executor 与 reader；wall time、peak RSS 和可提供的 I/O counter |
| Numerical | Random seed 与 integrator；boundary policy 与 population；sink、tolerance registry 和 deterministic flag |
| Geometry | Resolved release source identity 与 canonical geometry summary |
| Outputs | Case 选择的 effective product 与 schedule |
| SQLite | Schema version、journal setting、table row count 与相对路径 |
| Provenance | Bundle path、exact 与 normalized digest，以及 record count |
| Lifecycle | Status、termination summary、mass ledger 与可选 failure |

Manifest 声明的路径都相对于 attempt 目录。产品命令在打开前会检查路径范围。

## Resolved input document

`resolved-case.json` 保存展开本地组件引用后的规范化 scientific Case。
`resolved-run-profile.json` 保存 worker 实际使用的 execution 与 dataset binding。

项目发生变化后，可以通过这两份文件回答当时的运行问题：

- Attempt 使用了哪个 direction 与 time step？
- 当时选择的 population、seed 和 output schedule 是什么？
- 逻辑 dataset 映射到哪个 lock 与 reader？
- Attempt 请求了多少 worker 和内存？

Manifest 保存两份文档的 SHA-256 identity。归档或共享结果时，将 resolved copy 与目录一并
保留。

## Particle-state SQLite 数据库

`particles.sqlite` 是主要可查询结果。公开 schema 包含六张表：

| 表 | 作用 |
| --- | --- |
| `run` | 一条 run identity 和 terminal status |
| `particle` | 稳定 particle identity 与 population；origin 与 birth；carrier mass 和 sensitivity weight |
| `particle_mass` | 与粒子关联的 substance mass |
| `output_event` | 按物理顺序排列的 output time 和 event kind |
| `particle_state` | 各 sample 的粒子位置与所选气象量；quality 与 provenance pointer |
| `termination` | 每个已终止粒子的一条 classified terminal reason |

Compound key 包含 `run_id`，复制后的行仍可归入一个确切 attempt。产品流中的 particle state
按稳定 particle ID 与 per-particle sample sequence 排序。

Worker 活动期间，SQLite 使用 write-ahead logging。正常 terminal finalization 会 checkpoint 并
truncate WAL。运行中 reader 可以使用 indexed high-water snapshot；归档和 full verification
则在终态后进行。

## Field-level provenance bundle

`provenance-bundle.json` 将 `particle_state` 中所选气象值连接到 source record。它覆盖五个已
存储字段：

- Eastward wind；
- Northward wind；
- Geometric vertical velocity；
- Air pressure；
- Air temperature。

每项不同的 field lineage 对应一条 content-addressed record：

| 部分 | 含义 |
| --- | --- |
| Field token | Canonical field name |
| Quality | Source、derived、estimated 或其他声明 quality |
| Sources | 按科研顺序排列的 locked source identity |
| Transforms | Profile/query path 依次执行的 operation ID 与 parameter |
| Fallback reason | 选择 fallback field path 时的可选说明 |
| Profile SHA-256 | 确切 interpretation-profile identity |

Bundle 根据 canonical JSON SHA-256 复用重复 record。第二个 dictionary 将五个可选 field slot
组合为 content-addressed field set。最后，每个 `(particle_id, sample_sequence)` 指向一个
field-set SHA-256。这样无需在每个 particle sample 中重复完整 source 与 transform chain。

Sample assignment order 与 SQLite particle-state order 一致。某个已存储气象值缺失时，对应
field-set slot 为 null；SQLite validity 与 quality field 说明该 sample 状态。

## Exact-file 与 normalized digest

几类 hash 用于不同的比较：

| Digest | 哪些变化会更新它 |
| --- | --- |
| SQLite SHA-256 | Finalized database file 的任意字节变化 |
| Canonical SQL digest | 规范化公开 table content 或 order 变化 |
| Provenance-bundle SHA-256 | Final JSON bundle 的任意字节变化 |
| Provenance content digest | Normalized record、field set 或 sample assignment 变化 |
| Canonical output digest | Canonical SQL content 或 normalized provenance content 变化 |

Manifest 将这些身份连接起来。Provenance block 包含 SQLite exact hash、canonical SQL hash、
bundle hash、normalized provenance hash 和 canonical output hash。`result verify` 会从文件
重新计算这些值。

Exact-file hash 适合传输检查。Normalized digest 适合比较容器字节因存储原因不同、内容仍
等价的输出。

## Lifecycle 与 provenance 完成顺序

Formal provenance bundle 在 SQLite output 关闭后 finalize，因为它包含最终 SQLite SHA-256。
`complete`、`completed_with_particle_errors` 和安全 `cancelled` 都会得到 terminal database
与 formal bundle。

Output finalization 中途停止时，已部分构建的 bundle 会移动到唯一的 `forensic-aborted` 名称，
不会占用 formal `provenance-bundle.json` 路径。`result inspect` 会列出这些文件，便于查看中断
后保留下来的内容。

## 派生 run report

`trajecta run report --result RESULT` 根据 `result inspect` 创建 `run-report.md`。报告在一份
Markdown 中先整理 identity、lifecycle 和 input，再汇总 resource、particle count 与 quality。
Mass accounting、verification state 和 artifact path 位于后续章节。

报告通过原子方式重新生成，并排除在 canonical output digest 之外。Full verification 或
forget 操作可能更新 catalog view，此后可以刷新报告；数值结果身份保持不变。

## 日常分析中的读取顺序

可以从简到繁读取：

1. 用 `result inspect` 查看 lifecycle、count、identity 与 artifact path。
2. 文件传输后用 `result verify` 检查 file 与 digest。
3. 用 `result verify --full` 检查 row、lifecycle、quality、termination 与 mass。
4. 用 `result trajectory` 读取 particle metadata 与 ordered state record。
5. 专门分析再使用 read-only SQLite 和 provenance-bundle query。

Trajectory stream 在各条 state 上带有 `provenance_id`。公开 provenance bundle 使用相同
particle ID 与 sample sequence，把 state 连接到五字段 field set。

## 复制与保留结果

Attempt 进入终态后，将整个目录作为一个单元复制。Manifest、resolved document 与 SQLite
database 一起保留。存在时的 WAL entry，以及 provenance bundle、report 与 forensic file
也放入同一份副本。目标
位置先运行 quick verification，长期归档再运行 full verification。

分析输出适合放在用户选择的相邻目录。Attempt 目录由此保持稳定，图件、表格和未来的导出
产品也可以使用各自的命名与保留策略。
