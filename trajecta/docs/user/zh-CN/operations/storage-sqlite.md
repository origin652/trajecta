---
title: 存储、SQLite 与 WAL
description: 规划 Trajecta 存储，理解 SQLite write-ahead log，读取活跃输出，并处理磁盘空间不足或文件系统异常。
---

# 存储、SQLite 与 WAL

Trajecta 将本机任务目录、气象资料和 run product 写入各自路径。工作站上可以让它们共用一个
文件系统，但三者的增长方式和恢复需求有所区别。

## 存储区域

| 区域 | 常见内容 | 增长方式 |
| --- | --- | --- |
| 配置目录 | 机器 TOML 文件和本机 endpoint 选择 | 体积小，改动较少 |
| 任务目录 | SQLite catalog、WAL、daemon ownership、事件和 attempt metadata | 随提交的 attempt 与事件历史增长 |
| 资料根 | DatasetLock 覆盖的 GRIB 或 NetCDF 文件 | 随资料准备过程增长 |
| 输出根 | 每个 run attempt 一个目录 | 随粒子状态行、provenance 和保留的中断任务增长 |

`project show`、`project status` 和 `project data-plan` 会显示项目相对路径。`config list` 显示
选择任务目录与资源策略的机器设置。提交大型队列前，应检查所有相关文件系统的可用空间。

## 结果目录结构

完整结果通常包括：

```text
RESULT/
  run-manifest.json
  particles.sqlite
  provenance/
  run-report.md          # 执行 `run report --result RESULT` 后生成
```

根据 attempt 状态，目录中还可能有 resolved document、worker log、临时文件或 forensic
文件。`result inspect` 会返回该 run 的实际 artifact inventory：

```text
trajecta --format json result inspect RESULT
```

Manifest 会跟随生命周期转换写入。因此，活跃或 interrupted attempt 可以已经存在 manifest，
其状态仍与 `complete` 终态不同。

## WAL 表示什么

SQLite write-ahead logging 会先将近期提交的 page 写入 sidecar，随后才 checkpoint 到主库：

```text
particles.sqlite
particles.sqlite-wal
particles.sqlite-shm
```

任务 catalog 也使用 SQLite WAL。活跃数据库旁的 WAL 属于数据库状态的一部分。Reader
可能需要通过它读取已经提交的行，writer 也可能仍在使用它。

正常结果收尾时，Trajecta 会 checkpoint 粒子数据库并 truncate WAL。因此，安全关闭的成功
终态具有零字节 WAL，或不再存在 WAL。Running 与 interrupted 目录中的非空 WAL 对应另一种
生命周期状态，应与主库放在一起。

!!! warning "SQLite sidecar 要成组保留"

    不要单独删除或移动 `*-wal`。结果完成收尾与验证前，将主库、`-wal` 和 `-shm` 视为一组
    文件。

## 并发读取

Trajecta 结果命令打开只读快照，并遵守当前 indexed high-water 边界，从而避开 writer 尚未
完整发布的 trajectory record。

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 100
```

活跃快照只代表读取时刻。再次执行命令可以看到后续行。准备稳定导出或完整科学分析时，
可等待任务进入终态，再运行 full verification。

其他程序直接打开 `particles.sqlite` 时，可采用以下方式：

- 使用 read-only mode；
- 保持 WAL 与 shared-memory sidecar 位于数据库旁；
- 不执行 schema change、checkpoint、vacuum 或 write transaction；
- 遵守 [SQLite 参考](../reference/results-sqlite.md)中的 indexed ordering；
- 移动或归档结果目录前关闭连接。

## 估算输出增长

粒子状态存储主要由每个 output event 仍然活跃的粒子数量决定。可以先采用以下估算：

```text
particle rows ≈ 所有 output event 的 active particle 数量之和
```

每行实际字节还包括 index、SQLite page、metadata 和 provenance。Population 较早流出计算域
时，后续行数会减少。模拟时长与粒子数保持不变时，更短的 output interval 会增加 scheduled
snapshot 数量。

可以先使用相同输出 schedule 运行一个有代表性的小案例，再为更大 population、provenance
bundle、活跃写入期间的 WAL 和保留的失败 attempt 留出余量。Scheduler memory budget 不会
预留磁盘空间。

## 处理文件系统空间耗尽

文件系统写满后，SQLite 增长、manifest 替换、provenance 收尾或任务 catalog 都可能中断。
首先减少新的写入：

1. 暂停提交新任务。
2. 查看 `job list`，确认活跃结果目录。
3. 文件系统仍能容纳收尾写入时，请求安全取消。
4. 在活跃结果目录之外释放空间。
5. 空间恢复后运行 `doctor --deep`。
6. 逐个检查受影响 attempt 及其事件记录。

若空间已经不足以完成安全收尾，worker 可能进入 failed 或 interrupted。保留它的 attempt
目录。之后的 rerun 会使用新目录，不会继续写旧 SQLite。

## 检查 SQLite 状态

Deep doctor 会在所选环境中执行临时 create、WAL write、integrity check、checkpoint、
truncate 和 cleanup：

```text
trajecta --project PROJECT doctor --deep
```

它检查当前 SQLite runtime 和文件系统能否用于新任务。已有 run 使用结果命令：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
```

Manifest 可读而 SQLite 不可用时，`result inspect` 可以返回 partial response。Full
verification 会检查终态数据库、manifest count、lifecycle 和 canonical output identity。

Integrity 或 count 检查失败时，可保留原结果目录，并在存储问题修复后创建新 attempt。这样
可以继续诊断旧 attempt，也避免两个 run identity 共用一个数据库。

## 移动或归档结果

等待任务进入终态，并关闭所有直接 reader。随后运行：

```text
trajecta result verify RESULT --full
trajecta run report --result RESULT
```

复制完整目录，包括零字节 sidecar 和已保留的 forensic 文件。目标位置再次验证通过后，再
按照所在环境的存储流程处理源目录。

`job prune` 可以估算 superseded attempt 占用的空间：

```text
trajecta --format json job prune
```

在 `0.1.0-alpha.1` 中，响应是 `delete_enabled: false` 的 dry-run plan，不会移除 catalog row
或文件。
