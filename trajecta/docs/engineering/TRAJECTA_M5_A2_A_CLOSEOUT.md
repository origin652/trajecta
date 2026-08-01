# Trajecta M5-A2 A Closeout

日期：2026-07-28

## 1. 裁决

**M5-A2 验收通过，阶段完成。**

A 的实现、本地全量门禁、Windows B 机械复验和 WSL/Linux smoke 均通过。M5-A2 冻结范围内没有剩余 P0/P1 功能缺口。

该裁决只关闭 M5-A2，不宣称 M5 完成、发布完成或 M5-A2E 完成。

## 2. 复验证据

B 报告：

```text
docs/engineering/TRAJECTA_M5_A2_B_EXECUTION_REPORT.md
SHA-256 28a3c09cd8a3ca5414581363dad634c107405bf6c421a222766b325cdc307e14
```

Windows：

```text
workspace                  521 passed / 11 ignored
trajecta-job                31 passed
trajecta-cli lib            17 passed
m5_a2_runtime_contracts      2 passed（真实 CFSR，82.85 s）
trajecta-local-ipc           2 passed
```

WSL Ubuntu 24.04：

```text
trajecta-local-ipc           2 passed
trajecta-job                31 passed
trajecta-cli lib            16 passed（Linux 条件编译差异）
m5_a2_runtime_contracts      2 passed（真实 CFSR，89.55 s）
```

Windows 与 WSL 均验证：

- 前台与 detached 运行进入同一个 durable daemon/worker 路径；
- 前台客户端断连不取消 worker；
- daemon 崩溃后按 PID+OS start-token 重关联 live worker；
- queued/safe/force cancel 的 catalog、manifest、SQLite、WAL、provenance 和 forensic 语义；
- 外部内存压力暂停新派发并产生去重 warning；
- 多消费者 event cursor fan-out；
- 无遗留 Trajecta 进程。

fmt、workspace/local-IPC clippy `-D warnings`、doc、M5/M4 validator 和 `git diff --check` 均通过。origo 文件开始/结束 SHA 相同，crate 版本保持 `0.0.0`。

## 3. 报告措辞校正

B 报告列出的 8 个 `target/m5-a2-runtime-*` 目录创建时间均早于本次 B 复验，是 A 调试首失败时保留的既存 forensic。成功的 runtime contract 会清理本轮临时目录。因此这些目录可继续保留用于取证，但不作为 B 本轮成功执行的新 artifact。

该措辞差异不影响命令退出码、真实 CFSR 断言、Windows/WSL 结果或 A2 验收。

## 4. 后续边界

- M5-A2E：处理 A1/A2 代码拆分、去重、宽泛兜底和死代码审计；尚未开始；
- M5-A3：任务操作、result verify/trajectory、rerun/forget/prune dry-run；
- M5-A4：正式跨平台产品矩阵与打包；
- M5-A5：FLEXPART 科学/性能对比和首次预发布。

未 commit、未 push、未打 tag、未发布。A2E 开始前不得把结构精简与 A3 新功能混在同一轮修改中。
