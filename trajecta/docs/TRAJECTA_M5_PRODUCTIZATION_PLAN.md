# Trajecta M5 产品化计划

## 1. 目标与完成定义

M5 将 M4 已完成的数值核心包装成可供科研人员长期使用的产品：

- 用命令正确配置、检查并驱动完整数值核心；
- 支持多个项目、Case 和 RunProfile；
- 支持先配置项目、后下载气象资料；
- 通过持久化本地队列运行一个或多个任务；
- 支持查看进度、安全取消、断电恢复、显式重跑和结果验证；
- 生成 Windows x64 与 Linux x86_64 便携预发布包；
- 在最终验收阶段与 FLEXPART 做科学可比较性和性能对比。

M5 不增加新物理过程或数值算法，不实现 GUI、远程运行时 I/O、SLURM、动态插件或运行中自动下载资料。产品矩阵最高 10,000 粒子，专项性能测试最高 50,000 粒子，不再运行百万粒子门禁。

完整中英文教程、性能图解读、AI/人类渐进配置指南和系统化 troubleshooting 独立放入 M5.1，由 A 与用户讨论后定稿。M5 只交付必要 help、最小示例、许可证、预发布限制和基础故障提示。

## 2. 命令与运行方式

统一发布一个 `trajecta` 二进制，公开命令面冻结为：

```text
trajecta config init|path|list|get|set|unset|validate

trajecta project init|status|show|get|set|unset|validate|data-plan|finalize

trajecta case validate|resolve
trajecta data inspect|lock
trajecta met probe|replay
trajecta doctor [--deep]

trajecta run [--detach]

trajecta job list|status|wait|events|cancel|rerun|forget|prune

trajecta result inspect|verify|trajectory
```

全局输出支持：

```text
--format human|json|jsonl
```

现有 `--json` 保留为 `--format json` 的简写。流式事件和大规模轨迹优先使用 JSONL，禁止把全部粒子轨迹一次性载入内存。

### 2.1 前台与后台语义

- `trajecta run` 默认以前台方式运行：任务仍提交给本地 daemon，但当前命令持续等待任务终态并显示进度；只有 `Complete` 返回退出码 0。
- `trajecta run --detach` 以后台方式运行：任务成功进入持久队列后立即返回 job ID，daemon 和 worker 在终端退出后继续工作。
- 后台任务通过 `job status`、`job wait` 和 `job events --follow` 查看或重新连接。
- 前台命令被关闭不会自动杀死 worker；用户需要使用 `job cancel` 明确取消。
- 所有前台和后台任务使用同一队列、身份、恢复和审计合同，不存在两套数值执行路径。

## 3. 配置和项目模型

### 3.1 机器配置

机器级配置使用 TOML，只保存 daemon、CPU/内存资源池、worker、监控、空闲退出、默认 reader backend 和具名 RunProfile 模板。

默认路径：

- Windows：`%APPDATA%\Trajecta\config.toml`；
- Linux：`$XDG_CONFIG_HOME/trajecta/config.toml`，无该变量时使用 `~/.config/trajecta/config.toml`。

`config get/set/unset` 使用稳定点路径并原子替换文件。每次修改后立即进行类型和局部语义检查，使人类或 AI 可以多轮逐项配置，而不是一次生成完整 JSON。

`config init` 将探测到的 CPU 和内存转换成明确、可编辑的资源池值；daemon 后续只使用已保存值，不随运行时环境偷偷改变配置。

### 3.2 项目结构

项目默认使用 YAML：

```text
trajecta-project.yaml
cases/
profiles/
locks/
runs/
```

`trajecta-project.yaml` 只是项目索引和编排配置，不复制或取代已有 Case、RunProfile 和 DatasetLock 科学合同。

项目状态由内容推导：

```text
draft -> configured -> finalized
```

- `draft` 允许字段不全，但不能运行；
- `configured` 表示 Case、Profile 和资料需求逻辑完整，资料或 lock 可以尚不存在；
- `finalized` 表示资料存在、lock 一致，所有路径和解析结果已经可运行。

行为冻结如下：

- `project validate` 按当前阶段检查；configured 状态下尚未下载资料只报告 pending/warning；
- `project data-plan` 输出稳定 JSON，供独立 Python 下载助手消费；
- `project finalize` 创建缺失的 DatasetLock；
- 已有 lock 与当前资料不一致时 hard fail，不得自动覆盖；
- 更新资料后只能通过显式 `data lock --replace` 更新 lock；
- 对 configured 项目执行 `run` 时自动尝试 finalize，资料不足则明确失败并提示生成 data plan；
- 修改 finalized 项目会使其退回 configured 或 draft，不允许继续冒充 finalized。

### 3.3 Case、Profile 和模板

一个项目可以包含多个 Case 和多个具名 RunProfile，但一个具体 RunProfile 仍只绑定一个 `case_path`。

多个 Case 复用配置时使用机器配置中的 Profile 模板。模板应用时生成具体 Profile 快照；模板后续修改不会偷偷改变已有项目，必须显式 reapply。每次运行仍保存完整 resolved Case、Profile 和 lock identity。

## 4. Doctor 与资料准备

`doctor` 检查配置解析、目录权限、资源池、daemon/IPC、reader backend、项目状态和资料可达性。

`doctor --deep` 进一步执行真实 reader 打开、代表字段 probe、native backend 检查、临时 SQLite 写入和 daemon 启停检查，但不启动正式模拟。

允许先配置后下载：configured 项目缺少计划内资料时，doctor 报告 pending 而非配置错误；如果项目声称 finalized 却缺少资料或 lock 不一致，则必须返回错误。

## 5. 本地 daemon、队列与调度

所有运行都经过本地单用户 daemon：

- Windows 使用 named pipe；
- Linux 使用 Unix domain socket；
- 不开放 TCP；
- daemon 按需启动，空闲后自动退出；
- daemon 和 worker 使用同一发布二进制的隐藏内部模式，不增加第二个发布程序；
- 每个任务由独立 worker 进程执行，以隔离崩溃和内存溢出；
- worker 继续使用现有 `RunnerBuilder -> MetEngine -> SimulationRunner -> SQLite/provenance` 产品链，不复制数值逻辑。

daemon 使用持久化 SQLite 任务库，保存任务、attempt、事件、资源申请和 worker lease。

每个任务明确申请 CPU slots、memory MiB 和 worker 数量。调度规则：

- 不允许超过机器配置中的 CPU 或内存总池；
- 基本顺序为 FIFO；
- 队首暂时无法放入时允许安全回填后续小任务；
- 队首最多被越过三次，之后为它保留资源，避免长期饥饿；
- 请求本身超过总资源池时在提交阶段直接拒绝；
- 检测到外部内存压力时暂停派发新任务并告警，但不自动杀死正在运行的任务；
- 永不自动重试。

完成、取消或合法失败的任务在 daemon 重启后绝不自动重跑。

只实现 Local `JobBackend`，同时冻结未来 SLURM backend 所需的提交、状态、取消和事件边界；M5 不实现 SLURM。

## 6. 生命周期、取消和异常恢复

公共任务状态至少包括：

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

当前 manifest v1 尚未发布，M5 直接加入 `cancelled` 和 `interrupted`，不创建 manifest v2。

安全取消：

- `job cancel` 设置取消请求；
- Runner 只在宏步边界响应，不在数值核中间破坏状态；
- SQLite checkpoint、provenance 和 manifest 被合法终态化；
- manifest 为 `cancelled`，保留已完成的所有轨迹；
- `result verify` 可以验证取消产物完整，但报告 `run_success=false`。

强制取消使用 `job cancel JOB --force`，立即终止 worker，状态标为 `interrupted`，保留 forensic，不伪造 provenance 或 Complete。

异常恢复：

- daemon 单独崩溃时，仍在运行的 worker 继续工作，新 daemon 根据 lease、PID 和 IPC 重新关联；
- 突然关机、OOM kill 或 worker 消失时，重启后将任务标为 interrupted；
- 不覆盖原 attempt，不自动续跑或重跑；
- 已完成任务只重新加载状态，不重新执行；
- 损坏或未终态化的 SQLite/WAL、manifest 和临时文件全部保留为 forensic 证据。

## 7. 事件、身份和重跑

`job events` 是持久化、可重复读取的事件日志，不是会被某个消费者取走的破坏性队列。

```text
job events
job events JOB_ID
job events --since CURSOR
job events JOB_ID --follow
```

事件至少包含单调事件序号、时间、job series、run ID、attempt、状态、宏步、模拟时间、粒子计数、wall time、CPU、RSS、warning、错误和输出位置。

多个终端、脚本或 AI 可以从各自 cursor 独立消费。监控只提供任务级指标，不加入逐粒子遥测；正式性能测试要求监控中位开销不超过 1%。

身份分为：

- `job_series_id`：同一个逻辑任务系列；
- `run_id`：每次实际运行的唯一 UUIDv7；
- `attempt`：系列内从 1 开始递增。

`job rerun` 在相同 series 下创建新 run ID 和 attempt。活动任务必须先完成安全取消。新 attempt 只有在 `Complete + result verify --full` 后，旧 attempt 才能被标记为未来可清理；旧 attempt 的摘要、状态、digest 和 `superseded_by` 永久保留。

M5 内不实现真实产物删除：rerun 后的清理判断和 `job prune` 都只能生成 dry-run 清单，列出候选路径、大小、理由和保护条件，不提供 `--apply`，也不调用文件删除。实际删除能力留待未来单独设计和用户裁决。`job forget` 只从 daemon 日常列表中移除终态记录，不删除项目目录中的审计摘要或任何运行产物。

## 8. 结果接口和退出码

`result inspect` 展示状态、身份、资料、粒子计数、终止分类、质量账本、资源和产物位置。

`result verify` 默认 quick，检查 manifest、文件 SHA、SQLite integrity、WAL、bundle 结构和身份；`--full` 再执行 lifecycle、质量、mass ledger、row count、digest 和轨迹顺序审计。

验证返回值表达“产物是否符合其声明状态”，不等同于运行是否成功。合法 cancelled 产物可以 verify=0，但 `run_success=false`；interrupted 或损坏产物返回非零。

`result trajectory` 按粒子 ID、多个 ID 或 `--all` 流式读取 birth、scheduled states、termination/end 的完整时间有序轨迹，不补造不存在的状态，也不偷偷插值。

每个 attempt 自动生成 `run-report.md`，记录命令、身份链、运行状态、资源、终止原因、验证摘要和 forensic 指针。

退出码：

- 前台 `run` 和 `job wait` 只有 Complete 返回 0；
- completed-with-errors、failed、cancelled、interrupted 返回 1；
- 参数、命令或格式错误返回 2；
- `run --detach` 在成功进入队列后返回 0；
- `result verify` 根据产物完整性返回，不根据 `run_success` 返回。

## 9. 实施阶段与 A/B 分工

### M5-A0：基线与权威合同冻结

由 A 完成：

- 形成独立的 M4 完成基线 commit/push；
- 以 `TRAJECTA_M5_A0_PRODUCT_CONTRACT.md` 作为 M5 控制面最高权威；
- 写入 M5 权威计划、命令合同、状态机、退出码和职责；
- 冻结 project/config/data-plan/job-event 机器合同；
- 冻结 manifest v1 的 cancelled/interrupted 修订；
- 冻结 Local `JobBackend` 接口；
- 增加自动合同 validator，拒绝状态机漂移、错误退出码和新增可达 `NotImplemented`。

开发期间所有 crate 保持 `0.0.0`。A0 通过后才生成第一份 B Prompt。

### M5-A1：配置、项目和 CLI 外壳

主要交给 B，A 复验：

- 完成 CLI 解析、三种输出格式和 help；
- 完成 config/project 渐进式编辑；
- 完成项目状态推导、data-plan、finalize 和模板快照/reapply；
- 完成 stage-aware validate 与 doctor 基础检查；
- scientific validation 必须调用现有 crate API，不在 CLI 重写规则。

### M5-A2：daemon、调度与恢复

由 A 主导：

- 实现本地 IPC、单实例 daemon、worker 子进程和持久任务库；
- 实现资源池、FIFO、安全回填、三次越过上限和内存压力暂停；
- 为 Runner 增加低开销控制接口，仅允许宏步边界取消、进度快照和 heartbeat；
- 实现 worker 重新关联、失联检测、interrupted forensic；
- 实现安全取消的 SQLite/provenance/manifest 终态化；
- 不改变积分、插值、边界、population 或气象查询结果。

### M5-A2E：A2 代码精简与结构收口

A2 功能和真实进程验收冻结后，由 A 单独执行一次不扩功能的工程收口：

- 盘点 A1/A2 新增生产代码，优先拆分过大的 runtime/catalog 文件；
- 删除重复实现、宽泛兜底、死分支、叙述性注释和只为未来预留的抽象；
- 复用现有 helper 与类型，收紧错误传播和模块职责，但不建立 `v2`、`fix2` 等平行版本；
- 不改变公开 CLI、job/schema、状态机、数值路径、容差、异常分类或磁盘产物语义；
- 精简后必须原样复跑 A2 的 Windows/WSL 进程合同、恢复/取消场景和 workspace 全量门禁。

### M5-A3：任务操作、验证和轨迹产品化

A 处理生命周期核心，B 处理冻结后的机械接口：

- 完成全部 job 命令；
- 完成 series/run/attempt/superseded 关系；
- 完成 result inspect/verify/trajectory；
- 完成自动报告、多消费者事件 cursor 和故障注入测试。

### M5-A4：跨平台产品矩阵和打包

B 执行固定脚本和矩阵，A 审计：

- Windows x64 与 Linux x86_64，Linux 最低 Ubuntu 22.04；
- 单一 binary 包含 Rust/native reader，默认 Rust；
- 构建 ZIP、tar、SHA-256、SBOM、许可证、build manifest 和最小示例；
- clean-extraction 环境运行 help、doctor、配置初始化、示例验证和小型 E2E。

### M5-A5：FLEXPART 对比与预发布

所有其它门禁通过后再执行 FLEXPART 科学和性能对比。最终一次性把版本从 `0.0.0` 改为 `0.1.0-alpha.1`。用户确认发布后创建 tag，由 GitHub Actions 生成 Prerelease；下载后的 clean-extraction smoke 通过，M5 才完成。

## 10. 测试与验收矩阵

### 10.1 合同与故障测试

必须覆盖：

- CLI 所有命令和三种输出格式；
- TOML/YAML 原子写、非法点路径、类型错误和并发修改；
- configured 缺资料、finalized 丢资料、lock 不一致；
- daemon 重启、daemon 崩溃、worker 崩溃和陈旧 lease；
- 排队任务、已完成任务和活动任务的重启行为；
- 安全取消、强制取消、取消时输出失败；
- 外部内存压力、worker OOM kill、磁盘写失败和 SQLite persist 失败；
- 队首回填与三次越过上限；
- 多个事件消费者和 cursor 恢复；
- rerun 不覆盖旧 RunID；
- rerun/prune 只能生成 dry-run 清单，并断言磁盘文件零删除；
- quick/full verify 与 run_success 分离；
- 监控中位开销不超过 1%。

### 10.2 Rust reader 正式产品矩阵

1,000 粒子完整矩阵：

```text
2 platforms x 3 data families x 3 populations x 2 directions = 36 cells
```

10,000 粒子代表矩阵每平台六格，共 12 格：

- ERA5 pressure：release forward、air-mass backward；
- ERA5 hybrid：air-mass forward、ozone backward；
- CFSR pressure：ozone forward、release backward。

所有格必须满足 Complete、abnormal=0、lifecycle/finite/mass/quality/population audit、SQLite integrity、terminal WAL、provenance、digest 和 daemon/job/report 身份门禁。

### 10.3 Native reader 矩阵

```text
2 platforms x 3 data families x 2 directions x 1k release = 12 cells
```

必须使用 release binary；native 不作为默认 backend。

### 10.4 FLEXPART 对比

冻结场景：同一台 WSL/Linux 机器、ERA5 hybrid、普通 forward release、1 小时模拟、10 分钟输出、10k/50k 粒子，每档预热一次并正式重复三次，使用相同 CPU 配额。

分开报告等价核心计时与 Trajecta 完整 SQLite/provenance 产品计时。如果 Trajecta 等价核心中位耗时超过 FLEXPART 的 1.5 倍，必须完成归因和合理优化；剩余差距只能由 A 与用户书面例外接受，不能通过删输出、降精度或改变物理合同修绿。

科学比较硬门包括输入资料、时间窗、输出覆盖、nonfinite、状态和质量账本。量化共同输出时刻的水平大圆距离、垂直差异、质心、扩散宽度、输送距离、空间占据和终止分布。

有可靠 particle crosswalk 时报告逐粒子连续轨迹差异；没有时明确降级为集合比较，不能伪装成逐粒子误差。

交付原始 JSON/CSV、四张性能图和 comparability matrix。矩阵逐项标记 `exact / aligned / aggregate-only / not-comparable` 并说明原因。

FLEXPART 只作为外部 GPL 参考测试工具，不链接或复制进 MIT 产品二进制，正式业务运行不依赖它。

## 11. 最终约束

- M4 必须先形成干净、可追溯的基线提交；
- 支持命令面不得存在可达 `NotImplemented`；
- 不通过修改数值容差、异常分类或减少输出修绿；
- M5 开发期间不制造 `v2`、`fix2` 等重复合同；新合同只建立一个 v1，现有未发布 v1 直接修订；
- 不调用子代理；
- B 只能由 A 写详细 Prompt 后，由用户在外部启动；
- 中高风险任务由 A 完成，冻结后的 CLI、脚本、打包和机械矩阵优先交 B；
- GitHub Prerelease 可下载并通过 clean-extraction smoke 后，才能宣称 M5 完成。
