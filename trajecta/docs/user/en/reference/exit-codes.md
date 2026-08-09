---
title: Process exit codes
description: Interpret Trajecta shell status for successful commands, usage errors, foreground and detached runs, waits, inspection, and verification.
---

# Process exit codes

Trajecta uses three product-level process exit codes:

| Code | Shell meaning |
| ---: | --- |
| `0` | The command completed its requested operation |
| `1` | The parsed command encountered an application, validation, runtime, or non-successful terminal-run condition |
| `2` | Command-line syntax, option placement, or an argument value failed during parsing |

Operating-system loader errors, process signals, and forced termination can
produce other platform values before the CLI returns a product status.

## Command success and run success

Machine output separates the command operation from the scientific run:

| Field | Meaning |
| --- | --- |
| `ok` | The CLI operation produced its requested response |
| `run_success` | The selected attempt has the `complete` success state, when a command reports run lifecycle |

For example, `result inspect` can successfully inspect an interrupted result.
The process exits `0` and `ok` is true, while `run_success` is false. Automation
that needs a completed scientific run checks both values.

## Parsing and application errors

In human mode, usage text is written to stderr and the process exits `2`.
Application errors exit `1`. Help exits `0` and is written to stdout.

In JSON or JSONL mode, parse and application failures use a structured envelope
or diagnostic stream item on stdout. A usage error retains exit `2`; a parsed
application failure returns `1`. Structured application responses do not also
write their diagnostic to stderr.

## Run submission

### Foreground

```text
trajecta --project PROJECT run --profile PROFILE
```

The command first waits for durable admission and then follows the current
attempt to a terminal state:

| Terminal state | Exit code | `run_success` |
| --- | ---: | --- |
| `complete` | `0` | `true` |
| `completed_with_particle_errors` | `1` | `false` |
| `failed` | `1` | `false` |
| `cancelled` | `1` | `false` |
| `interrupted` | `1` | `false` |

Closing the terminal after admission disconnects that foreground client. The
daemon continues to own the accepted attempt; reconnect with `job status` or
`job wait`.

### Detached

```text
trajecta --project PROJECT run --profile PROFILE --detach
```

Exit `0` means the queue durably accepted the receipt. The worker may still be
queued or may fail later. Read its later status with:

```text
trajecta job wait JOB_ID
```

`job wait` uses the same terminal-state exit table as a foreground run.

## Events and streams

A one-shot `job events` request exits `0` when the requested batch is read. In
follow mode for one job, the final JSONL summary includes `run_success`. The
process exit follows the selected attempt's terminal state. A stream-wide read
or encoding error exits `1`, and a diagnostic item precedes the summary when the
stream has already started.

Meteorological probe and replay commands use their own native stream processing
and return `1` for a reader or stream failure. Machine envelope mode requires
native meteorological records to be directed to `--output FILE`.

## Result commands

| Command | Exit `0` means | Additional lifecycle field |
| --- | --- | --- |
| `result inspect` | Manifest and available artifacts were inspected; partial SQLite inspection may be returned with a warning | `run_success` reports manifest lifecycle |
| `result verify` | The selected quick or full verification operation completed successfully | `run_success` reports whether the verified run is `complete` |
| `result trajectory` | All requested particle IDs were validated and the selected records were written | Header and summary include run lifecycle |
| `run report` | `RESULT/run-report.md` was written or refreshed | Report body includes run lifecycle |

`result verify` can return `0` for a structurally valid cancelled or failed run
whose available terminal product passes the selected verification checks. Read
`run_success` when the workflow requires `complete`.

## Shell examples

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

For machine output, parse the complete JSON response before replacing it with a
single Boolean shell status. The [schema reference](schemas.md) gives the exact
envelope and stream shapes.
