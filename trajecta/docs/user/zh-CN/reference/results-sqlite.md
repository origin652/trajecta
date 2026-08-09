---
title: 结果与 SQLite 参考
description: 查阅 Trajecta 结果目录、结果读取命令、SQLite 第 1 版表结构、查询顺序、WAL 生命周期和内容摘要关系。
---

# 结果与 SQLite

队列接受一次执行后，会为它创建独立结果目录。运行清单、数据库、解析后文档和溯源信息只属于
一个运行 ID 与执行轮次。重跑创建新目录，并保留早期目录。

日常查看和导出轨迹时，先使用结果读取命令：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

`RESULT` 可以是任务系列 ID、具体运行 ID 或结果目录路径。通过 ID 查询时使用 `--config` 选择的
任务数据库；直接路径不需要任务数据库。

## 目录布局

| 路径 | 生命周期 | 内容 |
| --- | --- | --- |
| `run-manifest.json` | 运行开始时创建，收尾时替换 | 运行 ID、状态、输入、资源、数量、质量、SQLite 与溯源散列及摘要 |
| `particles.sqlite` | 运行期间持续写入 | 粒子、质量、输出事件、状态、气象样本和终止 |
| `particles.sqlite-wal` | 活跃写入伴随文件；安全终态后为零或不存在 | 尚待检查点合并的已提交数据库页 |
| `particles.sqlite-shm` | 需要时存在 | SQLite WAL 协调状态 |
| `resolved-case.json` | 数值执行前写入 | 工作进程实际采用的规范化科学案例 |
| `resolved-run-profile.json` | 数值执行前写入 | 工作进程实际采用的本机执行与资料绑定 |
| `provenance-bundle.json` | 输出收尾时完成 | 输出行使用的源记录、字段集和样本归属 |
| `run-report.md` | 按命令创建 | 由其他结果产物生成的人类可读视图 |
| `provenance-bundle.json.forensic-aborted*` | 某些溯源收尾中断后存在 | 保留的部分溯源流或构建状态 |

失败或中断时还可能出现临时文件和工作进程诊断文件。`result inspect` 返回实际排序后的产物清单，
不会假设所有终态目录都具有成功运行的布局。

## 结果检查

```text
trajecta --format json result inspect RESULT
```

检查命令严格解析公开运行清单。运行清单可读而 SQLite 不可用时，返回部分响应及
`result.inspect_sqlite_unavailable`，来自 SQL 的字段为 `null`。成功终态清单缺少 SQLite 或
溯源信息文件时，返回 `result.artifact_missing`。

命令会将清单中的 SQLite 行数与实际数据库比较。两者不一致时返回
`result.manifest_sqlite_count_mismatch`，不会把两组数量合并显示。

## 快速验证与完整验证

```text
trajecta result verify RESULT
trajecta result verify RESULT --full
```

快速模式检查文件散列和基础结构关系，耗时较短。完整模式进一步检查生命周期覆盖、有限值和质量
分类，再核对质量账本闭合、SQLite 数量与完整性、溯源语义及规范输出摘要。

通过任务数据库定位的 `complete` 执行轮次完成完整验证后，守护进程会记录已验证运行 ID 与规范
摘要。该记录可以使同一任务系列中较早的终态执行轮次进入只读清理候选。

## 轨迹读取

选择一个或多个粒子 ID：

```text
trajecta result trajectory RESULT --particle-id 42
trajecta result trajectory RESULT --particle-id 42 --particle-id 105
```

CLI 会排序 ID 并拒绝重复值。写出轨迹前会先检查全部请求 ID；任何粒子缺失时，只返回一条诊断，
不会生成部分数据流。`--all` 选择全部粒子，不能与 `--particle-id` 同时使用：

```text
trajecta --format jsonl result trajectory RESULT --all
```

| 输出模式 | 形状 |
| --- | --- |
| 人类可读 | 按确定字段顺序显示的行 |
| JSON | 一个完整响应，含流头与记录；SQLite 行先写入新建临时缓冲文件，全部校验完成后再开始标准输出 |
| JSONL | 流头、每条轨迹数据一项、最后一条汇总 |

记录按粒子 ID 和样本序号排序。读取大粒子群时，JSONL 可避免构造一个大型 JSON 文档，并让下游
逐行处理。

## 运行报告

```text
trajecta run report --result RESULT
```

输出固定为 `RESULT/run-report.md`。命令在同目录创建名称唯一的临时文件，刷新并同步后原子重命名。
结果产物未改变时，重复运行会生成相同报告字节。

报告不会把自身加入产物摘要，也不会改变科学内容、SQLite 规范 SQL 摘要、溯源内容或规范化输出摘要。

## SQLite 第 1 版

公开定义为
[`testdata/M4_SQLITE_SCHEMA.v1.sql`](https://github.com/origin652/trajecta/blob/main/trajecta/testdata/M4_SQLITE_SCHEMA.v1.sql)。
数据库设置 `PRAGMA user_version = 1`，页大小 32 KiB，启用外键、严格表和 WAL，关闭自动检查点。
终态检查点由 Trajecta 写入端负责。

### `run`

一行标识当前数据库运行。主要列为 `run_id`、`manifest_schema`、`case_name`、`status`，以及
纳秒分辨率的起止时间。`running` 行的结束字段为空，所有终态行都具有完整结束秒与纳秒。

### `particle`

每个稳定粒子 ID 一行，记录 `population_id`、来源、出生时刻、干空气质量和可选敏感度权重。
`origin_kind` 可为 `release`、`domain_initial` 或 `domain_boundary`，配套事件、区域和边界面列
随来源类型使用。

### `particle_mass`

组合键 `(run_id, particle_id, substance_id)` 保存每种跟踪物质的非负千克质量。

### `output_event`

事件按 `event_sequence` 排序，保存物理秒、纳秒和事件类型。类型可为 `birth`、`start`、
`interval`、`end` 或 `termination`。多个事件物理时刻相同时，时间索引再按事件序号排序。

### `particle_state`

主键为 `(run_id, particle_id, sample_sequence)`。每行包含：

- 输出事件和物理时刻；
- 积分偏移与粒子存活时间；
- 经度、纬度和海拔高度；
- `alive` 或 `terminated` 状态，以及可选终止原因；
- 选定的风、气压和气温；
- 气象字段的有效性与质量标签；
- 指向溯源信息中对应记录的可选 ID。

公开时间索引为：

```sql
CREATE INDEX particle_state_by_time
ON particle_state (run_id, physical_seconds, physical_nanosecond, particle_id);
```

### `termination`

每个已终止粒子一行，记录原因、正常或异常分类、物理时刻，以及取值 0–1 的可选边界相交分数。

## 只读查询示例

用 SQLite 只读模式打开数据库。执行轮次仍活跃时，让 `particles.sqlite-wal` 和
`particles.sqlite-shm` 保持在主库旁。

读取一个粒子的轨迹：

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1 AND particle_id = ?2
ORDER BY sample_sequence;
```

读取一个物理输出快照：

```sql
SELECT *
FROM particle_state
WHERE run_id = ?1
  AND physical_seconds = ?2
  AND physical_nanosecond = ?3
ORDER BY particle_id;
```

统计终止原因：

```sql
SELECT classification, reason, COUNT(*) AS particles
FROM termination
WHERE run_id = ?1
GROUP BY classification, reason
ORDER BY classification, reason;
```

活跃写入期间，应在一个读事务内使用该快照可见的索引高水位。第二个进程不要执行结构变更、写
事务、`VACUUM` 或检查点。Trajecta 自带的结果读取命令已经按这些规则实现快照读取。

## WAL 与终态收尾

写入端活跃时，非空 WAL 属于正常状态。安全完成和安全取消会对数据库执行检查点，并截断终态
WAL。工作进程被强制停止或失联时，执行轮次目录可能保留非空 WAL 和 `running` 清单。

主库与伴随文件应保持在一起。只移动已经停止写入的目录，到达目标位置后运行完整验证。
[存储指南](../operations/storage-sqlite.md)说明磁盘写满和 WAL 收尾中断时的处理。

## 内容摘要关系

运行清单分别记录规范化溯源内容、SQLite SQL 内容和规范化输出摘要。路径规范化摘要允许比较位于
不同执行轮次根目录中内容相同的科学结果。可选运行报告不参与这些摘要，因此刷新
`run-report.md` 不会改变结果摘要。

派生图件、统计表或其他分析文件应写入用户选择的外部目录，使原始运行结果保持完整。
