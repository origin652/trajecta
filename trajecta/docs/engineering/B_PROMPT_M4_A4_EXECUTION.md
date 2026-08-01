# 给 B：M4-A4 平台、性能与终审机械执行包

你是 B 模型。本轮只承担 **A 已冻结合同下的中等及以下机械实现、执行、采集和报告**；所有
中难/高难修改、数值异常诊断、跨平台差异裁决、性能 baseline 接受以及 M4 最终签署仍由 A 完成。

## 0. A 已完成的冻结起点（2026-07-25）

本轮不再从“实现 A4 工具”开始。A 已完成并通过最终 WSL 预检的执行入口是：

- `crates/trajecta-core/tests/m4_a4_real_perf.rs`；
- `tools/run_m4_a4_real_matrix.py`；
- `tools/monitor_m4_a4_wsl_cell.py`。

A4.5 O1/O2 已把正式 particle-loop exact reuse 固定为 `19 logical / 13 executed / 6 reuse`，并把
boundary column/stencil cache 的重复构建压下；B 不得重做、替换或继续优化这些路径。最终源码的 WSL
preflight 已通过：

```text
artifact:
  target/m4-a4/cells/wsl__era5-hybrid__forward__p1000__w4/attempt-20/
cell summary:
  M4_A4_CELL_SUMMARY.json
cell summary SHA-256:
  263a04f41494196e85fec975fa4c9d7ea865088d111c710e08e8abd527379998
release binary SHA-256:
  c3bd42f3004f0bf25aa93be822a6ecb57ac53aac687dd42ff52d3c861fbd84c9
result:
  passed; runner 10,504 ms; peak RSS 122,011,648 bytes;
  79 writer-active SQLite snapshots; I/O delta 0; abnormal 0;
  lifecycle rows 6,701/6,701; WAL 0 bytes; finite audit 0 failures;
  normalized bundle/SQL/canonical identities independently recomputed.
```

B 开始时先重算上述 summary SHA 并确认 artifact 完整。只补写 `docs/engineering/` 报告不会使该 binary 证据失效；
若 `crates/`、`tools/`、Cargo manifests/lockfile 或冻结资料发生变化，停止正式矩阵并交 A，不能拿旧
preflight 给新执行代码背书。

当前没有 C 模型。A 不调用子代理；B 只能由用户通过本书面 Prompt 单独运行，也不得自行再委派代理。

开始前完整阅读：

- `docs/engineering/TRAJECTA_M4_PARTICLE_LOOP_PLAN.md`（尤其 12.2～12.4、13）；
- `docs/engineering/TRAJECTA_M4_MODEL_ASSIGNMENT.md`（尤其 M4-A4、报告格式、Commit 规则）；
- `docs/engineering/TRAJECTA_M4_A2_A_COMPLETION_REPORT.md`；
- `docs/engineering/TRAJECTA_M4_A3_A_COMPLETION_REPORT.md`；
- 本 Prompt。

从用户交给你的当前工作树继续，不清理、不 reset、不覆盖其他未提交改动。先记录：

```text
git status --short --branch
git rev-parse HEAD
git diff --stat
```

外层 `E:\flexpart\origo-validation-v1.json` 是用户文件，**不得读取、修改、移动、删除、加入 Git 或写入
报告内容**。本轮不得 commit、不得 push、不得创建 PR，不得宣称 M4-A4/M4 完成。

## 1. 绝对责任边界

### 1.1 可以做

- 新增 A4 专用的显式 ignored 真资料测试/执行 harness；
- 新增或修补低风险 Python/WSL 编排、SQLite 只读检查器、artifact 索引器和结构化比较器；
- 增加**纯观测**计数器或测试接口，用于 wall time、RSS、I/O、批量查询次数和 digest 采集；
- 运行 Windows/WSL 门禁和冻结矩阵；
- 生成机器 JSON、日志、SHA、最小复现和 B 执行报告；
- 发现失败后缩小复现，但不得通过改变合同来“修绿”。

### 1.2 不可以做

- 不得修改 RK2、PV、W、boundary、surface-layer、domain-fill 质量守恒、垂直坐标、气象插值、
  release/ozone 科学规则等数值核心；
- 不得修改任何 algorithm ID、科学公式、容差、normal/abnormal 分类或 hard-gate 语义；
- 不得修改公共 Case、manifest、SQLite、provenance bundle 的 schema/version 来迁就本轮结果；
  新测量优先写入 `target/m4-a4/` 独立 JSON；
- 不得增加近似缓存、舍入键、抽样比较或静默 fallback；批量查询键必须按 exact 值度量；
- 不得自行优化核心来解决性能失败；不得自行裁决 Windows/WSL/native 数值差异；
- 不得降低正式粒子数、缩短 1 小时场景、减少输出事件后宣称正式通过；
- 不得把未运行、skip、历史 artifact、权限失败或 external block 写成 passed；
- 不得删除失败产物后只保留重跑成功结果。

若完成观测所需的改动会进入上述禁区：停止该项实现，保留最小复现，在报告中标
`needs_a_adjudication`，继续执行仍可安全完成的其他项。

## 2. 冻结正式场景与矩阵

唯一正式长测场景：

| 项 | 冻结值 |
|---|---|
| platform | WSL Ubuntu-24.04 |
| family/profile | ERA5 hybrid 137 层 / `era5-cds-hybrid137-v0` |
| population | air-mass domain-fill |
| duration | 3,600 s |
| maximum numerical step | 600 s |
| expected numerical steps | 6 |
| output | SQLite particle state，每 600 s；起点和终点也保留 |
| directions | forward、backward |
| random seed | 沿用冻结 A2 air-mass seed，不另换 seed |
| reader | Rust production reader |
| memory budget | 保持生产合同；不得为通过 RSS 门禁伪降缓存 |

正式 6-cell 矩阵：

| cell | particles | workers | direction | 用途 |
|---|---:|---:|---|---|
| S50-F | 50,000 | 4 | forward | 生产 runner scaling |
| S100-F | 100,000 | 4 | forward | scaling、RSS、SQLite、baseline candidate |
| S50-B | 50,000 | 4 | backward | 生产 runner scaling |
| S100-B | 100,000 | 4 | backward | scaling、RSS、SQLite、baseline candidate |
| D100-F | 100,000 | 1 | forward | 与 S100-F 做规范轨迹确定性比较 |
| D100-B | 100,000 | 1 | backward | 与 S100-B 做规范轨迹确定性比较 |

`50k -> 100k` 比例只比较同方向、同 4-worker runner、同 binary、同场景的 run-only wall time。
不得把 build/compile 时间混入粒子缩放比例。1-worker/4-worker 不比较速度，只比较科学输出身份。

在正式矩阵前允许且必须做一个 WSL 1,000-particle forward/4-worker preflight，用于验证工具和 JSON；
它必须标 `preflight`，不得计入正式 6-cell 通过数。

## 3. P0：先审计 A 已冻结的证据工具，不重写它

实际冻结文件名如下：

- `crates/trajecta-core/tests/m4_a4_real_perf.rs`：A4 ignored 真资料入口；
- `tools/run_m4_a4_real_matrix.py`：attempt-preserving、`--resume`、cell/aggregate hard-gate 编排器；
- `tools/monitor_m4_a4_wsl_cell.py`：独立 WSL read-only/query-only SQLite writer-active 监控器；
- `docs/engineering/TRAJECTA_M4_A4_B_EXECUTION_REPORT.md`：B 执行报告。

编排器已经直接运行 release test binary，并生成 `command.json`、`binary-identity.json`、GNU time、
并发读取证据、`M4_A4_CELL_SUMMARY.json` 和 phase summary。Rust harness 已用流式 bundle validator、真实
SQLite canonical SQL inspection 和磁盘 SHA 独立重算三类 normalized identity。B 只审计和执行；不要
另建功能重复的 runner、monitor 或 digest 实现。

不要把 A4 参数硬塞回 A2/A3 的冻结语义。可以抽取无行为变化的共享 helper，但 A2/A3 既有测试入口和
summary schema 必须保持兼容。

### 3.1 A4 ignored test 参数

至少支持并严格校验以下环境变量：

```text
TRAJECTA_M4_A4_PARTICLES
TRAJECTA_M4_A4_DIRECTION            # forward | backward
TRAJECTA_M4_A4_WORKERS              # 1 | 4（正式矩阵）
TRAJECTA_M4_A4_DURATION_SECONDS     # 正式必须 3600
TRAJECTA_M4_A4_TIME_STEP_SECONDS    # 正式必须 600
TRAJECTA_M4_A4_OUTPUT_INTERVAL_SECONDS # 正式必须 600
TRAJECTA_M4_A4_ARTIFACT_DIR
TRAJECTA_REQUIRE_REAL_MET=1
```

正式模式必须 hard assert：资料族/profile、粒子数、方向、worker、duration、step、output interval 均与
本 Prompt cell 一致。缺资料、空选择、错误 profile、错误时窗或不足 frame coverage 都是明确失败，
不是 skip。

输出 schedule 使用 `OutputSchedule::Interval { interval: 600 s, ... }`，不得继续用 A2 的
`Endpoints` 冒充。100,000 粒子应有 7 个 output events；`particle_state` 的上限约为 700,000 行，
但正常终止粒子在终止事件写完最后一行后不得重复写。hard gate 必须逐粒子证明：event 0 到终止
event（含）或 event 6 连续覆盖、终止行与 `termination` 一致、未终止粒子 event 6 为 alive，且实际
总行数精确等于上述 lifecycle 推导值。另逐表记录 `run`、`particle`、`particle_mass`、`termination`
的真实行数。

### 3.2 计时和 RSS

编译与执行分离：

1. 先 `cargo test --offline --release ... --no-run`；
2. 记录最终 test binary 的绝对路径、size、SHA-256；
3. 每个 cell 在新的直接进程中执行该 binary；不得把 `cargo`、编译器或链接器 RSS 当 runner RSS；
4. WSL 用 `/proc/self/status` 的 `VmHWM` 或 direct-process `/usr/bin/time -v` 采集 peak RSS，记录方法；
5. harness 用 monotonic clock 至少分开记录：lock/build/preload、`runner.run()`（含 terminal finalize）、
   process total；
6. scaling 和 baseline 统一使用 `runner.run()` 含终态 finalize 的 wall time；所有原始值同时保留。

RSS 是进程级累计峰值，包含生产 runner 的预加载与执行，正式 100k 每格必须 `<= 2 GiB`。若不同采集
方法不一致，同时记录但不得挑较小值修绿；门禁采用 direct test process 的可复现主方法。

### 3.3 真实 reader/provider I/O 计数

必须使用生产 `IoCallCounters`，不得用 mock 或只看 column-cache miss：

1. 在 dataset inspect/lock/build/FrameLoader 前安装同一组 counters；
2. `build_runner(...)` 完成、所有正式 frames 已进入 MetEngine 后取 `after_preload` snapshot；
3. `runner.run()` 终态完成后取 `after_run` snapshot；
4. 逐项写出 `inspect`、`build_index`、`decode`、`provider_frame_load`、
   `format_detection_open_attempt`；
5. hard assert `after_run - after_preload` 五项全部为 0；
6. 用 RAII/明确 cleanup 保证成功和失败路径都 clear active counters。

不得只在一个合成单测证明 delta=0；正式 6 个真实 cell 每格都要有实际 snapshot。

### 3.4 批量气象查询计数与 exact reuse

增加纯观测 instrumentation，区分：

- logical batch-query requests；
- 实际执行的 batch queries；
- exact-key reuse；
- unique exact keys。

exact key 至少冻结：物理时刻、domain、实际 pinned frame window 身份与权重、完整 transport plan、
vertical coordinate、按稳定粒子顺序排列的经纬高 IEEE-754 bit pattern（不得十进制舍入），以及会
改变结果的其他请求身份。报告中写出 key 公式和 SHA 算法。只允许两种复用：完整 exact batch，或
因正常终止过滤产生的 exact ordered subsequence；后者的每一行必须按三坐标 bit pattern 从原 batch
保持顺序选出，输出值、validity、quality、provenance、status、bounds 和 explain 均原样选择。

计数必须按 caller origin 分开。`Integrator + Output` 是冻结的 vectorized particle-loop bulk gate；
`Boundary`、`PopulationVertical` 和 `Unclassified` 必须另行完整报告，不能藏入 bulk gate，也不能把其
内部 scalar/certificate 查询误算进 13。正式六步场景要求：

- particle-loop 为精确 `19 logical / 13 executed / 6 exact reuse`；
- particle-loop unique executed exact keys 精确为 13，repeated exact executions 为 0；
- 每个非终态输出与下一步 RK2 首查询若为完整 exact batch 或 exact ordered subsequence，必须命中
  复用，不得二次执行；
- 不得为了达到 13 修改键、删除字段、抽样粒子或把近似请求视作相同；
- instrumentation 只观测现状。若当前生产 runner 实际重复执行，记录 raw/unique/reuse 和首个重复键，
  将 cell 标 failed，交 A 修改；B 不实现数值路径缓存。

### 3.5 SQLite 并发读取与终态审计

每个 100k 正式 cell 在 writer 运行期间启动独立只读检查进程：

- SQLite 出现后使用 read-only/query-only 连接轮询；
- 每次在一个读事务中读取 schema version、event count、particle count、state count；
- 记录 attempts、successful snapshots、busy/locked/other errors 和时间；
- 至少取得 3 个 writer-active 成功快照；不得把终态后的读取算作并发成功；
- 检查器不得修改 PRAGMA、checkpoint 或数据库内容。

终态必须检查：

- manifest `complete`、`RunOutcome::Complete`、`abnormal_count == 0`；
- `PRAGMA integrity_check` 精确为 `ok`；
- WAL 已按生产生命周期 checkpoint/truncate，非空 `-wal` 为失败；
- 主 `particles.sqlite` 大小 `<= 512 MiB`，同时记录 `-wal`、`-shm`、bundle 和整个 run dir size；
- 7 output events；`particle_state` 行数按逐粒子 lifecycle 精确推导（100k cell 上限约 700,000）；
- 全部状态/质量/核心气象列 finite；
- mass ledger 每步及最终均在冻结 tolerance 内；
- manifest、provenance bundle、SQLite 互相引用的 SHA/identity 可从磁盘重算成立。

### 3.6 单 cell 机器报告

每次尝试写独立、不覆盖的：

```text
target/m4-a4/cells/<cell-id>/attempt-<n>/M4_A4_CELL_SUMMARY.json
target/m4-a4/cells/<cell-id>/attempt-<n>/stdout.log
target/m4-a4/cells/<cell-id>/attempt-<n>/stderr.log
target/m4-a4/cells/<cell-id>/attempt-<n>/command.json
```

单 cell summary 至少包含：

- schema version、cell id、formal/preflight、attempt；
- git HEAD、dirty paths（只列路径，不读取外层用户文件）、binary SHA；
- OS/WSL distro/kernel/CPU/RAM、rustc/cargo、compiler、SQLite、netCDF、HDF5、ecCodes、libclang
  可获得版本；不可获得写 `null + diagnostic`，不得编造；
- 全部冻结参数和 input identity/content SHA；
- exact command、白名单环境变量、start/end UTC、exit code；
- 三段 wall time、peak RSS 与方法；
- status/outcome/abnormal/steps/particles/row counts/mass ledger；
- I/O snapshots/delta；query request/executed/reuse/unique；
- SQLite 并发读、integrity、sizes；
- manifest/bundle/SQLite/canonical content/canonical SQL/canonical output SHA；
- checks 字典和 failures 数组。

只有所有 hard checks 都通过才可写 `status=passed`。测试进程 exit 0 但审计失败仍是 failed。

## 4. P0 preflight

A 已在 `attempt-20` 实跑 WSL：ERA5 hybrid、forward、1,000 particles、4 workers、
3,600 s/600 s/600 s。B 先审计该 passed evidence；执行源码和资料身份未变时不要浪费时间重复运行。
若执行源码或资料变化，则必须重新跑同一 preflight，且新 attempt 通过后才可继续正式矩阵。

preflight 必须实际证明：

- release binary 直接执行；
- interval output 为 7 events，state rows 与逐粒子 lifecycle 推导值精确一致（上限 7,000）；
- I/O delta=0；
- query 计数 JSON 非空且公式可审计；
- SQLite 并发 reader 至少 1 次 writer-active 成功（正式 100k 要求 3 次）；
- summary、日志、SHA、artifact index 能从零生成。

preflight 任一证据工具错误时先修工具并重跑；不得在工具尚不可信时开始 100k。

## 5. P0 正式 6-cell 长测

在未改执行源码/资料的前提下，从仓库根目录执行唯一正式入口：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4 `
  --resume
```

不要手工逐格拼环境变量。runner 已冻结顺序、1/4-worker、50k/100k 参数、release direct binary、
GNU-time RSS、50k 至少 1 个/100k 至少 3 个 writer-active SQLite snapshot、终态 WAL/size/finite/lifecycle/
identity 审计、两方向 scaling 和 1/4-worker normalized digest aggregate gate。默认首个 cell failed 即停并
返回 A；只有 A 明确要求补齐失败面时才使用 `--continue-on-fail`。

按 `S50-F -> S100-F -> S50-B -> S100-B -> D100-F -> D100-B` 顺序执行。每格开始前只清理该格
新的 attempt staging，不删除既有 attempt。编排器必须支持 `--resume`：只跳过**完整审计 passed** 的
cell；跳过时在总 summary 中引用原 artifact SHA，不得只凭目录存在。

环境/权限类失败最多可原参数重试一次，两个 attempt 都保留。科学、数值、RSS、SQLite、I/O、query、
digest 或 abnormal 失败不得自动改参数重试修绿。

正式判据：

| gate | 判据 |
|---|---|
| completion | 6/6 `Complete`，manifest complete，abnormal=0 |
| scale forward | `S100-F.run_wall / S50-F.run_wall <= 2.4` |
| scale backward | `S100-B.run_wall / S50-B.run_wall <= 2.4` |
| RSS | 四个 100k cell 均 `<= 2 GiB` |
| SQLite | 四个 100k cell 主 DB 均 `<= 512 MiB`，integrity ok |
| execution I/O | 6/6 after-preload delta 全 0 |
| query | 6/6 particle-loop 均为 19 logical / 13 executed / 6 reuse，unique=13、repeated=0；其他 origin 单列 |
| determinism F | S100-F 与 D100-F normalized content、canonical SQL/output 相同 |
| determinism B | S100-B 与 D100-B normalized content、canonical SQL/output 相同 |
| mass/finite/coverage | 6/6 hard gate 全通过 |

exact SQLite SHA 和 exact provenance bundle SHA 可因 run_id/物理布局不同而不同，不得错误要求相同；
必须比较从真实磁盘独立重算的 normalized content、canonical SQL、canonical output。1/4-worker 任一逻辑
主键缺失、状态/终止不同或规范 digest 不同都标 failed，不得自行设容差。

## 6. Baseline candidate 与 25% 回退规则

B 只生成：

```text
target/m4-a4/baseline/M4_A4_WSL_BASELINE_CANDIDATE.json
```

内容绑定 WSL runner identity、CPU/RAM、kernel、binary SHA、git HEAD、正式参数、S100-F/S100-B 的
run wall time 与 cell summary SHA。

- 若仓库中没有 A 已签署的 accepted baseline：写 `candidate_only`，不得写 regression passed；
- 若存在 accepted baseline：只有 runner identity、参数、binary contract 和测量方法全部相同才计算
  `(current/baseline - 1)`；`>25%` 标 failed；身份不同标 not-comparable，交 A；
- B 不得自行把 candidate 改名为 accepted baseline，不得把 baseline 文件提交进仓库。

## 7. Windows/WSL/native 最终证据整理与跨平台测量

### 7.1 可复用证据

A3 书面允许复用其 1,000-particle Rust/native 6-pair evidence。A2 的 Windows/WSL air-mass 12-cell
功能证据也可进入 A4 artifact index。复用前必须：

- 验证报告所列 JSON 和关键 artifact 仍存在；
- 重算 size/SHA；
- 标 `reused_from_a2` / `reused_from_a3`，不得写成本轮 executed；
- 校验 input identity、algorithm IDs、row coverage 和报告引用一致；
- 缺失或 SHA 不一致标 blocker，不得伪造替代 artifact。

### 7.2 Windows/WSL 结构化差异

对 A2 三族双方向的 Windows/WSL SQLite 做逻辑主键对齐；对 A3 ozone 三族双方向同样对齐。至少比较：

- `particle`、`particle_mass`、`output_event`、`particle_state`、`termination` coverage；
- status、termination reason、validity、quality；
- longitude/latitude/height、U/V/W、pressure、temperature；
- dry-air carrier mass、ozone mass（适用时）；
- 每字段 exact mismatch count、max abs、max rel、max ULP、worst logical key；
- nonfinite、value-presence mismatch、Windows-only/WSL-only keys。

比较前必须证明两边 family/direction/particle count/input content identity/算法身份可比。不得截短到较短一侧，
不得抽样，vector/status 不得拆开后忽略失败。结果只写 `measured`、`not_comparable` 或 `blocked`；
**B 不得给跨平台差异下 passed/failed 科学裁决，也不得改容差。**

输出：

```text
target/m4-a4/platform-diff/M4_A4_WINDOWS_WSL_DIFF.json
target/m4-a4/native-review/M4_A4_NATIVE_EVIDENCE_REVIEW.json
```

## 8. Windows 与 WSL 完整门禁

在不运行 ignored 长测的普通 workspace 门禁中，Windows 和 WSL 都实际执行并保存完整日志：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/validate_m4_a0_contracts.py tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

若实际文件名不同，py_compile 使用真实新脚本列表。另单独编译并运行 A4 preflight/正式 ignored test；
不得把普通 `cargo test` 中 ignored 计数写成 A4 executed。

WSL 使用仓库冻结的 Ubuntu-24.04，设置独立 `CARGO_TARGET_DIR`，使用离线依赖。若沙箱/挂载权限失败，
保留原始错误并按用户已有授权方式在 WSL 正常环境重跑；只有确实缺外部工具/资料且无法在本轮解决时才写
`external_blocked`。

## 9. Artifact index、总汇总与报告

生成：

```text
target/m4-a4/M4_A4_ARTIFACT_INDEX.json
target/m4-a4/summary/M4_A4_B_FINAL_SUMMARY.json
docs/engineering/TRAJECTA_M4_A4_B_EXECUTION_REPORT.md
```

artifact index 对每个本轮产物和复用产物记录：相对路径、类型、produced/reused、cell/phase、size、
SHA-256、生成命令或来源报告。不得索引临时编译缓存和用户外层文件。

总 summary 必须有四态字段：

```text
implemented
executed
passed
blocked
```

并单列：

- preflight；
- formal 6-cell；
- 每条性能 gate；
- 1/4-worker digest；
- query/I/O；
- SQLite concurrent/integrity/size；
- Windows/WSL ordinary gates；
- reused A2/A3/native evidence；
- platform diff measurement；
- baseline candidate；
- failures、external blockers、needs A adjudication、未运行项。

B 报告至少包含：

1. 当前 HEAD/dirty 状态和未提交声明；
2. 精确执行命令、exit code、开始/结束时间；
3. 6-cell 参数与结果表；
4. 50k/100k 两方向比例计算原值；
5. 100k RSS、SQLite size、row counts、并发读表；
6. 每格 I/O before/after/delta 与 query raw/unique/reuse；
7. 1-worker/4-worker 三类 normalized digest；
8. Windows/WSL 每字段差异摘要，但不作科学裁决；
9. A2/A3/native 复用 artifact 与重算 SHA；
10. baseline candidate 身份；
11. 所有失败最小复现、artifact 链和建议 A 调查入口；
12. OS、Rust、compiler、SQLite、netCDF、HDF5、ecCodes、libclang 版本；
13. 明确声明：B 未改数值核心、未改容差/schema/算法 ID、未 commit/push、未触碰
    `origo-validation-v1.json`、不宣称 M4-A4/M4 完成；
14. 明确声明：后续中难/高难修复、差异裁决、baseline 接受和最终签署由 A 完成；A 不调用子代理，
    B 仅由用户通过本书面 Prompt 单独使用。

## 10. 失败保真规则

首个正式失败出现时，立即复制/冻结到：

```text
target/m4-a4/first-failure/<cell-id>/
```

至少保留 command/env、stdout/stderr、cell summary、manifest（若有）、SQLite/bundle（若合法存在）、
版本身份和 SHA。之后可继续跑不依赖该失败的 cell，以得到完整失败面；不得因 1 个失败把其余全部写成
passed 或 external_blocked。

以下情况直接交 A，不修绿：

- abnormal termination、质量账本、finite、row coverage 失败；
- reader/provider I/O delta 非零；
- particle-loop 不是 19/13/6、unique!=13、相同 exact key 被重复执行，或其他 origin 未完整披露；
- RSS/SQLite/scaling 超门禁；
- 1/4-worker normalized digest 不同；
- Windows/WSL/native 状态、终止、coverage 或数值出现需科学解释的差异；
- 为修复必须进入 1.2 禁区。

## 11. 本轮完成定义

B 的工作只有在以下条件同时满足时才叫“执行包交付”，仍不叫 M4-A4 完成：

- P0 工具经 preflight 证明可信；
- 正式 6-cell 全部实际运行，或每个未运行 cell 有真实 external blocker；
- 所有机器 JSON、日志、SQLite/bundle、SHA、artifact index 可追溯；
- Windows/WSL ordinary gates 实际运行；
- A2/A3/native 复用证据经 SHA 审计；
- 跨平台差异完成结构化测量但未越权裁决；
- 报告诚实列出 passed/failed/blocked/未运行；
- 未 commit、未 push、不宣称 M4-A4/M4 完成。

最后只向 A 汇报：执行包路径、6-cell 结果、性能门禁、差异测量、首个失败（若有）和需要 A 裁决项。
不要请求自行发布版本，也不要代 A 修改阶段状态。
