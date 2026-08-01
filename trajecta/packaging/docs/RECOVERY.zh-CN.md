# 故障急救

1. 保留结果目录、任务数据库、SQLite sidecar 和 forensic 文件。
2. 运行 `trajecta job status JOB_ID` 与 `trajecta result inspect RESULT`。
3. 接收新任务前运行 `trajecta doctor --deep`。
4. 原 attempt 进入终态后，才能使用 `job rerun`。
5. 当前版本的 `job prune` 仅生成 dry-run 计划，不删除产物。

不要手工删除 WAL，也不要改写 run manifest。
