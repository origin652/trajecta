# 给 B：M4-A1 closeout Fixup3（精确补洞，不重写主体）

你是 B 模型。本轮完整阅读：

- `docs/TRAJECTA_M4_A1_A_FIXUP2_REVIEW.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`
- `testdata/M4_RUN_MANIFEST.schema.json`

保留全部现有未提交改动；不修改 `boundary/met_path.rs`；不碰外层 `origo-validation-v1.json`；不 commit；不宣称 M4-A1/M4 完成。

A 已接受：sample 逐条 seed、streaming SQLite inspect、external sort/WAL/warmup、digest 三字段进入 identity、unique forensic name、删除 `best_any`。不要重写这些主体。

## P0-1：收紧 streaming document parser

1. `seed.deserialize(&mut de)` 成功后必须调用 `de.end()`，拒绝第二个 JSON value、尾随非空字节和垃圾；
2. 对每个顶层字段使用 duplicate guard；重复 `schema_version/run_id/sqlite/record_hash_algorithm/records/field_sets/samples` 一律 hard fail；
3. `samples` 只能出现一次；不得通过第二个 samples 数组分别满足 expected count；
4. records/field_sets 改用 capped `DeserializeSeed`，逐项 push，在 `MAX_UNIQUE_* + 1` 立即失败；禁止先构造任意大 `Vec` 再查长度；
5. records/field_sets 仍在 cap 内完整做排序、唯一、hash、slot 和 builder dictionary 校验；
6. validator 返回一个小型 `BundleValidationSummary`，至少含 counts 和从实际 on-disk 内容计算的 normalized content digest，不返回 samples `Vec`。

必须补负例：

- 合法 bundle 后追加 `{}`；
- 尾随非空垃圾；
- 每个顶层字段分别重复；
- samples 重复；
- records/field_sets 超 cap；
- truncated JSON；
- 未知字段、缺字段保持 hard fail。

## P0-2：错误不得再被 `let _ =` 吞掉

1. 将 builder `abort` 改为返回 `Result`，AfterRename 时 quarantine 失败必须上传；
2. runner 建立统一 `abort_outputs` / `quarantine_outputs`，收集并返回错误；关键路径禁止 `let _ =`；
3. terminal manifest persist 失败后：先 quarantine，确认正式 bundle 不存在，再清理；若 quarantine 失败，返回包含 persist + quarantine 两者的结构化错误，绝不能报告已清理；
4. failed-manifest 的二次 persist 结果也不得静默丢弃；需要冻结优先级/聚合规则；
5. `finalize_success` 中 contribute_manifest、termination summary、lifecycle clock、manifest validation 等 terminal 步骤失败时，同样执行 output abort/quarantine，而不是只覆盖 persist failure；
6. sink finish/checkpoint/write/commit 失败在返回前或 runner 捕获点立即清 spool/tmp/runs。

增加真实 runner 级测试：

- 使用真实 `ParticleStateSqliteSink` 产出正式 bundle；
- manifest store 第一次 running persist 成功、第二次 terminal persist 强制失败；
- 断言无正式 `provenance-bundle.json`，存在唯一 forensic，tmp/spool/runs 全无，磁盘无 complete manifest；
- 预放 `.forensic-aborted`，必须选择 `.1`；
- 注入 quarantine failure，断言错误不可被原 persist error覆盖/吞掉；
- AfterRename 走 sink/runner 链，不只 builder 单测。

## P0-3：从 on-disk artifact 独立重算 digest

复用 P0-1 streaming validator，在读取实际 bundle 时同时计算：

```text
trajecta.provenance-content/v1
```

并返回 `BundleValidationSummary.content_sha256`。增加 run-output inspection：

1. 流式 hash exact bundle；
2. 流式验证并重算 bundle content digest；
3. inspect SQLite，重算 exact SHA 与 canonical SQL；
4. 重算 canonical output；
5. 与 manifest 五个 digest 字段逐项比较。

`RunManifest::validate` 至少应直接重算并校验：

```text
canonical_output_sha256 ==
canonical_output_digest(sqlite_sql_sha256, content_sha256)
```

测试不得再给 builder 传伪 SQL SHA 来证明 production determinism：

- 两个真实 sink/runner run，不同 UUID、相同科学内容：exact SHA 不同，content/canonical SQL/canonical output 相同；
- 小型 1/4 worker、chunk、逆排列生产路径：canonical output 相同；
- 改 SQL 数值、record transform/source、field-set slot、sample assignment，各自使对应 digest 变化；
- CFSR E2E 必须从磁盘重算 content 和 canonical output，不只检查长度。

保持矩阵小型，不跑 10 万。

## P0-4：给现有 mesh 加真正 topology validator

若继续使用当前 keyhole/ear-clip 路径，`finish_mesh` 必须验证的不只是代表点和面积：

1. 使用球面向量归一化得到 triangle interior representative，禁止经纬度算术平均；
2. 每条 triangle 非共享边不得与 exterior/hole 边 proper-intersect；
3. triangle 不得包含任何 hole vertex，hole edge 也不得穿 triangle；
4. 任意两个 triangles 的 interiors 不得 overlap；除共同顶点/共同完整边外不得相交；
5. triangle containment 也要检测，不能只查 edge crossing；
6. 最后再做 positive excess 与面积守恒；
7. validator 应是确定性 exact topology gate，随机采样只能作补充。

补齐正例：

- 凹 exterior + 多 hole；
- 三个 hole，其中含凹 hole；
- 日期线 shell + 至少两个 holes；
- MultiPolygon，至少一个 component 含多个 holes；
- ring 起点、方向、hole 输入排列变化后的 canonical mesh/digest 确定性。

补齐负例：hole 相切、hole 相交、hole 越 shell、triangle overlap 的人工坏 mesh、自交、180°/极点歧义。若当前算法无法通过合法 fixture，保留最小反例并标 blocked；不得用更松面积容差或更多随机采样修绿。

## 门禁与报告

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/validate_m4_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-core --test m4_a1_cfsr_e2e -- --nocapture
```

更新 `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`。必须列出新增负例测试名、runner 故障后实际文件集合、两个真实 production run 的五类 digest，以及 multi-hole topology validator 覆盖项。未闭合项标 blocked，不要只写 fixture passed。
