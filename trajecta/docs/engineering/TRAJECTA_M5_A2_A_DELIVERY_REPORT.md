# Trajecta M5-A2 A 交付报告

日期：2026-07-28

## 1. 结论

M5-A2 的 A 侧实现与本地验收已闭合：所有公开运行都进入同一个本地 daemon 和持久队列，独立 worker 可以在前台客户端退出后继续运行；daemon 崩溃后，新 daemon 能按 PID 与 OS start-token 重新关联仍存活的 worker；安全取消、强制取消、worker 失联和终态恢复均具有明确的 catalog、manifest、SQLite、provenance 与 forensic 语义。

本报告只宣称 **M5-A2 A 侧实现和本地门禁完成**，不宣称 M5 完成。当前未 commit、未 push、未打 tag、未发布。

代码体积和结构性精简不混入本轮，已单独冻结为 **M5-A2E**。

## 2. 实现范围

### 2.1 本地 daemon、IPC 与独立 worker

- daemon 按需启动，使用持久单实例 lease，并按配置空闲退出；
- daemon 与 worker 都使用同一发布二进制的隐藏内部模式，没有增加第二个业务程序；
- Windows 使用本机 named pipe，Linux 使用 Unix socket；IPC 为 typed request/response，单 frame 硬上限 4 MiB；
- worker 是独立 OS 进程，stdout/stderr 写入独立 worker log；
- Windows 后台 daemon 启动时临时禁止标准句柄继承，避免 `run --detach` 调用方因捕获管道一直等不到 EOF；
- daemon setup/bind 失败会释放已 claim 的 lease，不留下假 owner。

### 2.2 持久任务库与调度

- 新增 `trajecta-job`，使用 SQLite WAL 持久化 job series、run、attempt、状态、事件、资源申请、daemon lease 和 worker lease；
- 实现 FIFO、资源池、安全回填、队首最多越过三次以及不可满足资源请求的硬失败；
- 外部内存压力只暂停新派发，不取消运行中 worker；每个受影响的 queued attempt 只持久写一次 `daemon.external_memory_pressure` warning；
- terminal attempt 没有出边，不重新派发、不自动重跑；
- 多个事件消费者使用相同 cursor 时读取相同事件，不存在消费端抢占。

### 2.3 Runner 控制面

- Runner 只在冻结的安全边界暴露 heartbeat、进度快照和取消决策；
- 安全取消只在宏步边界生效，不改变积分、插值、边界、population 或气象查询结果；
- heartbeat/progress 写入 catalog，执行线程不通过 CLI 复制数值规则；
- 修复 heartbeat deferred transaction 与并发 cancel commit 的 SQLite WAL snapshot-upgrade 竞态：heartbeat 现在先取得短暂 write intent，避免合法取消被误报为 `run.control/database is locked`。

### 2.4 取消与产物语义

- queued attempt 取消只终结 catalog，不创建 run 目录、manifest、SQLite 或 provenance；
- safe cancel 进入 `cancelling`，由 worker 在安全边界生成合法 `Cancelled` manifest、终态 SQLite 和 provenance，terminal WAL 为 absent/0；
- `job wait` 对 Cancelled 返回进程退出码 1，并输出 `run_success=false`；
- force cancel 先按 PID+start-token 终止匹配 worker，再审计磁盘 manifest，最终状态为 `Interrupted` 或已存在的真实终态；
- running manifest 被强停时原子改为合法 `Interrupted`，保留 forensic，不生成 provenance；
- 已写终态 manifest、但尚未更新 catalog 就退出的 worker，不再被误判为 Interrupted，catalog 会采纳经验证的真实终态。

### 2.5 恢复与断连

- daemon 启动时检查旧 lease 的 PID 与 OS start-token，拒绝 PID reuse；
- live worker 被新 daemon 重新关联，worker lease 的 daemon instance 原子替换，并持久写 `worker.reattached`；
- worker 已消失且 manifest 仍为 Running 时，manifest 与 catalog 收口为 Interrupted，不自动重跑；
- 合法终态 manifest 被 catalog 采纳；损坏 manifest 保留 forensic，不用新文件覆盖；
- 默认前台 `run` 只是等待客户端；杀掉前台客户端不会取消、终止或收养 worker；
- `run --detach` 在 durable queue 接受后立即返回，与前台路径共用同一 daemon、worker、Runner 和输出实现。

## 3. 真实进程验收

主测试：

```text
cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture
```

真实 CFSR fixture：

```text
E:\flexpart\tools\flexctl\target\test-data\cfsr\20090101\raw
```

同一测试覆盖：

1. daemon 按需启动、catalog 持久化和空闲 lease 释放；
2. 前台真实 CFSR worker Complete，manifest 的 series/run/attempt 与 catalog 一致；
3. 超高内存 reserve 下 detached job 保持 queued，并产生去重的外部压力 warning；
4. 两个相同 cursor 的事件消费者获得完全相同的事件数组；
5. queued cancel 不产生 run artifact；
6. 10,000 粒子 safe cancel 生成合法 Cancelled manifest、SQLite、provenance 和 WAL 收尾；
7. 10,000 粒子 force cancel 生成合法 Interrupted forensic，不伪造 provenance；
8. 杀掉默认前台 `run` 客户端后，worker PID+start-token 仍存活且 job 保持 Running；
9. 再强杀 daemon，新 daemon 启动后重新关联同一个 worker identity，并写入 `worker.reattached`；
10. 重关联后的 worker 仍可由新 daemon 强停并完成 Interrupted 收尾。

该测试在最终 workspace 全量轮中通过，耗时约 81.5 秒。

## 4. 关键回归与故障覆盖

`trajecta-job` 的定向测试覆盖：

- durable queue、fan-out events 和 catalog reopen；
- daemon compare-and-swap owner 与 stale release；
- FIFO、安全回填、三次越过上限和资源超限；
- external pressure 暂停派发与 warning 去重；
- spawn/attach race 中的 safe cancel；
- terminal row 不再离开终态；
- daemon restart 的 live reattach 与 missing worker interrupt；
- 已存在终态 manifest 的 recovery reconcile；
- force-stop 失败保留 active forensic，成功后才 terminalize；
- force cancel 遇到已终态 manifest 时采用真实终态。

`trajecta-cli` 的定向测试覆盖：

- 完整 job 命令解析、event cursor/follow 和 force cancel；
- running manifest 到 Interrupted 的恢复；
- existing terminal manifest 的不重写采纳；
- daemon bind 失败释放 lease；
- 真实 CFSR foreground/detach/cancel/disconnect/restart 进程链。

## 5. 最终门禁

```text
cargo test --offline --workspace
→ 521 passed, 11 ignored

cargo test --offline -p trajecta-job
→ 31 passed

cargo test --offline -p trajecta-cli --lib
→ 17 passed

cargo test --offline -p trajecta-cli --test m5_a2_runtime_contracts -- --nocapture
→ 2 passed

cargo test --offline --manifest-path vendor/trajecta-local-ipc/Cargo.toml
→ 2 passed

cargo clippy --offline --workspace --all-targets -- -D warnings
→ passed

cargo clippy --offline --manifest-path vendor/trajecta-local-ipc/Cargo.toml --all-targets -- -D warnings
→ passed

cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
→ passed
```

M5 A0 validator 最终输出：35 个 CLI commands、9 个 job states、0 个 known CLI placeholders、pruning mode=`dry_run`。

## 6. 边界与诚实限制

- `job rerun`、`job forget`、`job prune`、`result inspect/verify/trajectory` 仍属于 M5-A3；
- M5 内仍不提供任何真实产物删除，rerun/prune 后续只能生成 dry-run 清单；
- Windows/WSL 正式产品矩阵、安装包、SBOM 和 clean-extraction 属于 M5-A4；
- FLEXPART 科学/性能对比和首次版本提升属于 M5-A5；
- 本轮没有修改数值算法、容差、异常分类或 M4 正式 evidence；
- 未触碰 `E:\flexpart\origo-validation-v1.json`；
- 未使用子代理；当前存在 B 模型，后续只通过书面 Prompt 分配机械复验；
- 未 commit、未 push，不宣称 M5 完成。

## 7. M5-A2E 冻结项

A2 新增功能已经闭合，但代码体积需要单独治理。当前优先审计文件包括：

```text
crates/trajecta-job/src/catalog.rs     2474 physical lines
crates/trajecta-cli/src/runtime.rs     1947 physical lines
crates/trajecta-cli/src/project.rs     2391 physical lines（主要来自 A1）
```

M5-A2E 只做拆分、去重、删除宽泛兜底/死代码和复用现有 helper；不得扩功能、建立平行版本、改变公开合同或改变任何数值/磁盘行为。精简后必须原样复跑本报告的真实进程链和全量门禁。

## 8. B 机械复验

给 B 的冻结 Prompt：

```text
docs/engineering/B_PROMPT_M5_A2_EXECUTION.md
```

B 只执行固定命令、采集 evidence 和报告首个失败，不修改 daemon、调度、恢复、Runner 生命周期或数值实现。
