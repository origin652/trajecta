---
title: 进程退出码
description: 解读 Trajecta 命令成功、用法错误、前后台运行、等待、结果检查和验证的命令行状态。
---

# 进程退出码

Trajecta 使用三个产品级退出码：

| 退出码 | Shell 中的含义 |
| ---: | --- |
| `0` | 命令完成了所请求的操作 |
| `1` | 命令已经解析，但遇到应用、校验、运行时错误，或运行进入非成功终态 |
| `2` | 命令语法、参数位置或参数值在解析阶段无效 |

操作系统加载错误、进程信号和强制终止可能在 CLI 返回产品状态前产生其他平台退出值。

## 命令成功与运行成功

机器输出分别表示命令操作和科学运行：

| 字段 | 含义 |
| --- | --- |
| `ok` | CLI 成功产生了该命令请求的响应 |
| `run_success` | 涉及运行生命周期时，所选执行轮次是否处于 `complete` |

例如，`result inspect` 可以成功读取一个 `interrupted` 结果。此时进程退出码为 `0`，`ok` 为
`true`，`run_success` 为 `false`。只接受完整科学运行的自动化流程应同时检查两项。

## 解析错误与应用错误

人类可读模式中，用法信息写入标准错误并退出 `2`；应用错误退出 `1`；帮助写入标准输出并退出
`0`。

JSON 或 JSONL 模式中，解析错误与应用错误通过标准输出返回结构化响应或诊断流项目。用法错误
仍保留退出码 `2`，解析成功后的应用错误返回 `1`。结构化应用响应不会再把同一诊断写到标准
错误。

## 提交运行

### 前台

```text
trajecta --project PROJECT run --profile PROFILE
```

命令先等待任务写入持久队列，再跟踪当前执行轮次到终态：

| 终态 | 退出码 | `run_success` |
| --- | ---: | --- |
| `complete` | `0` | `true` |
| `completed_with_particle_errors` | `1` | `false` |
| `failed` | `1` | `false` |
| `cancelled` | `1` | `false` |
| `interrupted` | `1` | `false` |

任务准入后关闭终端，只会断开该前台客户端。守护进程继续管理执行轮次，可通过 `job status` 或
`job wait` 重新连接。

### 后台

```text
trajecta --project PROJECT run --profile PROFILE --detach
```

退出 `0` 表示队列已经持久接受回执。工作进程可能仍在排队，也可能稍后失败。后续状态通过：

```text
trajecta job wait JOB_ID
```

`job wait` 采用与前台运行相同的终态退出码表。

## 事件与数据流

单次 `job events` 成功读取所请求批次后退出 `0`。跟踪一个任务时，最后一条 JSONL 汇总包含
`run_success`，进程退出码跟随所选执行轮次的终态。数据流读取或编码错误返回 `1`；如果输出已经
开始，会先写一条诊断项目，再写汇总。

气象探测和重放命令使用自己的原生数据流处理。读取器或数据流失败返回 `1`。机器可读模式要求
原生气象记录写到 `--output FILE`。

## 结果命令

| 命令 | 退出 `0` 表示 | 运行状态字段 |
| --- | --- | --- |
| `result inspect` | 已读取运行清单和可用产物；SQLite 不可用时可能返回带警告的部分结果 | `run_success` 反映清单生命周期 |
| `result verify` | 所选快速或完整验证成功完成 | `run_success` 表示被验证运行是否为 `complete` |
| `result trajectory` | 所有请求粒子 ID 均有效，记录已经写出 | 流头和汇总包含运行生命周期 |
| `run report` | `RESULT/run-report.md` 已写入或刷新 | 报告正文包含运行生命周期 |

`result verify` 可以成功验证某个结构完整的取消或失败终态，此时退出码可能为 `0`。下游需要
`complete` 时继续检查 `run_success`。

## Shell 示例

=== "Windows PowerShell"

    ```powershell
    .\trajecta.exe --format json job wait $jobId
    $code = $LASTEXITCODE
    if ($code -eq 0) { "complete" } else { "exit=$code" }
    ```

=== "Ubuntu 24.04"

    ```bash
    if ./trajecta --format json job wait "$job_id"; then
      echo complete
    else
      code=$?
      echo "exit=$code"
    fi
    ```

处理机器输出时，先解析完整 JSON 响应，再根据流程需要转换成 Shell 布尔状态。
[结构定义参考](schemas.md)列出完整的 JSON 响应与数据流结构。
