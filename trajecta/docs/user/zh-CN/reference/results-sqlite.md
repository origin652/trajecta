---
title: 结果与 SQLite
description: 结果目录合同、推荐产品命令和高级只读 SQLite schema 指南。
---

# 结果与 SQLite

结果目录是不可变 attempt 产物。直接打开 SQLite 前，应先使用产品命令读取。

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

`run-report.md` 是幂等派生视图，不参与科学内容和 canonical-output 摘要。未来导出产品
必须写入显式指定的外部目录；当前版本没有 export 命令。

## 主要文件

| 路径 | 作用 |
| --- | --- |
| `run-manifest.json` | 运行身份、状态、输入、执行、质量、计数和摘要 |
| `particles.sqlite` | 粒子、样本、质量、事件和终止表 |
| `provenance-bundle.json` | 样本对应的源记录与 field-set 归属 |
| `resolved-case.json` | 该 attempt 实际接收的 Case |
| `resolved-run-profile.json` | 该 attempt 实际接收的执行 Profile |
| `run-report.md` | 可选的人类可读派生报告 |
| `forensic/` | 存在中断或失败时保留的证据 |

## SQLite schema version 1

公开 SQL 定义位于
[`testdata/M4_SQLITE_SCHEMA.v1.sql`](https://github.com/origin652/trajecta/blob/main/trajecta/testdata/M4_SQLITE_SCHEMA.v1.sql)。

| 表 | 内容 |
| --- | --- |
| `run` | 一条运行身份与终态 |
| `particle` | 稳定粒子身份与出生元数据 |
| `particle_mass` | 每个粒子的物质质量 |
| `output_event` | 按顺序记录的物理输出事件 |
| `particle_state` | 位置、时间、状态、所选气象量、质量和 provenance 指针 |
| `termination` | 每个终止粒子的一条分类原因 |

数据库应以只读方式打开。不要写表、执行可变 migration，也不要从其他进程 checkpoint
活动 attempt。成功终态结果需要满足 `PRAGMA integrity_check = ok`，终态 WAL 为空。
无法保证只读访问的自定义分析，应先复制完整结果目录。
