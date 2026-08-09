---
title: 存储、SQLite 与 WAL
description: 规划 Trajecta 存储，理解 SQLite 预写式日志，读取活跃结果，并处理磁盘空间或文件系统异常。
---

# 存储、SQLite 与 WAL

Trajecta 将本机任务数据库、气象资料和运行结果写入不同路径。它们可以位于工作站的同一文件系统，
但增长速度和恢复方式不同。

## 存储区域

| 区域 | 典型内容 | 增长方式 |
| --- | --- | --- |
| 配置目录 | 本机 TOML 配置和端点选择 | 体积小，改动较少 |
| 任务数据库目录 | SQLite 主库、WAL、守护进程标识、事件和执行轮次元数据 | 随提交次数和事件历史增长 |
| 资料目录 | 资料锁覆盖的 GRIB 或 NetCDF 文件 | 随资料准备范围增长 |
| 结果根目录 | 每个执行轮次一个独立目录 | 随粒子状态、溯源信息和保留的中断运行增长 |

`project show`、`project status` 和 `project data-plan` 可以查看项目相对路径。`config list` 显示
选择任务数据库和资源策略的本机设置。提交大型队列前，应分别检查这些路径所在文件系统的空间。

## 结果目录

一个完成结果通常包含：

```text
RESULT/
  run-manifest.json
  resolved-case.json
  resolved-run-profile.json
  particles.sqlite
  particles.sqlite-wal
  provenance-bundle.json
  run-report.md          # 执行 `run report --result RESULT` 后存在
```

不同终态还可能保留工作进程日志、临时文件或中断诊断文件。以下命令会返回当前目录中实际存在的
产物：

```text
trajecta --format json result inspect RESULT
```

运行清单会随生命周期更新。因此，活跃或中断目录可能已经有运行清单，但内容和终态
`complete` 目录不同。

## WAL 表示什么

SQLite 的预写式日志会先把新提交的数据库页写入伴随文件，之后再通过检查点合并到主库：

```text
particles.sqlite
particles.sqlite-wal
particles.sqlite-shm
```

本机任务数据库也使用 WAL。活跃数据库旁的 WAL 是数据库状态的一部分，读取器可能依赖它读取
已提交记录，写入端也可能仍在使用。

正常结果收尾时，Trajecta 会对粒子数据库执行检查点，并截断 WAL。成功关闭的终态结果因此拥有
零字节 WAL，或不再存在 WAL。`running` 或 `interrupted` 目录中的非空 WAL 对应另一种生命周期，
应与主库一起保留。

!!! warning "SQLite 伴随文件要成组保留"

    不要单独删除或移动 `*-wal`。完成收尾和验证前，将主库、`-wal` 与 `-shm` 视为一个整体。

## 并发读取

Trajecta 结果命令打开只读快照，并遵守当前已经建立索引的高水位范围，避免把写入端尚未完整发布
的行当作完整轨迹：

```text
trajecta result inspect JOB_ID
trajecta result trajectory JOB_ID --particle-id 100
```

活跃快照只表示命令执行时刻的状态。再次运行命令可以看到后续行。需要稳定导出或完整科学分析时，
等待任务进入终态并先运行完整验证。

其他程序直接打开 `particles.sqlite` 时：

- 使用只读模式；
- 保持 WAL 和共享内存伴随文件位于主库旁；
- 不执行结构修改、检查点、`VACUUM` 或写事务；
- 使用 [SQLite 参考](../reference/results-sqlite.md)中的索引顺序；
- 移动或归档目录前关闭连接。

## 估算结果增长

粒子状态存储主要由每个输出事件仍然活跃的粒子数决定：

```text
particle_state 行数 ≈ 所有输出事件中活动粒子数之和
```

实际字节还包括索引、SQLite 页面、运行元数据和溯源信息。粒子较早流出区域时，后期状态行会
减少。输出间隔越短，计划快照越多；即使模拟时长和粒子数不变，结果目录也会明显增大。

可以先用相同输出计划运行一个有代表性的小案例，再为正式粒子数按比例估算，并额外预留：

- 溯源信息包；
- 活跃写入期间的 WAL；
- SQLite 索引和页开销；
- 失败或中断后保留的执行轮次；
- 运行报告及诊断文件。

调度器内存预算不包含磁盘空间。

## 文件系统空间耗尽

磁盘写满后，SQLite 增长、运行清单替换、溯源收尾和任务数据库都可能中断。先减少新的写入：

1. 暂停提交新任务。
2. 查看 `job list`，确定活跃结果目录。
3. 文件系统仍能容纳收尾写入时，请求安全取消。
4. 从活跃结果目录以外的位置释放空间。
5. 空间恢复后运行 `doctor --deep`。
6. 检查每个受影响执行轮次及其事件。

若空间不足以完成安全收尾，工作进程可能进入 `failed` 或 `interrupted`。保留整个执行轮次目录。
之后重跑会使用新目录，不会继续写旧 SQLite。

## 检查 SQLite 状态

深度环境检查会在当前环境中完成临时创建、WAL 写入、完整性检查、检查点、截断和清理：

```text
trajecta --project PROJECT doctor --deep
```

这项检查用于确认新任务能否使用当前 SQLite 运行环境和文件系统。已有结果使用：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
```

运行清单可读但 SQLite 不可用时，`result inspect` 可以返回部分信息。完整验证检查终态数据库、
清单行数、生命周期和规范化输出摘要。

完整性或数量检查失败后，保留原结果目录。修复存储问题后创建新执行轮次，使损坏运行与新运行
拥有各自的数据库和运行 ID。

## 移动或归档结果

等待执行轮次进入终态，并关闭所有直接 SQLite 读取连接。然后运行：

```text
trajecta result verify RESULT --full
trajecta run report --result RESULT
```

复制完整目录，包括零字节伴随文件和保留的中断文件。在目标位置再次验证结果，再按本地存储流程
处理源目录。

`job prune` 可以估算被替代执行轮次占用的空间：

```text
trajecta --format json job prune
```

`0.1.0-alpha.1` 返回 `delete_enabled: false` 的 `dry_run` 计划，不会删除任务数据库行或文件。
