---
title: Trajecta 操作指南
description: 说明渐进配置、延后准备资料、finalize、队列、事件、取消、重跑、dry-run prune、结果读取和 AI 辅助。
---

# 操作指南

以下流程假定已经安装 `0.1.0-alpha.1`，并选定本机配置。

## 渐进配置

使用 selector 逐项读取和修改：

```text
trajecta config list
trajecta config get resources.memory_pool_mib
trajecta config set resources.memory_reserve_mib 1024
trajecta config set resources.memory_pool_mib 8192
trajecta config unset profile_templates.experimental
trajecta config validate
```

请保持单一配置真源。`set` 或 `unset` 失败时，有效旧文件不会被部分内容替换。

## 先配置项目，后准备资料

气象文件尚未准备时，可以执行 `project init`、`project set`、`case validate` 和
`project validate`。项目保持 `configured` 或 `draft`，缺少 lock 会形成 diagnostic。

```text
trajecta --project PROJECT project status
trajecta --project PROJECT project validate
trajecta --project PROJECT project data-plan --output data-plan.json
```

该状态可用于审阅和规划 provider request，尚不满足任务接收条件。

## 使用 data-plan 与 finalize

网络访问前先审阅 data-plan：

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json
```

CFSR 下载只使用 Python 标准库。执行 ERA5 pressure 或 hybrid 请求前，请安装冻结的资料
准备依赖：

```text
python -m pip install -r requirements-data.txt
```

批准请求后增加 `--execute`。助手只写入声明的数据根，不会创建或替换 DatasetLock。

```text
python tools/fetch_trajecta_data.py --project PROJECT --plan data-plan.json --execute
trajecta --project PROJECT project finalize
```

Finalize 会展开所选文档，检查真实资料、能力和覆盖范围，再原子写入 lock。Case 或
Profile 变化后，应重新生成 `project data-plan`。

## 前台和后台运行

命令默认前台等待：

```text
trajecta --project PROJECT run --profile PROFILE
```

以下命令提交到本地队列，并在任务被接收后返回：

```text
trajecta --project PROJECT run --profile PROFILE --detach
trajecta job wait JOB_ID
```

Daemon 根据 CPU 和内存池接收任务。关闭提交终端后，后台任务继续运行。前台客户端断开
也不会撤销已接收任务。

## 消费任务事件

从头读取整个队列：

```text
trajecta --format jsonl job events --since 0 --follow
```

只读取一个任务：

```text
trajecta --format jsonl job events JOB_ID --since SEQUENCE --follow
```

处理事件后保存最后一个 sequence。重连时通过 `--since` 续读，以减少遗漏或重复处理。
JSONL 依次输出 stream header、数据项，并在流结束时输出唯一 summary。

## 取消、重跑、forget 与 prune

```text
trajecta job cancel JOB_ID
trajecta job cancel JOB_ID --force
trajecta job rerun JOB_ID
trajecta job forget JOB_ID
trajecta job prune
```

安全取消请求 worker 在安全点停止。Worker 无法到达安全点时，才考虑 force cancel。
Rerun 创建新 attempt 并保留旧 attempt。Daemon 恢复时不会重跑已经完成的任务。

当前版本的 `job prune` **只执行 dry-run**。它返回确定性计划，不删除结果、任务行、
forensic 资料和 attempt。请在事故处理之外审阅该计划。

## 读取结果

优先使用产品命令：

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta result trajectory RESULT --particle-id 42
trajecta run report --result RESULT
```

SQLite 结构用于高级只读分析。不要原地修改数据库、manifest、provenance bundle 或结果
目录。

## AI-assisted configuration companion

向 AI 提供人类审阅所使用的同一组 Case、Profile、project index、data-plan 和公开 schema。
请让 AI 提议 `config set` 或 `project set` 命令，避免创建第二份配置。不得提供 CDS 凭据
或与任务无关的私有路径。

接收 AI 辅助设置前执行：

```text
trajecta config validate
trajecta --project PROJECT project validate
trajecta --project PROJECT doctor --deep
```

Diagnostics 和 resolved documents 是最终验收证据。
