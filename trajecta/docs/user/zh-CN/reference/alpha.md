---
title: Alpha release 范围
description: 了解 Trajecta 0.1.0-alpha.1 的平台、本地 runtime、资料准备、结果接口、compatibility 与预留 extension point。
---

# Alpha release 范围

`0.1.0-alpha.1` 提供从项目准备到 verified trajectory result 的完整本地流程。Alpha label 为
首个 stable release 前的 public document 与 operational detail 调整保留空间。本页集中列出
当前范围。

## 已包含的工作流

当前 release 支持：

1. Incremental machine 与 project configuration。
2. Case 与 RunProfile validation 和 resolution。
3. Deterministic data planning、inspection、locking 与 project finalization。
4. 向 persistent local queue 进行 foreground 或 detached submission。
5. 带 durable event 的 multi-job CPU 与 memory admission。
6. Safe cancellation、force cancellation、rerun 与 restart recovery。
7. Result inspection、quick 或 full verification、trajectory reading 与 Markdown report
   generation。
8. Windows x64 与 Ubuntu 24.04 x86-64 packaged execution。

支持的 meteorological family 为 CFSR pressure level、ERA5 pressure level 与 ERA5 hybrid
model level。Package matrix 包含 regular release、dry-air-mass domain filling 与
stratospheric-ozone domain filling。

## Local control plane

Daemon、worker process、job catalog 与 IPC endpoint 位于同一台机器。Detached mode 支持该
机器上的后台任务，并能在 submitting terminal 关闭后继续运行。当前 command surface 不含
remote host、network queue submission 和 distributed multi-node scheduling。

一份 machine configuration 选择一个 local endpoint 与 catalog。多份配置可以描述分开的
local queue，只需让 endpoint 和 catalog path 各自独立。

## 资料准备

Project 可以在 meteorological file 到达前完成配置。`project data-plan` 报告 missing 或 partial
requirement，data 就绪后再显式运行 `project finalize`。

Optional download helper 是一项独立 Python tool：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan DATA_PLAN
```

Default mode 预览 provider request 与 target path，`--execute` 才执行下载。Provider credential
从 official configuration 或 environment location 读取，不写入 Project、DatasetLock、log 或
provenance。Helper 也不会创建 DatasetLock，仍由 explicit finalization 完成。

## 结果产品

Primary result 是包含 manifest、SQLite particle history、provenance bundle、resolved input 与
optional Markdown report 的 run directory。Product command 提供 inspection、verification 与
trajectory streaming。Version 1 SQLite schema 可用于 advanced read-only analysis。

当前没有 general export command。后续 exporter 计划写入用户选择的 immutable run product
外部目录，使 current result identity 不受 derived file 影响。

## Queue retention

Daemon 重启后，complete 与其他 terminal attempt 继续保持终态。Rerun 由用户显式创建，并
生成新 attempt。`job forget` 改变日常列表可见性，同时保留 catalog 与 artifact。

`job prune` 返回 `delete_enabled: false` 的 deterministic dry-run plan。`0.1.0-alpha.1` 没有
应用该 plan 或删除 result directory 的命令。

## Extensibility

Plugin loading、plugin manifest 与 stable plugin API 尚未实现。预留的
[Extensibility](../concepts/extensibility.md)页面说明后续设计需要保持的边界。现有 Rust trait
属于 workspace 内的 contributor interface，不构成 loadable plugin mechanism。

## Compatibility

| Surface | Alpha compatibility rule |
| --- | --- |
| Software version | M5.1 保持 `0.1.0-alpha.1`；普通 commit 不创建另一个 version |
| Configuration 与 machine stream | 携带各自 `schema_version`；format change 使用新的 schema identity |
| Case 与 RunProfile | 携带 current numeric document schema，并拒绝 unknown field |
| SQLite | Public `user_version = 1` schema；external analysis 使用 read-only |
| CLI | Public command tree 从 binary 生成，并与 frozen contract 校验 |
| Rust crate | Contributor-facing，可以在 alpha series 中演进 |

Future release 引入另一个 identifier 时，alpha schema change 可能带有 documented migration。
Resolved document 与 result artifact 保持原 identity，不会因安装较新 binary 而被静默改写。

## Validation 范围

Clean-package product validation 覆盖两个发布平台，以及[平台参考](platforms.md)中的 matrix。
Scientific 与 performance measurement 使用 [Validation](../validation/index.md)所述冻结一小时
ERA5 对比。使用其他 duration、domain、dataset resolution 或 physical module 的研究，可以
加入符合自身 workflow 的 validation case。
