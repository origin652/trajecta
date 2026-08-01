---
title: Process exit codes
description: Stable process exit meanings for Trajecta commands, detached admission, and non-successful terminal runs.
---

# Process exit codes

| Code | Meaning |
| --- | --- |
| `0` | Command succeeded, or a detached job was durably admitted |
| `1` | Application failure, failed validation, or a run reached a non-success terminal state |
| `2` | Command-line usage or parse error |

For run-related commands, inspect the machine envelope as well as the process
status. `ok` states whether the command itself completed. `run_success`, when
present, states whether the selected run completed successfully. A detached
submission exits `0` after admission; later worker failure is observed with
`job status`, `job wait`, or `job events`.

Signals, forced process termination, and operating-system loader failures may
produce platform exit values outside this table. Treat them as interrupted
execution and preserve the job database plus attempt directory.
