# Trajecta M5-A0 交付报告

日期：2026-07-27
结论：**M5-A0 合同与类型闭环 passed，可进入 M5-A1；未 commit / 未 push，不宣称 M5-A1 或 M5 完成。**

## 1. 本轮交付

- 建立 `TRAJECTA_M5_A0_PRODUCT_CONTRACT.md`，作为 M5 控制面最高权威；
- 冻结 35 个公开命令、human/JSON/JSONL 输出、默认前台运行、显式 `--detach` 和退出码；
- 冻结机器配置、项目索引、渐进编辑、configured/finalized 边界和确定性 data-plan；
- 冻结 9 个任务状态、状态转换、显式资源请求、持久 fan-out 事件、无自动重试和三次队首回填上限；
- 冻结 rerun/prune 仅生成 dry-run 清单，不提供 `--apply`，`job forget` 不删除产物；
- 新增 `trajecta-job` crate，提供任务身份、状态、事件、查询、资源和 `JobBackend` 类型边界；
- 写入 `B_PROMPT_M5_A1_CLI_PROJECT.md`，供外部 B 模型按冻结范围实施 M5-A1；
- 新增 M5 JSON Schema、TOML/YAML examples 和 `validate_m5_a0_contracts.py`。

## 2. 高风险合同修订

### 2.1 Manifest v1

现有未发布的 `trajecta.run-manifest/v1` 直接增加：

- `job_series_id`；
- 从 1 开始的 `attempt`；
- `cancelled`；
- `interrupted`。

直接运行保持 `job_series_id == run_id`、`attempt == 1`。没有新增 manifest v2、fixup 或 cancel 专用版本。

终态语义：

- `complete`、`completed_with_particle_errors`、已建立 run manifest 的 `cancelled` 必须有合法 provenance；
- `failed`、`interrupted` 必须有 failure 且禁止 provenance；
- 只有 `complete` 的 `run_success=true` 和 wait exit code 为 0。

### 2.2 启动前取消

排队任务允许在首份 running manifest 持久化前取消。此时只终结 job catalog 记录，不启动模拟、不创建 run 目录，也不伪造 SQLite、manifest 或 provenance。

一旦 running manifest 已持久化，只有 Runner 在宏步边界安全收尾并生成合法 provenance 后，才能把 run manifest 写成 `cancelled`。该边界已写入同一个 job-contract v1，没有扩版本。

### 2.3 编译期状态机

`JobState::allows_transition_to` 与 `M5_JOB_CONTRACT.v1.json` 逐状态、逐目标互证。终态无出边；提交回执只能处于 `queued`；`RunInput` 拒绝未冻结的额外字段，避免控制面夹带科学覆盖参数。

## 3. 数值核心边界

- 未修改积分、边界、population、气象查询或输出数值路径；
- 仅公开 `required_capabilities_for_population`，供后续 data-plan 调用既有科学映射，禁止 CLI 复制规则；
- 未改变容差、异常分类、SQLite schema 或 provenance bundle schema；
- 所有 crate 仍为开发版本 `0.0.0`。

## 4. 最终门禁

```text
cargo fmt --all -- --check                                  passed
cargo clippy --offline --workspace --all-targets -- -D warnings
                                                               passed
cargo test --offline --workspace --quiet                    464 passed / 11 ignored
cargo doc --offline --workspace --no-deps                   passed
python tools/validate_m4_a0_contracts.py                     passed
python tools/validate_m5_a0_contracts.py                     passed
python -m py_compile tools/validate_m5_a0_contracts.py       passed
git diff --check                                             passed
```

M5 validator 冻结结果：

```text
cli_commands=35
job_states=9
known_cli_placeholders=11
manifest_schema_version=trajecta.run-manifest/v1
pruning_mode=dry_run
```

11 个 placeholder 均为 M4 遗留 CLI 占位，处于固定 allowlist 内且数量未增加。M5-A1 必须把可达占位替换为真实实现或稳定的 `command.stage_not_available` 产品诊断。

## 5. 尚未实现

- CLI 的 M5-A1 config/project/data-plan/doctor 工程实现；
- `project finalize` 的 lock、资料覆盖和内容身份闭环；
- daemon、IPC、worker、lease、持久任务库和资源调度；
- Runner 安全取消控制点、worker 重连和 interrupted forensic 执行路径；
- run/job/result 的真实执行、rerun 和 prune 计算；
- M5 正式故障矩阵、打包、FLEXPART 科学与性能对比。

本轮没有运行 M5 真实长测、50k/100k、百万点、WSL native 或 FLEXPART。workspace 中 11 个 ignored 均保留为显式阶段测试，不以缩小规模冒充通过。

## 6. 约束遵守

- 未调用子代理；后续只使用 A/B，不设置 C；
- B 只通过书面 Prompt 由用户在外部启动；
- 未触碰 `E:\flexpart\origo-validation-v1.json`；
- 未 commit、未 push、未打 tag、未发布；
- 不宣称 M5-A1 或 M5 完成。

## 7. B 交接

可直接交给 B：

```text
docs/engineering/B_PROMPT_M5_A1_CLI_PROJECT.md
```

该 Prompt 把 B 限定在 CLI/config/project/data-plan/基础 doctor 的中等工程工作；daemon、状态转换、恢复、取消、finalize 裁决、schema、版本和所有数值核心仍由 A 负责。
