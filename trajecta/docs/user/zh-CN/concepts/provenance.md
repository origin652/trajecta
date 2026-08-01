---
title: Provenance 与结果目录
description: 说明 manifest、SQLite 输出、resolved input、provenance bundle、报告和 forensic 产物之间的关系。
---

# Provenance 与结果目录

任务进入终态后，结果目录构成不可变证据边界。主要产物如下：

| 产物 | 用途 |
|---|---|
| `run-manifest.json` | 生命周期、身份、计数、质量、输入清单和输出引用 |
| `particles.sqlite` | 粒子状态及输出事件记录 |
| `provenance-bundle.json` | 按内容寻址的输入、配置、程序和输出身份 |
| `resolved-case.json` | Worker 实际执行的科学意图 |
| `resolved-run-profile.json` | 任务接收时的本机执行绑定 |
| `run-report.md` | 确定性人类可读派生视图 |
| Forensic entries | 存在中断或强制取消时保留的证据 |

Manifest 指向 SQLite 和 provenance 身份。Full verify 会重新计算内容摘要与语义摘要，
不会只检查文件是否存在。报告不参与科学摘要，因此重复生成保持幂等。

Worker 进入终态且 WAL 处理完成后，才可以复制结果目录。复制时应将目录作为整体保留。
日常分析先使用 `result inspect`、`result verify` 和 `result trajectory`，高级分析再直接
读取 SQLite。
