# Trajecta M4-A1：A 对 closeout fixup 的复验裁决

状态：工程门禁可复现全绿；外排排序与 WAL 主路径有实质进展；但有界终态校验、terminal 失败生命周期、规范化 digest 生产接线和通用多 hole 仍是 P0。**本轮不通过 M4-A1**。未 commit，不宣称 M4-A1/M4 完成。
日期：2026-07-23

依据：

- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`
- `testdata/M4_RUN_MANIFEST.schema.json`

## 1. 裁决表

| 项 | A 裁决 | 说明 |
|---|---|---|
| P0-1 chunk / k-way merge / SQLite lockstep writer | **accepted as implementation anchor** | chunk、跨 run 重复检测、多轮 fan-in≤32、ordered SQLite cursor 与流式 JSON writer 均已存在 |
| P0-1 全链有界内存 | **rework required** | publish 前 validator 仍把全部 `samples` 反序列化为 `Vec`；SQLite inspect 仍 `fs::read` 全文件 |
| P0-2 WAL TRUNCATE 主路径 | **accepted** | 检查 `(busy,log,checkpointed)`，关闭 writer 后拒绝非空 WAL，再流式 hash 主库 |
| P0-2 WAL terminal 失败证明 | **partial** | busy 测试只直接调用 SQLite pragma，未覆盖真实 sink/runner 的 bundle、spool、manifest 生命周期 |
| P0-3 semantic rules | **partial** | header/hash/slot/source/quality 等规则已实现，但 validator 的资源合同不合格 |
| P0-4 abort / Drop / terminal 生命周期 | **blocking** | terminal manifest persist 失败后，已 rename 的正式 bundle 不会 quarantine；故障矩阵未接 runner |
| P0-5 normalized / canonical digest | **blocking** | helper 和单测存在，但生产 sink/manifest/inspection 不发布 digest，`last_canonical_output_digest` 从未赋值 |
| P0-6 通用多 hole mesh | **blocking** | 仅固定双矩形 hole 通过；找不到合法桥时仍回退到未证明合法的 `best_any` |
| 工程门禁 | **passed** | A 本轮独立复现 326 passed / 2 ignored，真实 CFSR E2E 通过 |

## 2. 已接受的实现锚点

### 2.1 外排与写出主体

`output/provenance_bundle.rs` 当前已有：

- 固定 chunk 读取 sample spool；
- chunk 内排序和重复 key 检测；
- 多轮 k-way merge，单次输入 fan-in 硬上限 32；
- 跨 run 重复 `(particle_id,sample_sequence)` 检测；
- 与 SQLite `ORDER BY particle_id,sample_sequence` cursor 单遍 lockstep；
- samples 直接从最终 sorted run 写 JSON；
- `DigestingFile` 边写边计算 bundle SHA；
- `file_sha256` 使用 64 KiB 固定缓冲。

聚焦 provenance 单测本轮独立运行：9 passed。

### 2.2 WAL 主路径

`output/sqlite.rs::finish` 的主路径满足：

1. `PRAGMA wal_checkpoint(TRUNCATE)`；
2. 检查 `busy == 0`；
3. 检查 `log == 0 && checkpointed == 0`；
4. 关闭 writer；
5. 非空 `particles.sqlite-wal` hard fail；
6. 再流式 hash 主数据库。

因此 WAL 算法主体接受，不要求 B 重写；下一轮只补真实 sink/runner 失败生命周期。

### 2.3 真实 CFSR anchor

A 显式设置 `TRAJECTA_REQUIRE_REAL_MET=1` 重跑：

```text
cargo test --offline -p trajecta-core --test m4_a1_cfsr_e2e -- --nocapture
→ 1 passed; 0 failed; finished in 7.96s
```

该 anchor 证明当前小样本真实资料链能得到 `RunOutcome::Complete`、无异常 termination、SQLite 与 bundle exact SHA 对齐、sample 全覆盖及五 slot 引用存在。它不证明 10 万规模资源上限或跨 run canonical digest。

## 3. 仍阻断的 P0

### 3.1 publish 前 semantic validator 仍无界

`ProvenanceBundleDocument.samples` 仍是：

```rust
pub samples: Vec<BundleSampleAssignment>
```

而 `validate_bundle_file_semantics` 执行：

```rust
let doc: ProvenanceBundleDocument =
    serde_json::from_reader(BufReader::new(file))?;
```

`from_reader` 只是从 reader 取字节；反序列化目标是 `Vec` 时，全部 samples 仍常驻内存。因此最终 publish 路径仍随 sample 数线性增长，和前面的真外排相互抵消。

此外 `ParticleStateSqliteSink::inspect` 仍使用 `fs::read(path)` 计算 size/SHA，会把整个 SQLite 文件读入内存。生产 inspection 也必须改为 `metadata.len()` + 固定缓冲 hash。

要求：对 on-disk tmp bundle 使用自定义 streaming visitor/seed，header 与受 hard-cap 约束的 records/field_sets 可保留，samples 必须逐项验证 count、顺序、唯一、非负及 field-set 引用，禁止生成完整 `ProvenanceBundleDocument.samples`。

### 3.2 terminal manifest 失败后没有 forensic 闭环

当前顺序是 sink `finish` 先 rename 正式 bundle 并把 builder 标记 `finalized_ok=true`，随后 runner 才写 terminal manifest。若 `RunManifestStore::persist` 在 terminal write 失败：

- runner 直接返回 manifest error；
- 磁盘上旧 manifest 仍是 `running`；
- 正式名 `provenance-bundle.json` 仍留在 run 目录；
- builder 已 finalized，不会在 Drop 时 quarantine；
- runner 没有调用 output/sink abort 的接口。

这不满足 fixup 合同中“rename 后 terminal manifest 失败必须形成冻结 forensic 状态”。

同时，当前 `abort()` 对 final→forensic rename 错误直接忽略；若 forensic 目标已存在，Windows 上可能 rename 失败并留下正式 final。checkpoint/transaction 失败也只依赖对象最终 Drop，runner 尚存活时 spool 不会立即清理。

要求：增加 runner→product→sink 的显式 abort/rollback terminal 协议；terminal persist 失败、checkpoint busy、transaction/write/commit、bundle sync/rename 均立即清理临时物，rename 后失败必须把正式 bundle 原子隔离到唯一且可审计的 forensic 名称，rename 失败本身必须上报，不能忽略。

### 3.3 normalized/canonical digest 没有生产接线

代码中已有公式 helper，但生产链不发布结果：

- `last_content_digest` 只存于局部 `ProvenanceBundleBuilder`；sink finish 后 builder 被丢弃；
- `last_canonical_output_digest` 初始化为 `None`，全仓库没有赋值；
- `canonical_output_digest(...)` 只被单测调用，且测试传入伪造的 SQL SHA；
- `ProvenanceBundleIdentity`、run-manifest schema 和 `ParticleStateSqliteSink::provenance_identity()` 都不含 normalized/canonical digest；
- `SqliteInspection` 只暴露 canonical SQL 的半成品，没有把正式 bundle content digest 合入 canonical output。

现有 `normalized_digest_stable_across_run_ids` 只能证明 helper 公式，不证明 1/4 worker、chunk 或逆排列的真实生产产物可比较。

要求：在正式 sink finish 中计算并保存三项身份：normalized provenance content、canonical ordered SQL、canonical output；通过冻结 schema 的 manifest/inspection API 发布，并以两个真实独立 run 验证 UUID 改变时 exact SHA 不同而两项 normalized/canonical digest 相同。修改 record transform/source、field-set slot、sample assignment 或 SQL 数值时，相应 digest 必须变化。

### 3.4 多 hole 仍是 fixture 特化，不是通用支持

`multi_hole_keyhole_boundary` 的桥选择逻辑在无 `best_valid` 时仍接受 `best_any`：即找不到已证明不穿边的桥时，继续用“最近桥”构造 mesh。这条路径必须删除；合法桥不存在时应 hard fail，而不是把未证明合法的 topology 交给 ear-clip/star-fan。

当前测试只覆盖一个轴对齐 exterior + 两个轴对齐矩形 hole，随后仅采 64 点做 bbox 检查。没有闭合：

- 凹 exterior + 多 hole；
- 凹 hole、三个以上 hole；
- 日期线多 hole；
- MultiPolygon 中的多 hole；
- triangle 穿 hole、triangle 间 interior overlap、double-cover 的确定性证书；
- ring 相切/交叉等稳定负例。

`finish_mesh` 目前只比较三角面积总和与 polygon area；面积相等本身不能证明无重叠或无 hole 泄漏。P0-6 因此仍不通过。

## 4. 独立门禁结果

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                OK
  → 326 passed; 0 failed; 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python -m py_compile tools/validate_m4_a0_contracts.py          OK
python tools/validate_m4_a0_contracts.py                        OK
git diff --check                                                OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      OK, 1 passed
```

门禁绿说明当前已覆盖行为没有回归；它不覆盖本报告列出的生产资源和失败生命周期缺口。

## 5. A 最终裁决

本轮 **不接受 M4-A1 closeout**。下一轮只需处理本报告四个阻断：

1. streaming semantic validation + streaming SQLite inspect；
2. terminal manifest/abort forensic 全链；
3. normalized/canonical digest 的生产身份与真实跨 run 矩阵；
4. 通用、可证的多 hole topology。

已接受的 warmup、外排排序主体、WAL TRUNCATE 主路径和真实 CFSR anchor 不应重写。P1 10 万、WSL/native、三套资料完整轨迹矩阵继续等待 P0 闭合后再授权。

## 6. 约束

- 未 commit；
- 未修改 `boundary/met_path.rs`；
- 未碰外层 `origo-validation-v1.json`；
- 不宣称 M4-A1/M4 完成。
