# B Prompt：M5-A1 review closeout（机械合同修复）

角色：你是 B，只处理本 Prompt 列出的中等及以下机械修复。不要调用子代理。不要 commit、push、打 tag或发布；完成后交 A 复验，不得宣称 M5-A1 或 M5 完成。

## 1. 当前工作区与 A-owned 代码

A 已在当前未提交工作区完成以下较难修复，并已增加冻结反例：

- Project path jail：规范 `/` 分隔、拒绝重复分隔符与反斜杠、检查最深现存祖先，拒绝 symlink/junction 逃逸；
- 渐进 Case/Profile：只有明确的 `missing field` 可作为 draft，未知字段、错误类型、错误 kind/version 必须报错；
- Project 状态不再产生第四个 `error` 状态；错误通过 diagnostic 和退出码表达，state 保持 `draft/configured/finalized`；
- `doctor --deep` 使用唯一临时目录、`create_new`、SQLite 严格清理和有界资料扫描，不再覆盖固定同名文件。

这些修改位于：

```text
crates/trajecta-cli/src/project.rs
crates/trajecta-cli/tests/m5_a1_cli_contracts.rs
```

不要回退、复制或重写上述 A-owned 实现。若你的修改与这些区域冲突，停止并交 A。

开发期 crate 版本继续保持 `0.0.0`。不要新增 schema v2、fixup schema 或其他版本文件。不要触碰数值核心、M4 正式 evidence 或仓库外 `origo-validation-v1.json`。

## 2. P0-1：Config 运行时语义必须等价于冻结合同

修复 `crates/trajecta-cli/src/configuration.rs::validate`：

1. Profile template 名使用 `trim()` 后必须非空；纯空白名必须拒绝；
2. 每个 template 的 `execution.worker_threads` 必须 `<= resources.cpu_slots`；
3. 保留现有 `memory_reserve_mib < memory_pool_mib`、正数、reader、monitoring 和 `deny_unknown_fields` 规则；
4. 失败使用稳定产品诊断，且 `config set` 失败后旧文件字节完全不变。

至少新增反例：

```text
cpu_slots=1 + template worker_threads=2 -> exit 1
profile_templates.<纯空白名> -> exit 1
失败前后 config SHA-256 相同
```

## 3. P0-2：ProjectIndex 补齐冻结 schema 语义

在不修改 A 的路径 containment helper 前提下，补齐 `validate_index`：

- Case/Profile map 的名字必须非空；建议同时拒绝纯空白；
- `ProjectProfile.path` 继续走 A 的 portable path jail；
- `dataset_profiles` 的 dataset ID 与 profile 名必须非空；
- `template` 若存在必须非空；
- `template_sha256` 若存在必须恰好 64 个 ASCII 十六进制字符；
- template 与 SHA 仍必须成对出现；
- `default_profile` 必须指向现存非空 Profile 名；
- 所有失败必须在写 index 前发生，旧 index 字节不变。

冻结反例至少包括：

```text
index.cases.<空名>
空 profile 名
dataset_profiles 的空 key / 空 value
空 template
短 SHA / 非 hex SHA
```

诊断使用 `project.invalid_index`；路径类错误继续使用 A 已有的 `project.path_escape`。

## 4. P0-3：机器模式用法错误必须保留 exit code 2

当前 `main_entry` 在 JSON/JSONL parse-error 路径把所有错误经 `write_outcome` 压成了 1。修复为：

- 参数、命令、格式、未知 flag、重复 flag、缺值：human/JSON/JSONL 均返回 2；
- JSON 仍输出唯一 `trajecta.cli-output/v1` envelope；
- JSONL 仍输出有序 diagnostic + 唯一 summary；
- 应用/产品错误继续返回 1；成功继续返回 0；
- 不把 diagnostic 复制到 stderr。

修改现有错误测试：不得继续把 `project unknown` 的 exit 1 固定为正确结果。增加 JSON 与 JSONL 的表驱动用法错误测试。

## 5. P0-4：冻结 staged `data lock` 诊断

在 A 接通真实 DatasetLockBuilder 前，保持解析面：

```text
data lock --root DIR --profile NAME --case FILE --output PATH [--replace]
```

暂时的稳定非零诊断必须为：

```text
data.lock_coverage_required
```

不得返回当前漂移出的 `data.lock_stage_owned_by_a`，不得生成空 lock、假 coverage 或假 success。输出已存在且没有 `--replace` 时，仍优先返回 `data.lock_exists`。

## 6. P1：data-plan runtime guard 补齐语义

保持生成算法不变，扩充 `validate_data_plan_shape`，使运行时 guard 同时检查：

- requirements 严格按 `(profile_name, case_name, dataset_id)` 排序；
- 上述 key 唯一；
- `required_capabilities` 已排序且唯一；
- `coverage_start <= coverage_end`；
- lockfile、cache_root、data_roots value 是规范、非空、项目相对 `/` 路径：禁止绝对路径、反斜杠、`.`、`..`、重复分隔符；
- SHA 继续为 64 位十六进制；
- optional `cache_root` 缺失时必须省略，不得为 null。

为纯 guard 增加 table-driven 拒绝测试；不要只测试生成器自然会生成的单 requirement 正例。若 private 函数不便从 integration test 调用，可在同模块添加 `#[cfg(test)]` 单元测试，不要扩大 public API。

## 7. 报告修正

更新：

```text
docs/engineering/TRAJECTA_M5_A1_B_DELIVERY_REPORT.md
```

必须：

- 删除不存在的 “server/credential ordering” 声明；
- 把 data-lock staged 诊断写成实际的 `data.lock_coverage_required`；
- 记录 A 已完成的 path jail、partial validation、三态状态和 doctor 临时目录修复，但不得把它们冒充 B 本轮实现；
- 更新真实测试计数与报告 SHA；
- 明确 `data lock` 真实闭环和 `project finalize` 仍由 A 完成；
- 保留“未 commit / 未 push / 不宣称 M5-A1 或 M5 完成”。

## 8. 门禁

依次执行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline -p trajecta-cli
cargo test --offline -p trajecta-job
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python tools/validate_m5_a0_contracts.py
python tools/validate_m4_a0_contracts.py
python -m py_compile tools/validate_m5_a0_contracts.py
git diff --check
```

不要运行真实资料正式矩阵、50k/100k、百万点、WSL native 或 FLEXPART。

## 9. 停止条件

遇到以下任一情况立即停止并交 A：

- 需要修改 `trajecta-core`、`trajecta-met`、`trajecta-case` 或 `trajecta-job`；
- 需要改 schema、状态机、版本号或 A 的 path/partial/doctor 实现；
- 需要复制 private scientific/lock 逻辑；
- 任一既有 M4 测试回退；
- 无法在不伪造 success 的前提下完成。

最终只交实现、测试结果与更新后的报告；不要 commit/push。
