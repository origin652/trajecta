---
title: Process exit code
description: 解释 Trajecta 成功命令、usage error、foreground 与 detached run、wait、inspect 和 verify 的 shell status。
---

# Process exit code

Trajecta 使用三个 product-level process exit code：

| Code | Shell 含义 |
| ---: | --- |
| `0` | Command 完成所请求的 operation |
| `1` | 已解析命令遇到 application、validation、runtime 或 non-successful terminal-run condition |
| `2` | Command-line syntax、option placement 或 argument value 在 parsing 期间失败 |

Operating-system loader error、process signal 和 forced termination 可能在 CLI 返回 product
status 前产生其他 platform value。

## Command success 与 run success

Machine output 将 command operation 和 scientific run 分开表示：

| Field | 含义 |
| --- | --- |
| `ok` | CLI operation 生成了所请求的 response |
| `run_success` | Command 报告 run lifecycle 时，selected attempt 是否为 `complete` success state |

例如，`result inspect` 可以成功读取 interrupted result。此时 process exit 为 `0`，`ok` 为
true，`run_success` 为 false。需要 completed scientific run 的 automation 可以同时检查两个值。

## Parsing 与 application error

Human mode 中，usage text 写入 stderr，process exit 为 `2`。Application error 返回 `1`。
Help 写入 stdout，并返回 `0`。

JSON 或 JSONL mode 中，parse failure 与 application failure 通过 stdout 的 structured envelope
或 diagnostic stream item 返回。Usage error 保留 exit `2`，已经解析的 application failure
返回 `1`。Structured application response 不会在 stderr 重复 diagnostic。

## Run submission

### Foreground

```text
trajecta --project PROJECT run --profile PROFILE
```

命令先等待 durable admission，再跟随 current attempt 到终态：

| Terminal state | Exit code | `run_success` |
| --- | ---: | --- |
| `complete` | `0` | `true` |
| `completed_with_particle_errors` | `1` | `false` |
| `failed` | `1` | `false` |
| `cancelled` | `1` | `false` |
| `interrupted` | `1` | `false` |

Admission 之后关闭终端，只会断开该 foreground client。Daemon 继续持有 accepted attempt，
可以通过 `job status` 或 `job wait` 重新连接。

### Detached

```text
trajecta --project PROJECT run --profile PROFILE --detach
```

Exit `0` 表示 queue 已经持久化接收 receipt。Worker 此时可能仍为 queued，也可能随后失败。
后续状态通过以下命令读取：

```text
trajecta job wait JOB_ID
```

`job wait` 与 foreground run 使用同一 terminal-state exit table。

## Event 与 stream

One-shot `job events` request 在成功读取当前 batch 后返回 `0`。跟随单个 job 时，最终 JSONL
summary 包含 `run_success`，process exit 跟随 selected attempt terminal state。Stream-wide
read 或 encoding error 返回 `1`；stream 已经开始时，summary 前会先出现 diagnostic item。

Meteorological probe 与 replay command 使用自己的 native stream processing，reader 或 stream
failure 返回 `1`。Machine envelope mode 要求 native meteorological record 写入
`--output FILE`。

## Result command

| Command | Exit `0` 的含义 | 附加 lifecycle field |
| --- | --- | --- |
| `result inspect` | Manifest 与 available artifact 已读取；SQLite partial inspection 可以带 warning 返回 | `run_success` 报告 manifest lifecycle |
| `result verify` | Selected quick 或 full verification operation 成功完成 | `run_success` 报告 verified run 是否为 `complete` |
| `result trajectory` | 全部 requested particle ID 已校验，selected record 已写出 | Header 与 summary 包含 run lifecycle |
| `run report` | `RESULT/run-report.md` 已写入或刷新 | Report body 包含 run lifecycle |

Structurally valid 的 cancelled 或 failed run 可以完成 selected verification check，此时
`result verify` 返回 `0`。Workflow 需要 `complete` 时，可继续读取 `run_success`。

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

Machine output 可以先解析完整 JSON response，再根据具体流程转换为 Boolean shell status。
[Schema 参考](schemas.md)给出准确 envelope 与 stream shape。
