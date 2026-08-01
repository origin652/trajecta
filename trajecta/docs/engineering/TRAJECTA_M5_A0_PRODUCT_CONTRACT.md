# Trajecta M5-A0 产品与控制面合同

状态：**A0 authority contract**

本文件冻结 M5 的公开命令、配置与项目边界、任务身份、状态机、事件、取消、恢复和清理语义。M5-A0 不实现 daemon、worker 或任务调度，也不改变任何 M4 数值路径。

可执行自校验：

```text
python tools/validate_m5_a0_contracts.py
```

## 1. 权威顺序与版本

发生冲突时按以下顺序裁决：

1. 本文件；
2. `testdata/M5_*` 唯一 v1 机器合同；
3. `crates/trajecta-job` 编译期类型；
4. `docs/engineering/TRAJECTA_M5_PRODUCTIZATION_PLAN.md`；
5. 实现报告和执行日志。

开发期所有 crate 保持 `0.0.0`。现有 run manifest 尚未发布，直接修订 `trajecta.run-manifest/v1`，禁止建立 manifest v2、cancel-v2 或 fixup 版本。

## 2. 公开命令与输出

命令面由 `M5_CLI_CONTRACT.v1.json` 唯一冻结。支持的命令族为：

```text
config  project  case  data  met  doctor  run  job  result
```

全局选项：

```text
--format human|json|jsonl
--json
--config PATH
--project PATH
```

规则：

- `--json` 等价于 `--format json`，同时使用属于用法错误；
- human 是简短交互输出；JSON 是单一完整对象；JSONL 用于事件、wait 和轨迹等流；
- follow/无限流模式不得使用单对象 JSON；
- JSON/JSONL 的 stdout 只写机器数据，warning 和进度不得混入；
- 路径使用操作系统原生字符串，不能假定为 UTF-8；机器 JSON 中无法表示的路径必须明确报错，不能 lossy 转换。

单结果 JSON 必须使用 `trajecta.cli-output/v1`；JSONL 每行必须使用 `trajecta.cli-stream-item/v1`。流的 sequence 从 1 递增，并且必须以唯一 summary 行终止；异常断流没有 summary，消费者据此识别不完整输出。

### 2.1 默认前台与显式后台

- `trajecta run` 默认 `foreground_wait`：提交给 daemon 后持续等待终态；
- `trajecta run --detach` 在 durable queue 接受后返回 job identity；
- 关闭前台客户端不等于取消，worker 必须继续运行；
- 用户只能通过 `job cancel` 请求取消；
- 前台与后台共用完全相同的 daemon、worker、Runner 和输出路径。

### 2.2 退出码

```text
0 = Complete，或 detached 请求已持久接受
1 = completed_with_particle_errors / failed / cancelled / interrupted / 产品错误
2 = 参数、命令、格式或用法错误
```

`result verify` 根据产物完整性返回，独立输出 `run_success`。合法 cancelled 产物允许 verify=0、run_success=false。

## 3. 配置合同

机器配置 schema 为 `trajecta.config/v1`，默认 TOML 文件位置：

- Windows：`%APPDATA%\Trajecta\config.toml`；
- Linux：`$XDG_CONFIG_HOME/trajecta/config.toml`，无变量时为 `~/.config/trajecta/config.toml`。

配置路径优先级：

1. `--config PATH`；
2. `TRAJECTA_CONFIG` 环境变量；
3. 操作系统默认路径。

配置只拥有：

- daemon 本机控制参数；
- 明确 CPU 和内存池；
- 任务级监控参数；
- 默认 reader backend；
- RunProfile 的 execution 模板。

科学 Case、资料路径和具体 DatasetLock 不得进入机器全局配置。默认 reader 必须为 Rust；native 只能显式选择。

`config init` 将检测结果写成明确数值。daemon 之后使用持久值，不按每次启动时的空闲资源偷偷改写资源池。

`config get/set/unset` 使用点路径。`set` 的值先按 JSON 值解析；无法解析时作为普通字符串，因此 `4` 是数字、`true` 是布尔值、`"4"` 是字符串。写入采用同目录临时文件、flush、原子 replace；失败不得损坏旧文件。

Profile 模板只快照 `ExecutionSpec`：worker、memory、executor 和默认 meteorology reader。它不保存 Case 路径、output root、lockfile 或 data root。模板更新不会自动改变项目，必须显式 reapply，并在项目索引记录模板 SHA-256。

## 4. 项目索引与渐进编辑

项目根使用：

```text
trajecta-project.yaml
cases/
profiles/
locks/
runs/
```

`trajecta-project.yaml` 的 schema identity 为 `trajecta.project-index/v1`。它只映射具名 Case、具体 RunProfile，以及下载前所需的逻辑 dataset → profile 名称；不复制科学字段，不存储计算得到的 project state。

项目路径选择优先级：

1. `--project PATH`；
2. 当前目录及父目录中最近的 `trajecta-project.yaml`；
3. 未找到即报明确错误，不扫描整块磁盘。

一个项目可有多个 Case 和多个具名 Profile；一个具体 RunProfile 仍必须绑定一个 `case_path`。模板可生成多个具体 Profile，但不创建“一个 Profile 动态绑定多个 Case”的新语义。

### 4.1 逐项编辑

`project get/set/unset` 接受以下命名空间：

```text
index.<field path>
case.<case-name>.<field path>
profile.<profile-name>.<field path>
```

Case/Profile 尚不完整时使用通用 YAML mapping 保存；达到完整形状后必须由现有 `trajecta-case` parser 和 validator 接管。类型错误、未知字段和互相矛盾的值是 error；单纯缺少后续必需字段是 draft。

命令重写为规范 YAML，不保证保留注释或原始字段顺序。每次写入都必须原子化。`unset` 可以使项目从 finalized/configured 降级，不得维持过期状态。

### 4.2 状态推导

```text
draft -> configured -> finalized
```

- `draft`：缺少 Case/Profile、必需字段或 simulation intent 组件；
- `configured`：索引、Case 和 Profile 形状及逻辑完整，但资料、lock 或本机路径可尚未就绪；
- `finalized`：完整 resolver、DatasetLock、内容 SHA、资料覆盖和输出路径检查全部通过。

state 永远重新计算，不接受用户手工 set。

configured 要求每个具体 RunProfile 的 dataset binding 都能在对应索引条目的 `dataset_profiles` 中找到唯一 profile 名；这解决“先配置、后生成 lock”时 lock 尚不存在而无法知道资料 profile 的问题。多余或缺失的 mapping 都是 error。

`project validate` 在 configured 阶段把计划内缺资料报告为 pending/warning；若项目宣称可 finalized 而 lock 不一致、资料损坏或时间覆盖不足，则为 error。

## 5. Data plan 与 finalize 边界

`project data-plan` 输出 `trajecta.data-plan/v1`：

- 不包含生成时间、随机 ID 或本机扫描顺序；
- requirement 按 `(profile_name, case_name, dataset_id)` 排序；
- capability 字符串排序去重；
- 每项明确资料 profile、时间覆盖、lockfile、roots、reader 和 ready 状态；
- 独立 Python 下载助手只消费该 JSON，不读取项目内部 Rust 类型。

允许先 configured 后下载。运行 configured 项目时可自动调用与 `project finalize` 相同的流程，但任何失败都必须停止，不能边运行边远程下载。

finalize：

- 缺失 lock 时可使用已经存在的本地资料创建；
- 已有 lock 与内容、profile 或需求不一致时 hard fail；
- 不自动覆盖已有 lock；
- 更新 lock 必须由用户显式执行 `data lock --replace`；
- A 负责 finalize 的具体实现与复验，B 不自行决定 lock 更新策略。

## 6. 身份与 manifest v1

每个运行具有：

- `job_series_id`：逻辑任务系列 UUIDv7；
- `run_id`：本次 attempt 的唯一 UUIDv7；
- `attempt`：系列内从 1 开始递增。

直接模式没有 daemon rerun 历史时，默认 `job_series_id == run_id` 且 `attempt == 1`。

run manifest v1 现在强制包含三项身份。不得复用或覆盖 RunID。`superseded_by` 属于 job catalog/attempt summary，不回写已完成 manifest。

## 7. Job 状态机

唯一状态与转换由 `M5_JOB_CONTRACT.v1.json` 冻结：

```text
queued -> starting -> running -> terminal
                 \-> cancelling -> cancelled
```

完整状态：

```text
queued
starting
running
cancelling
complete
completed_with_particle_errors
failed
cancelled
interrupted
```

终态没有任何出边。daemon 启动时只恢复记录：

- terminal：只加载，不派发；
- queued：仍在队列中；
- running/starting/cancelling：先尝试 worker lease 重新关联；
- worker 不存在或无法确认时转 interrupted，不重跑。

永不自动重试。

## 8. Local JobBackend 边界

`crates/trajecta-job` 冻结同步控制面接口：

```text
submit
list
status
cancel
events
```

实现只能接收已经规范化的 project/direct input 和显式资源请求。它不得解析科学规则、修改 Case/Profile 或执行数值算法。

M5 只实现 Local backend。未来 SLURM 只能实现相同控制边界；M5 不增加动态插件、远程 TCP 或 SLURM 代码。

## 9. 调度资源合同

- 每个 attempt 明确请求 cpu slots、memory MiB 和 worker threads；
- worker threads 不得超过 cpu slots；
- 请求超过全局池时提交失败，不能永久排队；
- FIFO 为基础顺序；
- 队首暂时放不下时可安全回填；
- 队首最多被越过三次，之后为其保留资源；
- 外部内存压力只暂停新派发并告警，不自动取消运行中任务。

## 10. 取消、中断与产物

`cancelled`：

- 只由安全取消产生；
- 在首份 running manifest 持久化之前取消时，只终结 job catalog 记录，不启动模拟、不创建 run 目录，也不伪造 manifest、SQLite 或 provenance；
- 一旦 running manifest 已持久化，只有在宏步边界完成合法输出收尾后才能把该 manifest 写成 cancelled；
- Runner 在宏步边界响应；
- 已存在的 cancelled run manifest 必须具有 finished_at 和合法 provenance bundle；
- 不具有 failure；
- 可以保留仍存活粒子的最后已完成状态；
- run_success=false。

`interrupted`：

- 用于 force stop、OOM kill、关机或 worker 消失；
- 必须具有 finished_at 和 failure；
- 禁止声明 provenance；
- SQLite/WAL/临时文件按 forensic 原样保留；
- 不伪造成功终态。

`failed` 同样禁止 provenance；受控失败必须有 finished_at 和 failure。

## 11. 持久事件

事件 schema 为 `trajecta.job-event/v1`，全局 sequence 从 1 单调递增。事件读取是 fan-out、非破坏性的：任何客户端读取都不改变事件，也不会阻止其它客户端从相同 cursor 再读。

`job events` 可读取全部任务或一个 series；`--since` 表示严格大于该 cursor；`--follow` 由客户端反复等待，不改变 backend 查询语义。

事件只包含任务级进度和资源数据，不包含逐粒子遥测。正式验收要求监控中位开销不超过 1%。

## 12. Rerun、forget 与 prune

- rerun 创建同 series 的新 run_id 和 attempt；
- 活动 attempt 先安全取消；
- 新 attempt 只有 Complete 且 full verify 通过后，旧 attempt 才能列入未来候选；
- M5 不实现任何真实删除；
- `job prune` 只输出 `trajecta.prune-plan/v1` dry-run 清单；
- 不存在 `--apply`；
- `job forget` 只改变 catalog 可见性，不删除 artifact；
- 测试必须比较执行前后文件、大小和 digest，证明零删除。

## 13. A0 已知占位与退出条件

M5-A0 允许 M4 遗留的 CLI parser/renderer 占位仍存在，但 validator 固定 allowlist 和数量上限，禁止新增占位。A1/A2/A3 必须按阶段逐步消除；M5 最终支持命令面不得有可达 `NotImplemented`。

M5-A0 完成必须满足：

- 所有 M5 JSON Schema 为 Draft 2020-12 且 example 通过；
- config TOML 和 project YAML 语义门禁通过；
- CLI 命令、默认前台、detach 和退出码冻结；
- Job 状态、转换、无自动重试、三次回填上限和非破坏事件冻结；
- prune 为 dry-run 且删除禁用；
- manifest v1 Rust/JSON 同时支持 series/attempt/cancelled/interrupted；
- `trajecta-job` 类型和 backend trait 编译；
- M4 既有测试不回退；
- fmt、clippy、test、doc、M4 A0、M5 A0 和 diff-check 通过；
- 未实现 daemon，未改变数值核心，不宣称 M5-A1 或 M5 完成。
