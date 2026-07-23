# 给 B：M4-A1 closeout 复验返工

你是 B 模型。本轮只修 A 复验列出的工程 P0。完整阅读：

- `docs/TRAJECTA_M4_A1_A_CLOSEOUT_REVIEW.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`
- `testdata/M4_RUN_MANIFEST.schema.json`
- `docs/TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`

保留现有未提交改动；不修改 A 的 `boundary/met_path.rs` 数值核心；不碰外层 `origo-validation-v1.json`；不 commit；不宣称 M4-A1/M4 完成。

开始返工前先执行 `cargo fmt --all`。A 复验时 `release/geometry.rs:2492` 未通过 `cargo fmt --check`，所以先前报告中的 fmt 绿灯无效。

## P0-1：把 sample spool 改成真有界外排

当前 `load_spool_samples -> Vec -> sort`、SQLite key `Vec/BTreeMap`、完整 document/bytes 和 `fs::read` 全文件均不合格。

实现：

1. 固定 chunk 大小读取原始 sample spool，每块排序并写 sorted run；
2. 多轮 k-way merge，固定 `MERGE_FAN_IN <= 32`，严格限制打开文件数；
3. merge 时检测重复 `(particle_id,sample_sequence)`；
4. 与 `SELECT ... FROM particle_state ORDER BY particle_id,sample_sequence` cursor 单遍 lockstep，证明无遗漏/多余；不得构造全量 key map；
5. records/field-sets 可按已有 hard cap 留内存；samples 必须直接从最终 merged run 流式写入 JSON；
6. 不得构造全量 `ProvenanceBundleDocument.samples` 或完整 bundle `Vec<u8>`；用 digesting writer 边写边算 exact bundle SHA；
7. SQLite、bundle 和其它大文件 SHA 均用固定 buffer 流式计算；
8. TempRunSet/Drop RAII 清理所有本轮 run、spool、tmp。

至少增加：超过两个 merge round、重复 key 跨 run、SQLite 缺/多一行、排序逆序输入、FD fan-in 上界和低内存预算测试。默认测试保持小型，用缩小的 test-only chunk/fan-in 触发多轮。

## P0-2：终态 WAL 必须可证明

1. checkpoint 必须读取并检查 `(busy, log, checkpointed)`；
2. 使用能得到 standalone 主文件的终态策略，建议 `PRAGMA wal_checkpoint(TRUNCATE)`；
3. `busy != 0` 或仍有未 checkpoint frame 一律 hard fail；
4. 关闭 writer 后确认 `particles.sqlite-wal` 不存在或长度为 0，再流式 hash 主文件；
5. 增加 active read transaction 导致 checkpoint busy 的真实负例，不得继续发布 bundle/complete manifest；
6. 无 reader 正例证明 checkpoint、close、hash、只携主 DB 重新打开和 `integrity_check` 成功。

## P0-3：replace 前完整 semantic validation

抽出纯函数 validator，至少检查：

- schema/version/algorithm/run UUID/SQLite relative path；
- lowercase 64-hex SHA；
- records/field_sets/samples 固定排序和唯一；
- record/field-set hash 重算；
- sources 非空、唯一且保序；
- operation、parameter name、fallback 非空；
- transform parameter 排序与重复规则；
- field-set 五 slot 精确且引用 field 一致；
- sample 非负、唯一且引用存在；
- SQL non-null→record non-null；pressure/temp quality 与 record 一致；wind quality 等于 U/V/W worst-of；
- counts、SQLite SHA、run_id 全部一致。

serde self-parse 不等于 schema validation。测试必须包含错 slot、错 hash、缺引用、漏/多 sample、uppercase SHA、空 source/operation、quality mismatch 等负例。

## P0-4：失败清理和 terminal 生命周期

- builder/sink 提供明确 abort/Drop cleanup；
- transaction 失败、checkpoint busy、bundle write/sync/rename、manifest persist failure 均不得留下 complete manifest；
- 清理 `.tmp`、sample spool、merge runs；
- 若 exact final bundle 已 rename 但 terminal manifest 失败，必须有冻结且可复验的 forensic 状态，下一次不得误复用；
- 为上述每个故障点增加注入测试。

## P0-5：实现 normalized digest，不再比较随机 UUID artifact

按 A 文档冻结的字节公式实现：

- `trajecta.provenance-content/v1` normalized digest；
- `trajecta.canonical-output/v1` = canonical ordered SQL digest + normalized provenance digest。

exact bundle/SQLite SHA 仍写 manifest，服务单 run 审计。跨独立 run 的 1/4 worker、chunk、逆排列比较 normalized digest；只有固定 run_id/SQLite identity fixture 才要求 exact artifact bytes/SHA 相同。

增加：

- 同科学内容、不同 UUID：exact SHA 不同但 normalized digest 相同；
- 改任一 record transform/source、field-set slot 或 sample assignment：normalized digest 必变；
- canonical output digest 对 SQL 数值变化和 provenance 变化都敏感。

## P0-6：通用多 hole mesh

当前 `two_holes...` 测试不得再用 `match Ok/Err` 允许失败。对合法 fixture 必须：

- mesh 必成功；
- triangle area 与 exterior-minus-holes 在冻结容差内；
- triangles 不 double-cover、不穿 hole；
- 采样永不进入任一 hole 或凹 bay；
- 支持凹 exterior、多个 hole、凹 hole、日期线与 MultiPolygon；
- 自交、ring 相切/交叉、180°/极点歧义稳定 hard fail。

优先采用可靠的 constrained triangulation/earcut 工程路径。不得用重采、clamp、质心回退、静默丢 triangle 或放宽面积容差修绿。若离线依赖阻断，提交最小 fixture 和候选依赖，停止该分支交 A。

## 复验与报告

先跑聚焦负例，再跑：

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

真实 CFSR E2E 必须重算而非只检查 SHA 长度：bundle exact SHA、SQLite SHA、record/field-set SHA、counts、run_id、slot 引用和 SQLite 全覆盖。

更新 `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，逐项区分 implemented/executed/passed/blocked。10 万、WSL、native、三套资料完整轨迹和 P1 全矩阵本轮先不跑。
