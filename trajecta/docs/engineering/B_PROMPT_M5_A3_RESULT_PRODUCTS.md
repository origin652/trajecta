# B Prompt — M5-A3 result inspect / trajectory / run-report

你是 B。只完成本文件冻结的机械读取、流式渲染与测试，不裁决 A 已完成的生命周期、digest、supersession 或 prune 核心。

## 0. 工作方式与禁止项

- 不使用子代理。
- 不 commit，不 push，不改版本号；所有 crate 继续保持 `0.0.0`。
- 不触碰仓库外层 `../origo-validation-v1.json`。
- 不改数值核心、积分、插值、边界、population、气象查询、容差或异常分类。
- 不改下列 A-owned 语义：
  - `trajecta-core/src/verification.rs` 的 quick/full verifier；
  - `trajecta-job` 的 catalog/history/IPC/daemon、attempt、full verification、supersession；
  - `job rerun`、`job forget`、`job prune` eligibility；
  - provenance、SQLite canonical digest、canonical-output digest；
  - 任何 manifest/SQLite/provenance/job schema。
- 不增加真实删除、`--apply`、自动清理、v2/fix2/legacy-copy 等平行版本。
- 不以宽泛 fallback、吞错、`unwrap_or_default` 或复制 A 的 validator 来“修绿”。
- 开始时记录 `git rev-parse HEAD`、`git status --short`、`git diff --stat`。当前工作树包含 A 未提交的 A3 核心，必须保留，不得回退或覆盖。

允许主要修改：

- 新建 `crates/trajecta-cli/src/result_products.rs`（名称可更短，但只能有一个正式实现）；
- `crates/trajecta-cli/src/lib.rs` 的模块接线；
- `crates/trajecta-cli/src/runtime.rs` 中 `result inspect/trajectory` 的 dispatch，以及为复用 A 的 `resolve_result` 所需的最小可见性调整；
- CLI 测试、M5 schema/example/validator；
- 本轮 B 报告。

`run-report.md` 本轮只交付纯读取、确定性 renderer 与原子 writer。不要自行把它接进 force-cancel、daemon recovery 或 catalog terminal transition；自动生命周期 hook 由 A 在 B 返回后接线，避免 B 改状态机。

## 1. 当前 A 基线（不得回改）

A 已完成并验证：

- rerun 保持 `job_series_id`，创建新 `run_id` 与递增 attempt；
- active attempt 必须先安全取消；forget 只隐藏，不删除；prune 永远 dry-run；
- Complete attempt 通过 `result verify --full` 后才记录 full-verification digest，并 supersede 较旧 attempt；
- `result verify` 已支持 run directory、`run-manifest.json`、series UUID、run UUID；
- quick/full 已严格检查身份、manifest、SQLite/WAL、bundle/digest，以及 full lifecycle/finite/quality/termination/mass；
- 合法 Cancelled 可 verify=0 但 `run_success=false`；Failed/Interrupted 不可伪装为正式产品；
- 真实 current-production 1k forward 与 50k backward 非零步产物已通过 full verifier；
- trajectory parser 已冻结：

```text
trajecta result trajectory RESULT --particle-id ID [--particle-id ID ...]
trajecta result trajectory RESULT --all
```

两种 selection 互斥且必须选一种；显式 ID 已排序、拒绝重复、拒绝超过 SQLite `i64` 上限。不要改 parser 语义。

## 2. P0-1：`result inspect`

实现：

```text
trajecta result inspect RESULT
```

`RESULT` 的解析必须复用 A 的 `resolve_result`：目录、`run-manifest.json`、series UUID、run UUID 均可。ID 路径下必须复用 catalog exact attempt，并核对 manifest 的 `job_series_id/run_id/attempt/status` 与 catalog 一致；不得另写一套 UUID/path resolver。

### 2.1 机器数据 shape

成功数据必须是下列稳定 shape，schema identity 固定为 `trajecta.result-inspection/v1`：

```json
{
  "schema_version": "trajecta.result-inspection/v1",
  "result_path": "<canonical native absolute run directory>",
  "identity": {
    "job_series_id": "<uuidv7>",
    "run_id": "<uuidv7>",
    "attempt": 1,
    "case_name": "case"
  },
  "lifecycle": {
    "status": "complete",
    "run_success": true,
    "started_at": {"seconds_since_unix_epoch": 0, "nanosecond": 0},
    "finished_at": {"seconds_since_unix_epoch": 1, "nanosecond": 0},
    "failure": null
  },
  "software": {},
  "inputs": {},
  "execution": {},
  "numerical": {},
  "particles": {
    "particle_count": 0,
    "particle_mass_count": 0,
    "state_count": 0,
    "output_event_count": 0,
    "termination_count": 0,
    "normal_termination_count": 0,
    "abnormal_termination_count": 0,
    "termination_by_reason": {}
  },
  "quality": {
    "wind": [],
    "pressure": [],
    "temperature": []
  },
  "mass_ledger": {
    "record_count": 0,
    "maximum_absolute_imbalance_kg": 0.0,
    "maximum_tolerance_fraction": 0.0
  },
  "artifacts": [],
  "catalog": null
}
```

规则：

- `software/inputs/execution/numerical` 直接使用 typed `RunManifest` 字段，不重新解释。
- `failure`、`finished_at`、`catalog` 使用 JSON `null`，不要根据分支删除 key。
- `run_success` 仅在 manifest status 为 `complete` 时为 true。
- particle/termination 主计数优先来自 manifest + SQLite public row counts；若二者均存在但冲突，返回 `result.inspect_identity_mismatch`，不要静默选一个。
- `quality.{wind,pressure,temperature}` 每项为按 `(validity, quality)` 排序的 bucket：

```json
{"validity":"missing","quality":"derived","count":109}
```

  `quality` 可为 null；只统计，不裁决合法性。合法性属于 A 的 full verifier。
- `maximum_tolerance_fraction` 为每条 ledger 的 `abs(imbalance_kg)/tolerance_kg` 最大值；`0/0` 视为 0，非零/0 必须报 manifest 错误而不是产生 infinity。
- artifact 项 shape 固定：

```json
{
  "role": "manifest|sqlite|sqlite_wal|provenance|run_report|forensic",
  "relative_path": "run-manifest.json",
  "exists": true,
  "size_bytes": 123,
  "declared_sha256": null
}
```

- artifact 只列 run directory 内的规范相对路径；拒绝 absolute、`.`、`..`、symlink escape。
- manifest、SQLite、WAL、声明的 provenance、现有 `run-report.md` 必须列出。直接子目录中匹配 `provenance-bundle.json.forensic-aborted*` 的文件作为 sorted forensic 项列出，不递归、不跟随 symlink。
- inspect 不重新 hash 大文件：`declared_sha256` 只复制 manifest 已声明的 SHA；manifest/report/WAL/forensic 没有声明 SHA 时为 null。
- catalog ID 查询时，`catalog` shape 固定：

```json
{
  "visible_in_routine_list": true,
  "full_verification": null,
  "superseded_by": null
}
```

  `full_verification` 存在时直接序列化 A 的 record；不要重算或伪造。

### 2.2 partial/forensic inspect

- strict manifest 解析失败：命令失败，code=`result.manifest_invalid`。
- Complete / CompletedWithParticleErrors / Cancelled 声明的 SQLite 或 provenance 缺失：命令失败，不得把正式产品显示为健康。
- Running / Failed / Interrupted 可以没有 provenance；仍应 inspect 成功并 `run_success=false`。
- SQLite 存在但只读查询失败时，保留 manifest/artifact inspection，并附一个 warning diagnostic `result.inspect_sqlite_unavailable`；particle/quality SQL-derived 字段为 null。只允许这一条明确的 partial 分支，不得用 `Err(_)` 吞掉其它错误。
- inspect 只读，不 checkpoint、不建索引、不修改 WAL、不写 report。
- 成功退出码 0，与 `run_success` 分离；artifact/identity/manifest fatal error 为 1；usage 仍为 2。

### 2.3 三种输出

- JSON：标准 `trajecta.cli-output/v1` 单对象，command=`result inspect`。
- JSONL：一个 data item + 唯一 summary；sequence 从 1。
- human：真正的短摘要，不要打印 pretty JSON。至少显示 identity/status、particles/terminations、resources、verification、artifact path；warning 单独显示。

## 3. P0-2：`result trajectory` 流式产品

### 3.1 读取与顺序

- 复用 `resolve_result` 与相同 catalog identity check。
- 使用 `ParticleStateSqliteSink::open_readonly`；不得打开读写连接，不得 checkpoint 或创建持久索引。
- 显式 IDs 在输出任何字节前检查全部存在；缺失时一次返回 sorted missing IDs，code=`result.particle_not_found`，不得输出半条流。
- `--all` 使用 SQLite `ORDER BY particle_id ASC, sample_sequence ASC` 流式读取；禁止 `Vec` 全量粒子/状态。
- 每粒子先输出一个 particle record，再输出该粒子的 state records。
- 不补造状态、不插值、不合并 birth/termination、不按全局 event time 重排。
- 全局 `event_sequence` 是稳定插入序，动态 birth/termination cohort 可在物理时间上交错；只保持数据库序号。每粒子的 `sample_sequence` 和物理轨迹顺序已由 A full verifier裁决，B 不复制 verifier。

### 3.2 record shape

所有 inner record 使用 schema `trajecta.trajectory-record/v1`。

Particle record：

```json
{
  "schema_version": "trajecta.trajectory-record/v1",
  "record_kind": "particle",
  "job_series_id": "<uuidv7>",
  "run_id": "<uuidv7>",
  "attempt": 1,
  "particle_id": 42,
  "population_id": "release",
  "origin": {
    "kind": "release|domain_initial|domain_boundary",
    "event_id": null,
    "domain_id": null,
    "boundary_face_id": null
  },
  "birth_time": {"seconds_since_unix_epoch": 0, "nanosecond": 0},
  "dry_air_mass_kg": 1.0,
  "sensitivity_weight": null,
  "substance_mass_kg": {}
}
```

State record：

```json
{
  "schema_version": "trajecta.trajectory-record/v1",
  "record_kind": "state",
  "job_series_id": "<uuidv7>",
  "run_id": "<uuidv7>",
  "attempt": 1,
  "particle_id": 42,
  "sample_sequence": 0,
  "event_sequence": 0,
  "event_kind": "birth",
  "time": {"seconds_since_unix_epoch": 0, "nanosecond": 0},
  "integration_offset_ns": 0,
  "elapsed_age_ns": 0,
  "position": {
    "longitude_degrees": 0.0,
    "latitude_degrees": 0.0,
    "height_asl_m": 100.0
  },
  "particle_status": "alive|terminated",
  "termination": null,
  "meteorology": {
    "eastward_wind_m_s": null,
    "northward_wind_m_s": null,
    "geometric_vertical_velocity_m_s": null,
    "air_pressure_pa": null,
    "air_temperature_k": null,
    "wind_validity": "missing",
    "wind_quality": null,
    "pressure_validity": "missing",
    "pressure_quality": null,
    "temperature_validity": "missing",
    "temperature_quality": null
  },
  "provenance_id": null
}
```

terminal state 的 `termination` 必须 LEFT JOIN public termination row：

```json
{
  "reason": "population_outflow",
  "classification": "normal",
  "time": {"seconds_since_unix_epoch": 0, "nanosecond": 0},
  "intersection_fraction": null
}
```

不得用 `termination_reason` 猜 classification 或 fraction。

### 3.3 输出与内存

- JSONL：先输出 result header data item，再输出 particle/state records，最后唯一 summary。外层仍是 `trajecta.cli-stream-item/v1`，sequence 从 1 连续。
- header data：

```json
{
  "schema_version": "trajecta.trajectory-stream/v1",
  "job_series_id": "<uuidv7>",
  "run_id": "<uuidv7>",
  "attempt": 1,
  "status": "complete",
  "run_success": true,
  "selection": {"mode":"all|particle_ids","particle_ids":[]}
}
```

- human：一行 result header；每粒子一行 immutable identity；每 state 一行时间/位置/status。不得 pretty-print JSON。
- JSON：必须是完整单一 `trajecta.cli-output/v1` 对象，data 为 header 字段加 `records` array。禁止把全部 records 收入 Vec。
- 为同时满足“完整 JSON”与 bounded memory，JSON mode 先把 records 以流式 serializer 写入 OS temp 下 create-new 的临时 spool，成功后再流式复制进最终 envelope；失败清理 spool，stdout 在成功前不得写半个 JSON。不要写入 result directory。
- JSONL/human 直接流式；意外断流不伪造 summary。
- stdout write error、SQLite read error、temp spool error必须非零退出，不得继续输出成功 summary。

## 4. P0-3：确定性 `run-report.md` renderer/writer

新增单一正式 API，例如：

```rust
render_run_report(inspection: &ResultInspection) -> Result<String, ...>
write_run_report_atomic(run_directory: &Path, inspection: &ResultInspection) -> Result<PathBuf, ...>
```

本轮不要接 daemon lifecycle hook。

规则：

- 文件固定名 `run-report.md`，UTF-8、LF、末尾一个 newline。
- 不写“generated_at=now”；相同 inspection 两次渲染必须 byte-for-byte 相同。
- 不加入 report 自身 SHA，不加入随机 nonce，不把临时绝对路径写入正文。
- section 顺序固定：
  1. `# Trajecta run report`
  2. Identity
  3. Lifecycle
  4. Inputs and software
  5. Execution resources
  6. Particles and terminations
  7. Quality summary
  8. Mass ledger
  9. Verification and supersession
  10. Artifacts and forensic pointers
- 正文必须声明：`run-report.md is a derived human-readable view and is not part of the scientific digest identity.`
- failure message 按原文 Markdown-safe 处理；不得执行或解释其中内容。
- artifact 只写相对路径、存在性和 size；不重新 hash。
- 原子写：same-directory create-new temp、write/flush/sync、atomic rename/replace；失败时旧 report bytes 不变，temp 被清理。
- writer 不修改 manifest/provenance/SQLite/catalog；report 不进入 canonical digest。

## 5. P0-4：schema/example/validator

新增并验证 Draft 2020-12：

- `testdata/M5_RESULT_INSPECTION.schema.json`
- `testdata/M5_RESULT_INSPECTION.example.json`
- `testdata/M5_TRAJECTORY_RECORD.schema.json`
- `testdata/M5_TRAJECTORY_RECORD.example.json`

更新 `tools/validate_m5_a0_contracts.py`：

- example 必须通过对应 schema；
- schema identity 与本 prompt 一致；
- `result inspect`、`result trajectory` 不再可达 `command.stage_not_available`；
- prune 仍为 dry-run / delete disabled；
- 不降低已有 command/state/placeholder 门禁。

## 6. 必测矩阵

至少新增 table-driven tests 覆盖：

### inspect

- Complete、CompletedWithParticleErrors、Cancelled；
- Failed、Interrupted 缺 provenance 的合法 forensic inspection；
- path / manifest path / series UUID / exact run UUID；
- catalog identity mismatch；
- missing formal artifact；
- SQLite partial warning 分支；
- quality bucket deterministic sort；
- zero tolerance ledger 的 0/0 与 nonzero/0；
- JSON、JSONL、human，machine stderr 为空。

### trajectory

- 单 ID、多 ID 输入顺序逆序但输出按 ID 排序；
- missing ID 在首个 stdout byte 前失败；
- `--all`；
- forward、backward；
- dynamic birth、interior termination、alive-at-end；
- release/domain_initial/domain_boundary origin；
- substance masses sorted；
- terminal classification/fraction 来自 termination table；
- JSONL sequence + 唯一 summary；
- JSON 为完整对象且不构造全量 Vec；
- human 不是 debug JSON；
- stdout/spool/SQLite fault 不输出成功 summary。

### run report

- 同一 inspection 两次 exact bytes 相同；
- Complete、Cancelled、Failed、Interrupted；
- full verification present/absent、superseded_by present/absent；
- forensic suffix `.forensic-aborted` 与 `.1` sorted；
- atomic replace fault 后旧 report 不变、无遗留 temp。

使用现有 public manifest/SQLite types 构造 fixture。不要手写一个与 production schema 不同的“简化 SQLite”来冒充 E2E。

## 7. 门禁

完成后运行并记录：

```text
cargo fmt --all -- --check
cargo test --offline -p trajecta-cli
cargo test --offline -p trajecta-job
cargo test --offline -p trajecta-core verification::tests
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
```

若真实 CFSR fixture 存在，再运行现有 M5 runtime contract；不得下载、安装或伪造资料。

## 8. 交付报告

只新增：

```text
docs/engineering/TRAJECTA_M5_A3_B_DELIVERY_REPORT.md
```

报告必须列出：

- source HEAD 与开始/结束 diff stat；
- 改动文件；
- inspect/trajectory/report 的 exact shape 与测试名；
- JSON bounded-memory 做法；
- 运行的门禁和精确计数；
- 未运行项与原因；
- 明确写：未 commit/push、未改 A core、未改版本、未删除 artifact、不宣称 M5-A3/M5 完成。

首个 P0 无法满足时停止扩功能，保留证据并交 A；不要自行放宽合同或修改 A verifier/catalog 来修绿。
