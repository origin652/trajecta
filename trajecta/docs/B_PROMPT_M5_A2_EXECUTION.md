# B Prompt：M5-A2 daemon/queue/recovery 机械复验

你是 B，只负责冻结源码上的机械执行、证据采集和报告。A 已完成 M5-A2 的 daemon、持久队列、独立 worker、取消与恢复实现；你不得修改这些实现，也不得自行修绿。

## 1. 工作目录与必读文件

工作目录：

```text
E:\flexpart\trajecta
```

开始前完整阅读：

```text
docs/TRAJECTA_M5_A0_PRODUCT_CONTRACT.md
docs/TRAJECTA_M5_PRODUCTIZATION_PLAN.md
docs/TRAJECTA_M5_MODEL_ASSIGNMENT.md
docs/TRAJECTA_M5_A2_A_DELIVERY_REPORT.md
```

## 2. 严格边界

- 不使用子代理；
- 不修改任何 Rust/Python/TOML/schema/contract/test 实现；
- 唯一允许新增或修改的仓库文件是最终执行报告：

  ```text
  docs/TRAJECTA_M5_A2_B_EXECUTION_REPORT.md
  ```

- 不修改版本号；所有 crate 继续保持 `0.0.0`；
- 不修改积分、插值、边界、population、气象查询、容差或异常分类；
- 不修改 daemon 调度、恢复决策、状态转换或 Runner 生命周期；
- 不修改 M4 正式 evidence；
- 不触碰 `E:\flexpart\origo-validation-v1.json`；开始和结束时分别记录其 SHA-256；
- 不实现或执行真实产物删除；`rerun/prune` 仍只能是 dry-run 边界；
- 不 commit、不 push、不建 tag、不发布；
- 不宣称 M5-A2、M5 或发布完成；最终裁决交 A；
- 不做 M5-A2E 精简，本轮只验证当前冻结实现。

如果任何 mandatory gate 首次失败：立即停止后续 mandatory 阶段，保留第一失败的完整 stdout/stderr、退出码和 artifact；不得重试、改参数、缩小规模、增加 sleep、放宽断言或修改源码。

## 3. 开始身份记录

记录到报告：

```powershell
git rev-parse HEAD
git branch --show-current
git status --short
git diff --check
Get-FileHash -Algorithm SHA256 docs\TRAJECTA_M5_A2_A_DELIVERY_REPORT.md
Get-FileHash -Algorithm SHA256 E:\flexpart\origo-validation-v1.json
```

另记录：

- Windows 版本与架构；
- `rustc -Vv`；
- `cargo -V`；
- CFSR fixture 是否完整存在：

  ```text
  E:\flexpart\tools\flexctl\target\test-data\cfsr\20090101\raw
  ```

至少确认 `pgbl00.gdas.2009010100.grb2`、`...0106...`、`...0112...` 三个文件存在。缺资料属于 `external_blocked`，不得伪造通过。

## 4. Windows mandatory gates

严格按以下顺序执行。每条记录命令、开始/结束时间、退出码和结果。

```powershell
cargo fmt --all -- --check

cargo test --offline -p trajecta-job

cargo test --offline -p trajecta-cli --lib

cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture

cargo test --offline --manifest-path vendor\trajecta-local-ipc\Cargo.toml

cargo test --offline --workspace

cargo clippy --offline --workspace --all-targets -- -D warnings

cargo clippy --offline --manifest-path vendor\trajecta-local-ipc\Cargo.toml --all-targets -- -D warnings

cargo doc --offline --no-deps

python tools\validate_m5_a0_contracts.py

python tools\validate_m4_a0_contracts.py

git diff --check
```

冻结预期：

```text
workspace: 521 passed / 11 ignored
trajecta-job: 31 passed
trajecta-cli lib: 17 passed
m5_a2_runtime_contracts: 2 passed
trajecta-local-ipc: 2 passed
M5 validator: 35 CLI commands / 9 job states / 0 placeholders / pruning_mode=dry_run
```

若总数因报告之外的并行源码变化而不同，立即停止并报告 source drift；不得自行更新预期。

## 5. 真实进程合同审计

`m5_a2_runtime_contracts` 必须实际运行，不得只 `--no-run`，不得因耗时约 80 秒而中断。报告逐项确认测试断言覆盖并通过：

1. daemon 按需启动、单实例 lease 和空闲退出；
2. 真实 CFSR 前台 Complete，catalog/manifest identity 一致；
3. external memory pressure 只暂停新派发，并持久写去重 warning；
4. 相同 cursor 的两个事件消费者得到相同事件；
5. queued cancel 不创建 run artifact；
6. 10,000 粒子 safe cancel：catalog/manifest=Cancelled、provenance 存在、SQLite 存在、terminal WAL absent/0、`run_success=false`；
7. 10,000 粒子 force cancel：catalog/manifest=Interrupted、forensic 保留、provenance 不存在；
8. 杀掉默认前台客户端后，独立 worker 仍存活且 job 仍为 Running；
9. 杀掉 daemon 后，新 daemon 以原 worker PID+start-token 重关联，并出现 `worker.reattached`；
10. 重关联 worker 可由新 daemon 正确强停和收尾。

成功测试会清理自己的临时目录。若失败，`target/m5-a2-runtime-*` 会保留 forensic；只读检查最新失败目录，不删除、不覆盖，并在报告列出完整路径。

## 6. WSL/Linux smoke

只有 Windows mandatory gates 全部通过后才执行。

先只读检查 WSL、Rust toolchain、离线依赖和 `/mnt/e/flexpart` 是否可达。不得安装包、不得联网下载、不得修改系统配置。

若环境可用，使用独立 target 目录：

```bash
cd /mnt/e/flexpart/trajecta
export CARGO_TARGET_DIR=/tmp/trajecta-m5-a2-b-target
cargo test --offline --manifest-path vendor/trajecta-local-ipc/Cargo.toml
cargo test --offline -p trajecta-job
cargo test --offline -p trajecta-cli --lib
cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture
```

WSL 首个失败后停止该分支，不重试。若缺少 `cargo`、离线 crate cache、CFSR mount 或本机 IPC 能力，标记 `external_blocked` 并保留原始命令/错误；不得把 external_blocked 写成 passed。

WSL smoke 不替代 M5-A4 的正式跨平台产品矩阵。

## 7. 结束审计

执行并记录：

```powershell
git status --short
git diff --check
Get-FileHash -Algorithm SHA256 E:\flexpart\origo-validation-v1.json
Get-Process | Where-Object { $_.ProcessName -like 'trajecta*' } | Select-Object Id,ProcessName,Path
```

要求：

- origo 文件开始/结束 SHA 完全相同；
- 除最终 B 报告外没有 B 造成的源码/合同变化；
- 没有测试遗留的 Trajecta daemon/worker；
- 若存在遗留进程，只报告 PID、路径和对应 forensic，不擅自杀死无法证明归属的进程；
- 版本仍为 `0.0.0`；
- `git diff --check` 通过。

写完报告后只补跑：

```powershell
cargo fmt --all -- --check
git diff --check
```

不要因为报告文件改变 source tree 而重跑真实进程矩阵。

## 8. 报告格式

报告必须包含：

- 开始 HEAD、branch、dirty-tree 摘要和 A 报告 SHA；
- host、Rust、CFSR fixture 身份；
- Windows 每条 mandatory gate 的命令、耗时、退出码和结果；
- 真实进程合同 10 项逐项结果；
- WSL 每条 smoke 结果，或精确 external_blocked 原因；
- workspace passed/ignored、job/CLI/IPC 定向计数；
- 首个失败路径与 forensic（如有）；
- 开始/结束 origo SHA；
- 最终 `git status --short` 与遗留进程检查；
- 明确写：未修改实现、未 commit/push、未做 A2E、不宣称 M5-A2/M5 完成，交 A 裁决。

如果全部 Windows mandatory gates 和真实进程合同通过，可写“B 机械复验通过”；仍不得代替 A 宣称 M5 完成。
