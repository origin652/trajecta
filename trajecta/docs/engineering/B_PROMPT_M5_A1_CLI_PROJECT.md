# B Prompt：Trajecta M5-A1 CLI / Config / Project 工程闭环

仓库：`E:\flexpart\trajecta`

角色：你是 B，只执行冻结后的中等及以下工程任务。不要调用子代理。不要 commit、push、打 tag 或发布。完成后交 A 复验，不得宣称 M5-A1 或 M5 完成。

## 1. 开工前必须完整阅读

按顺序完整阅读：

1. `docs/engineering/TRAJECTA_M5_A0_PRODUCT_CONTRACT.md`
2. `docs/engineering/TRAJECTA_M5_PRODUCTIZATION_PLAN.md`
3. `docs/engineering/TRAJECTA_M5_MODEL_ASSIGNMENT.md`
4. `testdata/M5_CLI_CONTRACT.v1.json`
5. `testdata/M5_JOB_CONTRACT.v1.json`
6. 所有 `testdata/M5_*.schema.json` 和对应 example
7. `crates/trajecta-job/src/model.rs`
8. `crates/trajecta-job/src/backend.rs`
9. 当前 `crates/trajecta-cli` 全部源码和测试

先运行并保存基线：

```text
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
cargo test --offline -p trajecta-cli
cargo test --offline -p trajecta-job
git diff --check
```

如果 A0 validator 不是 passed，立即停止并报告，不得修合同。

## 2. 唯一目标

实现 M5-A1 的 CLI、机器配置、项目渐进编辑、确定性 data-plan、Case/Data 现有命令接线和 A1 范围 doctor，使 A0 已冻结的命令与机器输出可实际使用。

本 Prompt 不实现：

- daemon、IPC、worker 或持久任务数据库；
- Runner 取消、重新关联或恢复；
- `project finalize` 的 lock 创建/替换核心；
- run/job/result 的真实执行；
- rerun/prune 的真实删除；
- FLEXPART、打包或正式真实资料矩阵。

## 3. 允许修改范围

允许：

- `crates/trajecta-cli/**`
- `crates/trajecta-cli/Cargo.toml`
- workspace `Cargo.lock`，仅限实际新增 CLI 依赖导致的机械更新
- 新增 CLI 的测试 fixture，但不得修改冻结 M5 schema/contract
- `docs/engineering/TRAJECTA_M5_A1_B_DELIVERY_REPORT.md`

只读、禁止修改：

- `crates/trajecta-core/**`
- `crates/trajecta-met/**`
- `crates/trajecta-case/**`
- `crates/trajecta-job/**`
- `testdata/M5_*`
- `testdata/M4_*`
- `docs/engineering/TRAJECTA_M5_A0_PRODUCT_CONTRACT.md`
- `docs/engineering/TRAJECTA_M5_PRODUCTIZATION_PLAN.md`
- `docs/engineering/TRAJECTA_M5_MODEL_ASSIGNMENT.md`
- 所有版本号、数值容差、异常分类和 schema identity
- `E:\flexpart\origo-validation-v1.json`

需要新增依赖时优先使用已经缓存的 `toml`、现有 `serde_yml`、`serde_json`、`sha2` 和 `hex`。不得引入 CLI framework、异步 runtime、数据库或“以后可能用到”的抽象层。

## 4. CLI 解析与公开输出

### 4.1 全局选项

实现：

```text
--format human|json|jsonl
--json
--config PATH
--project PATH
```

要求：

- `--json` 与 `--format json` 同义；同时出现 hard usage error 2；
- 未指定格式时为 human；
- 不得 lossy 转换非 UTF-8 路径；
- 未知 flag、重复单值 flag、缺值和互斥 input 模式均返回 2；
- parser 不打开资料、不执行科学工作。

### 4.2 输出合同

非流式 JSON 必须完全符合 `trajecta.cli-output/v1`：

```text
schema_version / command / ok / run_success? / data / diagnostics
```

JSONL 每行必须符合 `trajecta.cli-stream-item/v1`：

- sequence 从 1 递增；
- data、diagnostic、summary 三类严格互斥；
- 正常结束恰好一个最终 summary；
- stdout 只写 JSON/JSONL；
- stderr 不复制机器 diagnostic；
- human 输出简短、稳定、不得改变退出码。

移除 `trajecta-cli` 当前所有 `NotImplemented` / `command not implemented` 占位。A2/A3 命令即使尚无后端，也必须能正确解析并返回稳定的阶段不可用产品诊断，不能返回用法错误或 panic。

统一阶段不可用诊断：

```text
command.stage_not_available
```

退出码为 1，hint 必须指出负责阶段，例如 `requires M5-A2 local daemon`。

## 5. 命令语法

实现以下解析：

```text
trajecta config init
trajecta config path
trajecta config list
trajecta config get KEY
trajecta config set KEY VALUE
trajecta config unset KEY
trajecta config validate

trajecta project init [PATH] --name NAME
trajecta project status
trajecta project show
trajecta project get SELECTOR
trajecta project set SELECTOR VALUE
trajecta project unset SELECTOR
trajecta project validate
trajecta project data-plan [--output PATH|-]
trajecta project finalize

trajecta case validate PATH [--intent simulation|met-probe|migration]
trajecta case resolve PATH
trajecta data inspect FILE
trajecta data lock --root DIR --profile NAME --output PATH [--replace]
trajecta met probe ...
trajecta met replay ...
trajecta doctor [--deep]

trajecta run (--project PATH --profile NAME | --case FILE --run-profile FILE) [--detach]
trajecta job list|status|wait|events|cancel|rerun|forget|prune ...
trajecta result inspect|verify|trajectory ...
```

本轮真实执行到 `doctor` 为止。`project finalize`、run、job 和 result 只做完整 typed parse，然后返回 `command.stage_not_available`。禁止创建空 daemon、假 job ID、假 success 或假 result。

`run` 必须冻结：

- 默认 `detach=false`，即未来前台等待；
- 只有显式 `--detach` 才为后台；
- project 与 direct input 互斥；
- 关闭 CLI 不等价于 cancel，只体现在 help 和 typed command，不实现 worker。

## 6. Config 实现

### 6.1 路径优先级

严格按：

1. `--config PATH`
2. `TRAJECTA_CONFIG`
3. OS 默认路径

实现 Windows 与 Linux 默认路径；测试通过注入环境/路径，不污染用户真实配置。

### 6.2 文件和语义

实现与 `M5_CONFIG.schema.json` 等价的强类型结构，`deny_unknown_fields`。默认 reader 为 Rust。

`config init`：

- 探测逻辑 CPU 和物理内存；
- 将明确数值写入文件；
- 不覆盖已有文件；
- 不读取当前空闲内存作为后续动态 pool；
- 生成的配置必须通过同一 validator。

`config set KEY VALUE`：

- VALUE 先按 JSON value 解析，失败时作为字符串；
- 点路径必须存在于冻结结构或合法 template 名下；
- 禁止通过 set 引入未知顶层字段；
- 修改后先完整验证，再原子 replace；
- 失败时旧文件字节必须保持不变。

`list` 输出按完整点路径排序的 leaf；`get` 返回 typed value；`unset` 对必需字段应拒绝并给出 stable diagnostic，对 template 条目可删除。

建议诊断码：

```text
config.not_found
config.already_exists
config.invalid_schema
config.invalid_key
config.invalid_value
config.write_failed
```

不得把 TOML 注释保留作为合同；规范重写允许丢失注释。

## 7. Project 实现

### 7.1 发现与初始化

项目优先使用 `--project`，否则从 cwd 向父目录寻找最近的 `trajecta-project.yaml`。

`project init [PATH] --name NAME`：

- 创建 `trajecta-project.yaml`、`cases/`、`profiles/`、`locks/`、`runs/`；
- 初始 cases/profiles 为空，因此状态为 draft；
- 目标已有 index 时拒绝；
- 不生成虚假的完整 Case、Profile 或 DatasetLock。

### 7.2 Selector

实现：

```text
index.<field path>
case.<case-name>.<field path>
profile.<profile-name>.<field path>
```

- index 必须符合 `trajecta.project-index/v1`；
- case/profile 使用通用 YAML mapping 支持渐进字段；
- 当文件达到完整形状时，必须调用现有 `trajecta-case` parser/validator；
- 缺字段可为 draft，错误类型、未知字段或冲突必须为 error；
- set/unset 原子写；
- 规范 YAML 重写即可，不要求保留注释。

### 7.3 状态

实现纯推导，不把 state 写进 index：

- draft：缺少具体 Case/Profile 或 simulation intent 必需字段；
- configured：全部文档 shape/intent 正确，每个 Profile 的 case_path 指向索引 Case，每个 dataset binding 与索引 `dataset_profiles` 一一对应；资料和 lock 可缺；
- finalized：本轮不要自行产生。只有 A 的 finalize 实现可返回 finalized。

在 A1 中，即使已有资料看似齐全，也只报告 configured，并增加 info：

```text
project.finalize_requires_a
```

这样避免 B 自行复制 lock/coverage 裁决。

### 7.4 Data plan

仅 configured 项目可生成 `trajecta.data-plan/v1`。

算法冻结：

1. 按 Profile 名排序；
2. 解析具体 RunProfile，并通过 case_path 找到唯一索引 Case；
3. 从 Case time 获取 coverage；
4. 从索引 `dataset_profiles` 获取每个逻辑 dataset 的 profile 名；
5. 必须调用 `trajecta_core::runner::required_capabilities_for_population`，禁止复制 population → capability 映射；
6. capability 用 serde snake_case 字符串排序；
7. requirement 按 `(profile_name, case_name, dataset_id)` 排序；
8. `project_sha256` 是 `{index,cases,profiles}` 解析值构成的 BTreeMap canonical JSON SHA-256；不得包含 mtime、生成时间、绝对项目根或随机值；
9. lock 和 root 路径按项目相对路径输出；
10. lock 不存在且无资料为 missing；部分 roots/文件存在为 partial；lock 存在且现有公开 lock verifier 通过为 ready；
11. 不下载、不创建 lock、不改项目。

重复执行必须 byte-for-byte 相同。

## 8. Case、Data 和 Met

- 完成现有 CaseCommand、DataCommand 的 parser 与 dispatch；
- 所有 shape/reference/intent 规则调用 `trajecta-case`；
- data inspect/lock 调用 `trajecta-met` 已有公开 API；
- `--replace` 是唯一允许覆盖既有 lock 的显式开关；没有该 flag 必须拒绝；
- 保留现有 met probe/replay 科学和 provenance 路径，不复制或改写其算法；
- 把现有 met 输出接入冻结的 human/JSON/JSONL 外层，不改变 replay record 内容。

如果缺少完成 data lock 所需的公开 API，停止该子项并在报告中给出具体缺口；不得把 private 逻辑复制进 CLI。

## 9. Doctor A1 范围

`doctor` 检查：

- config 路径、解析和资源语义；
- 项目发现、index、Case/Profile 状态；
- 输出/临时目录创建、写入、flush、rename 和清理能力；
- reader backend 名称是否合法。

`doctor --deep` 在上述基础上：

- 对已存在 lock/资料调用公开 verifier/inspect；
- 使用临时目录验证 SQLite 创建、integrity_check、WAL checkpoint；
- daemon/IPC/worker 检查输出 info `doctor.daemon_pending_m5_a2`，不得假装通过或失败；
- 不启动模拟，不打开远程网络，不修改项目。

configured 项目缺计划资料为 warning/pending，退出 0；结构错误、finalized 伪称、lock 不一致或不可写为 error，退出 1。

## 10. 测试要求

至少增加：

- 表驱动 35 个公开命令路径解析测试；
- `run` 默认前台与显式 detach；
- global flag 重复、冲突、缺值、未知值；
- human/JSON/JSONL schema shape 和最终 summary；
- machine stdout 无杂质；
- config 路径三层优先级；
- config init 不覆盖；
- config set 类型解析、必需字段 unset 拒绝；
- 原子写失败后旧文件 SHA 不变；
- project 向父目录发现；
- project init 为 draft；
- selector get/set/unset；
- invalid type 与 missing field 的 error/draft 区分；
- configured 前 lock/资料允许缺失；
- dataset_profiles 缺失/多余 hard fail；
- data-plan byte-for-byte deterministic；
- 三种 population 的 capability 集来自公开 helper；
- stage-not-available 返回 1 而非 2；
- no reachable panic、todo、unimplemented 或新的版本文件。

测试必须使用 tempdir 和注入环境，不能读写真实 `%APPDATA%`、用户 home 或仓库外配置。

## 11. 门禁顺序

依次执行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline -p trajecta-cli
cargo test --offline -p trajecta-job
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
python -m py_compile tools/validate_m5_a0_contracts.py
git diff --check
```

不要运行真实资料正式矩阵、50k/100k、百万点、WSL native 或 FLEXPART。

## 12. 停止条件

遇到以下任一情况立即停止并交 A：

- 需要修改 core/met/case/job 才能继续；
- 机器合同、状态机或 schema 自相矛盾；
- data-plan 无法通过公开 helper 得到资料需求；
- public lock API 无法完成 data lock；
- 原子替换在 Windows 无法保持旧文件；
- 任一既有 M4 测试回退；
- 需要新增版本、放宽验证或使用假 success。

不要自行“修绿”合同。

## 13. 交付

报告写入：

```text
docs/engineering/TRAJECTA_M5_A1_B_DELIVERY_REPORT.md
```

报告必须包含：

- 实际修改文件；
- 支持与仍 staged 的命令；
- config/project 行为摘要；
- data-plan deterministic SHA；
- 测试和门禁计数；
- known placeholder 数量变化；
- 首个失败及 artifact；
- 未运行项目和诚实限制；
- 明确声明未实现 daemon/finalize/run/job/result；
- `未 commit / 未 push / 不宣称 M5-A1 或 M5 完成`。
