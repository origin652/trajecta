# 给 B：M4-A1 closeout fixup2（只修剩余四个 P0）

你是 B 模型。本轮依据 `docs/TRAJECTA_M4_A1_A_FIXUP_REVIEW.md` 返工。开始前完整阅读：

- `docs/TRAJECTA_M4_A1_A_FIXUP_REVIEW.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`
- `testdata/M4_RUN_MANIFEST.schema.json`
- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`

保留现有未提交改动；不修改 A 的 `boundary/met_path.rs` 数值核心；不碰外层 `origo-validation-v1.json`；不 commit；不宣称 M4-A1/M4 完成。

A 已接受以下锚点，不要重写：warmup dependency closure、chunk external sort、多轮 fan-in≤32、SQLite ordered cursor lockstep、流式 bundle writer、WAL TRUNCATE 主算法、真实 CFSR 小样本 E2E。

## P0-1：把最终 semantic validation 和 SQLite inspect 也改成真有界

当前阻断是 `serde_json::from_reader::<ProvenanceBundleDocument>` 最终仍生成完整 `samples: Vec<_>`。

要求：

1. 保留 `ProvenanceBundleDocument` 作为小型测试/工具类型可以，但生产 publish validator 禁止反序列化完整 samples；
2. 为 on-disk tmp bundle 实现自定义 serde `Visitor` / `DeserializeSeed` 或等价 streaming parser；
3. header、records、field_sets 可在既有 hard cap 内验证；samples 必须逐项消费，只保留前一 key、count 和当前引用；
4. 逐项检查非负、lowercase SHA、严格排序/唯一、field-set 引用存在；结束后检查 exact count；
5. validator 必须验证实际写出的 tmp 文件，不能只验证写入前内存对象；
6. `ParticleStateSqliteSink::inspect` 删除 `fs::read`，size 用 metadata，SHA 用已有固定缓冲 `file_sha256`；
7. 全仓库搜索大 artifact 的 `fs::read/read_to_end`，生产 SHA/inspection 路径不得全量读。

测试：

- streaming validator 的顺序、重复、缺引用、负 key、错 count、截断 JSON 负例；
- test-only 小 chunk/fan-in 触发多轮，不扩大默认测试时间；
- API 设计上 validator 不返回 samples `Vec`；
- SQLite inspect 对同一文件的 size/SHA/integrity/canonical SQL 与旧结果一致。

## P0-2：闭合 terminal manifest 与 abort/forensic 生命周期

增加明确的 runner→output product→sink abort 协议。建议为 `OutputProduct` / `ParticleStateSink` 增加默认 no-op 的 `abort` 或 `abort_terminal`，SQLite sink 实现真实清理。

必须满足：

1. transaction/write/commit、checkpoint busy/incomplete、bundle write/sync/validation/rename 失败时，立即 abort bundle，清 spool、所有 merge runs 和 tmp；不能等 runner Drop；
2. sink finish 已 rename 正式 bundle、但 terminal manifest persist 失败时，runner 必须回调 sink，将正式 bundle 隔离为唯一、稳定、可审计的 forensic artifact；
3. forensic rename 不得忽略错误；目标冲突要生成唯一名称或先验证安全清理，绝不能留下看似正式的 `provenance-bundle.json`；
4. terminal persist 失败后，磁盘不得出现 `complete` / `completed_with_particle_errors` manifest；已有 running manifest 可保留；
5. 下一次 build/run 不得把 forensic artifact 当本轮正式 bundle；
6. SQLite 主库是否保留为 forensic evidence 要写死合同，但不能被 complete manifest 引用。

至少增加真实全链故障注入：

- sink checkpoint busy；
- bundle BeforeTmpWrite / BeforeRename / AfterRename；
- terminal manifest store 在第二次 persist（terminal）失败；
- forensic 目标预先存在；
- 每个负例断言无正式 final bundle、无 tmp/spool/run，且无 complete manifest。

不要只直接测试 `PRAGMA wal_checkpoint`；必须走 `ParticleStateSqliteSink` 或 `SimulationRunner` 生命周期。

## P0-3：把 normalized/canonical digest 接入正式生产身份

按 A 冻结公式保留 exact SHA，并新增/冻结生产可见的：

- normalized provenance content SHA-256；
- canonical ordered SQL SHA-256；
- canonical output SHA-256。

建议将三项作为 `ProvenanceBundleIdentity` 的必填字段，并同步：

- Rust manifest 类型与 validation；
- `testdata/M4_RUN_MANIFEST.schema.json`；
- bundle/example/合同文档中相关身份描述；
- `ParticleStateSqliteSink::finish`、`provenance_identity()` 与 inspection API；
- CFSR E2E 的实际重算断言。

实现注意：

1. builder finalize 应返回 exact identity + normalized content digest，不要只留在随后被 drop 的局部字段；
2. sink 在最终 SQLite 上计算真实 canonical SQL digest，再调用 `canonical_output_digest`；
3. 删除或真正赋值当前永远为 `None` 的 `last_canonical_output_digest`；不得保留误导诊断字段；
4. canonical SQL 不能包含随机 run UUID，但必须覆盖所有冻结科学输出列；
5. inspection 应能从正式 SQLite + bundle/manifest 重算三项并比对。

测试必须用真实生产路径，不得再传伪 SQL SHA：

- 两个独立 UUID run，同科学内容：exact SQLite/bundle SHA 不同，normalized provenance 与 canonical output 相同；
- 1 worker/4 workers、不同 chunk、输入逆排列：canonical output 相同；
- 改任一 record transform/source：content 与 canonical output 变化；
- 改 field-set slot 或 sample assignment：content 与 canonical output 变化；
- 改 SQL 数值：canonical SQL 与 canonical output 变化；
- 固定 run_id + 固定输入 fixture：exact artifact bytes/SHA 保持相同。

矩阵保持小型；本轮不跑 10 万。

## P0-4：通用多 hole mesh，不接受未证明桥

立即删除 `best_any` fallback。找不到合法桥必须返回 `InvalidGeometry`，不能把已知未证明的桥继续交给 triangulation。

工程目标：对 schema 允许的合法 Polygon/MultiPolygon 提供可重复的 constrained triangulation。优先采用可靠算法；若使用平面 triangulation，必须先冻结适用投影与拓扑合同（建议对 canonicalized、可落入单个开半球的 component 使用使 great-circle 边为直线的投影），输出 triangle indices 后仍以原球面顶点计算面积和采样。遇到 180°、极点或无法安全投影的输入稳定 hard fail。

不得仅靠“triangle area 总和接近 polygon area”或有限随机采样宣称正确。测试 validator 至少检查：

- 每个 triangle 的内部代表点位于 exterior 且不在任一 hole；
- triangle 非边界边不穿 exterior/hole；
- triangle interiors 不互相重叠；
- 总面积在冻结容差内；
- 相同 canonical geometry 的 mesh 顺序确定。

冻结正例：

- 凹 exterior + 两个 hole；
- 至少一个凹 hole；
- 三个以上 hole；
- 日期线 shell + 多 hole；
- MultiPolygon，其中至少一个 component 有多 hole；
- ring 顺序/方向/起点变化后 canonical mesh/digest 不变。

冻结负例：自交、hole 相切/相交、hole 越 shell、精确 180° 边、极点歧义。所有负例稳定 hard fail；不得重采、clamp、质心回退、静默丢 triangle 或放宽面积容差修绿。

若离线依赖不足以安全完成，保留最小失败 fixture，标 `blocked` 交 A；不要把固定双矩形 fixture 通过写成通用 passed。

## 门禁与报告

先跑聚焦负例，再完整运行：

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

更新 `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，逐项区分 implemented / executed / passed / blocked。明确列出：

- streaming validator 是否仍存在 samples `Vec`；
- 每个故障注入后的文件集合与 manifest status；
- exact、content、canonical SQL、canonical output 四类 SHA；
- 多 hole 正负 fixture 名称；
- 未 commit、不宣称 M4-A1/M4 完成、未改 `met_path`、未碰 `origo-validation-v1.json`。

P0 未闭合前，不启动 10 万、WSL/native 或三套真实资料长矩阵。
