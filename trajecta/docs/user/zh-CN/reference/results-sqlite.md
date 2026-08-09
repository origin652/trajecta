---
title: 结果与 SQLite 参考
description: 读取 Trajecta 结果目录、product command、SQLite version 1 table、query ordering、WAL lifecycle 与 digest 关系。
---

# 结果与 SQLite

每个 accepted attempt 都有单独结果目录。其中的 manifest、database、resolved document 与
provenance 属于一个 run ID 和 attempt number。Rerun 会创建另一个目录，并保留先前 attempt。

通常先使用 product command：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

`RESULT` 接受 job-series ID、准确 run ID 或结果目录路径。ID resolution 使用 `--config`
选择的 job catalog；direct path 不需要 catalog lookup。

## 目录结构

| Path | Lifecycle | 内容 |
| --- | --- | --- |
| `run-manifest.json` | 以 running 创建，并在 closeout 时替换 | Run identity、state、input、resource、count、quality、SQLite identity、provenance identity 与 digest |
| `particles.sqlite` | 整个 run 期间写入 | Particle、mass、output event、state、meteorological sample 与 termination |
| `particles.sqlite-wal` | Active-write sidecar；safe terminal closeout 后为零或不存在 | 等待 checkpoint 的 committed page |
| `particles.sqlite-shm` | 存在时为 SQLite shared-memory sidecar | WAL coordination state |
| `resolved-case.json` | Numerical execution 前写入 | Worker 接收的准确 normalized scientific Case |
| `resolved-run-profile.json` | Numerical execution 前写入 | Worker 接收的准确 local execution 与 dataset binding |
| `provenance-bundle.json` | Output closeout 期间 finalize | Output row 使用的 source-record、field-set 与 sample attribution |
| `run-report.md` | 按请求创建 | 其他 result artifact 的 idempotent human-readable view |
| `provenance-bundle.json.forensic-aborted*` | 指定 interrupted provenance closeout 后出现 | 保留的 partial provenance stream 或 bundle state |

Failed 或 interrupted attempt 中还可能出现 temporary file 与 worker diagnostic file。
`result inspect` 返回实际 sorted artifact inventory，不会假定每个 terminal directory 都具有
successful layout。

## Inspection

```text
trajecta --format json result inspect RESULT
```

Inspection 严格解析 public manifest。Manifest 可读而 SQLite unavailable 时，命令返回
manifest 和 artifact inventory，同时给出 `result.inspect_sqlite_unavailable` partial response。
该 response 中的 SQL-derived field 为 null。Successful terminal manifest 缺少 SQLite 或
provenance 时，命令返回 `result.artifact_missing`。

Manifest SQLite row count 会与实际打开的 database 比较。出现差异时返回
`result.manifest_sqlite_count_mismatch`，不会将两组 total 合并显示为一个结果。

## Quick 与 full verification

```text
trajecta result verify RESULT
trajecta result verify RESULT --full
```

Quick verification 检查 result identity 和低成本 structural relationship。Full verification
再加入 lifecycle coverage、finite-value 与 quality check、mass-ledger closure、SQLite count
与 integrity、provenance semantic 和 canonical output digest。

Catalog-backed `complete` attempt 通过 full verification 后，daemon 会记录 verified run ID 与
canonical digest。该 record 可以使同 series 中较早的 terminal attempt 进入 dry-run prune
plan candidate。

## Trajectory 读取

选择一个或多个 particle ID：

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

Parser 对 ID 排序，并拒绝 duplicate。写出 trajectory output 前会校验全部 requested ID，
因此缺失粒子只返回一条 diagnostic，不会产生 partial stream。`--all` 选择全部粒子，不能与
`--particle-id` 组合：

```text
trajecta --format jsonl result trajectory RESULT --all
```

| Output mode | Shape |
| --- | --- |
| Human | Deterministic field-oriented line |
| JSON | 一个包含 header 与 record 的 envelope；SQLite row 先写入 create-new temporary spool，验证完成后才开始 stdout |
| JSONL | Header item、每条 trajectory row 一个 data item、一个 final summary |

Record 按 particle ID 与 sample sequence 排序。读取大型 complete population 时，JSONL 可以
避免一个体积很大的 JSON document，并让 consumer 增量处理 row。

## Run report

```text
trajecta run report --result RESULT
```

输出路径固定为 `RESULT/run-report.md`。命令写入同目录 create-new temporary file，flush 并
sync 后 rename 到目标。Artifact 未改变时，再次运行会生成相同 report byte。

Report 在 artifact digest calculation 中排除自身，不改变 scientific content、SQLite SQL
identity、provenance content 或 canonical output digest。

## SQLite version 1

公开定义为
[`testdata/M4_SQLITE_SCHEMA.v1.sql`](https://github.com/origin652/trajecta/blob/main/trajecta/testdata/M4_SQLITE_SCHEMA.v1.sql)。
Database 声明 `PRAGMA user_version = 1`、32 KiB page、foreign key、strict table、WAL mode，
并关闭 automatic checkpoint。Terminal checkpoint 由 Trajecta writer 持有。

### `run`

一行记录 database run。主要 column 为 `run_id`、`manifest_schema`、`case_name`、`status`，
以及 nanosecond-resolution start 和 finish timestamp。Running row 的 finish field 为 null；
terminal row 则具有两个 finish component。

### `particle`

每个 stable particle ID 一行，记录 `population_id`、origin、birth time、dry-air mass 与
optional sensitivity weight。`origin_kind` 可以是 `release`、`domain_initial` 或
`domain_boundary`；event、domain 与 boundary-face column 会随该选择变化。

### `particle_mass`

Composite key `(run_id, particle_id, substance_id)` 保存每种 tracked substance 的
non-negative kilogram mass。

### `output_event`

Event 按 `event_sequence` 排序，并带有 physical seconds、nanoseconds，以及 `birth`、`start`、
`interval`、`end` 或 `termination`。物理时刻相同时，time index 还会按 event sequence 排序。

### `particle_state`

Primary key 为 `(run_id, particle_id, sample_sequence)`。每行包含：

- Output event 与 physical timestamp；
- Integration offset 与 elapsed particle age；
- Longitude、latitude 与 height above sea level；
- `alive` 或 `terminated` status，以及 optional termination reason；
- Selected wind、pressure 与 temperature value；
- Sampled field 的 validity 与 quality label；
- 指向 bundle attribution record 的 optional provenance ID。

Public time index 为：

```sql
CREATE INDEX particle_state_by_time
ON particle_state (run_id, physical_seconds, physical_nanosecond, particle_id);
```

### `termination`

每个 terminated particle 一行，记录 reason、normal 或 abnormal classification、physical time，
以及取值零至一的 optional boundary-intersection fraction。

## Read-only query pattern

以 SQLite read-only mode 打开 database。Attempt 活跃时，将 `particles.sqlite-wal` 与
`particles.sqlite-shm` 保持在主库旁。

读取一条 trajectory：

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1 AND particle_id = ?2
ORDER BY sample_sequence;
```

读取一个 physical output snapshot：

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1
  AND physical_seconds = ?2
  AND physical_nanosecond = ?3
ORDER BY particle_id;
```

读取 termination total：

```sql
SELECT classification, reason, COUNT(*) AS particles
FROM termination
WHERE run_id = ?1
GROUP BY classification, reason
ORDER BY classification, reason;
```

Active writing 期间，可以使用 read transaction 和该 snapshot 可见的 indexed high-water
state。第二个 process 不应执行 schema change、write transaction、`VACUUM` 或 checkpoint。
Product command 已经应用 supported snapshot behavior。

## WAL 与 terminal closeout

Writer 活跃时出现非空 WAL 属于正常状态。Safe completion 与 safe cancellation 会 checkpoint
database 并 truncate terminal WAL。Force-stopped 或 lost worker 可能在 attempt 目录留下非空
WAL 与 running manifest。

主库与 sidecar 应保持在一起。移动或归档 inactive complete directory 后，可以在目标位置
再次运行 full verification。[存储指南](../operations/storage-sqlite.md)介绍 disk exhaustion 与
interrupted WAL closeout 的恢复流程。

## Digest 关系

Manifest 分别记录 normalized provenance content、SQLite SQL content 与 canonical output
identity。Path-normalized digest 可以在不同 attempt root 之间比较相同 scientific product。
Optional report 被排除，因此刷新 `run-report.md` 不会改变这些 identity。

Future export product 计划写入用户选择的 run product 外部目录。`0.1.0-alpha.1` 提供本页所列
result command 与 read-only SQLite interface，当前没有 export command。
