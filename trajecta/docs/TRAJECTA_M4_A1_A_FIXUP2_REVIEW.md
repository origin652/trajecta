# Trajecta M4-A1：A 对 Fixup2 的复验裁决

状态：Fixup2 的四条主线均有实质进展，工程门禁可复现全绿；但 streaming parser 完整性、forensic 错误传播、digest 独立审计和多 hole 拓扑证书仍未闭合。**本轮仍不通过 M4-A1**。未 commit，不宣称 M4-A1/M4 完成。
日期：2026-07-23

依据：

- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/TRAJECTA_M4_A1_A_FIXUP_REVIEW.md`
- `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP2.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`

## 1. 裁决表

| 项 | A 裁决 | 说明 |
|---|---|---|
| P0-1 samples streaming | **implementation anchor accepted** | `DeserializeSeed` 已逐条消费 samples，SQLite inspect 已改为 metadata + 流式 SHA |
| P0-1 parser 完整性 | **blocking** | 未执行 EOF 检查；重复顶层字段未拒绝；records/field_sets 的 hard cap 在完整 `Vec` 分配后才检查 |
| P0-2 abort/forensic API | **partial accepted** | sink/product hook、唯一 forensic 名和正式路径 quarantine helper 已实现 |
| P0-2 terminal guarantee | **blocking** | runner 和 builder 仍用 `let _ =` 吞掉 quarantine/abort 错误；无真实 terminal-persist 故障链测试 |
| P0-3 digest 生产字段 | **implementation anchor accepted** | content / canonical SQL / canonical output 已进入 identity、manifest 和 schema |
| P0-3 独立审计与矩阵 | **rework required** | CFSR 只查长度和 SQL digest；content/canonical output 未从 on-disk bundle 独立重算；跨生产 run 仍用伪 SQL digest 单测 |
| P0-4 `best_any` | **accepted** | 未证明桥回退已删除 |
| P0-4 通用 topology | **blocking** | 仍只有代表点+总面积，没有边穿 hole、triangle overlap/double-cover 证书；缺日期线多 hole/MultiPolygon |
| 工程门禁 | **passed** | A 独立复现 328 passed / 2 ignored；真实 CFSR E2E 通过 |

## 2. 已接受的进展

### 2.1 Sample 不再全量驻留

生产 `validate_bundle_file_semantics` 已改为 serde `DeserializeSeed` / `Visitor`，`samples` 每次只反序列化一个 `BundleSampleAssignment`，仅保留前一 key、count 和当前引用。这一方向正确，旧的 sample `Vec` 阻断已消除。

`ParticleStateSqliteSink::inspect` 也已删除生产路径的 `fs::read`，改用 `metadata.len()` 和固定缓冲 `file_sha256`。该部分接受。

### 2.2 Digest 已进入生产 identity

`ProvenanceBundleIdentity` 现已包含：

- exact bundle SHA；
- exact SQLite SHA；
- normalized provenance content SHA；
- canonical SQL SHA；
- canonical output SHA。

SQLite sink 使用真实 `canonical_sql_digest(connection)`，builder 用冻结公式计算 content 和 canonical output。manifest Rust 类型、schema 与文档已同步。生产接线主体接受。

### 2.3 Forensic helper 与 multi-hole fixture 有进展

- 已增加 product/sink abort 与 quarantine hook；
- `quarantine_path` 会选择唯一后缀，并在成功后确认正式路径消失；
- multi-hole 已删除 `best_any`；
- 新增 3 holes + concave hole 正例和 outside-hole 负例。

这些都是有效进展，但还不足以满足最终 hard gate。

## 3. 仍阻断的 P0

### 3.1 Streaming validator 接受尾随 JSON 和重复字段

当前 validator：

```rust
seed.deserialize(&mut de)
```

后没有调用 `de.end()`。同时 `visit_map` 对顶层字段直接覆盖 `Option`，没有 duplicate guard。例如第二个 `schema_version` 会覆盖第一个；第二个 `samples` 也可再次被消费。

A 用随后已删除的临时集成测试实测：

```text
streaming_validator_rejects_trailing_json_value       FAILED
  validator accepted a second trailing JSON value
streaming_validator_rejects_duplicate_top_level_field FAILED
  validator accepted duplicate schema_version
```

这不是理论风险：publish 前 semantic validator 必须证明整个文件恰好是一个、且字段唯一的 bundle document。

另一个资源边界是 `records` / `field_sets` 仍通过 `map.next_value::<Vec<_>>()` 完整分配，之后才检查 expected count。合同允许它们在 hard cap 内驻留，但 cap 必须在 streaming decode 的第 `MAX+1` 项立即 hard fail，不能在任意大数组分配后才拒绝。

### 3.2 Forensic 失败仍被静默吞掉

关键路径仍存在：

```rust
let _ = output.product.quarantine_forensic();
let _ = output.product.abort();
```

`ProvenanceBundleBuilder::abort` 对 `quarantine_path` 也仍使用 `let _ =`。因此：

- `AfterRename` 后若 forensic rename 失败，正式 `provenance-bundle.json` 仍可能存在；
- terminal manifest persist 失败时，runner 仍返回原 persist error，隐藏更关键的 quarantine failure；
- 报告中的“不得留下正式 bundle”没有由控制流保证。

当前测试仍只有直接 SQLite busy pragma 和 builder BeforeRename；没有“真实 sink 已 finish → terminal manifest 第二次 persist 失败 → 文件集合”的 runner 级用例。P0-2 不能凭接口存在即签字。

### 3.3 Digest 已发布，但尚未独立证明

当前 CFSR E2E 对 `content_sha256` 和 `canonical_output_sha256` 只断言长度为 64；仅 `sqlite_sql_sha256` 使用 `ParticleStateSqliteSink::inspect` 重算。

现有跨 UUID 单测仍调用：

```rust
finalize_after_sqlite(..., &"ab".repeat(32))
```

即使用伪 SQL digest，而不是两个真实 production SQLite。当前也没有从 on-disk bundle streaming 重算 normalized content 的 inspection API。

此外 `RunManifest::validate` 只验证三项都是 SHA 字符串，没有验证：

```text
canonical_output_sha256 ==
  canonical_output_digest(sqlite_sql_sha256, content_sha256)
```

因此一个内部不一致但格式正确的 identity 会通过 manifest validation。P0-3 的生产计算主体可接受，但审计和确定性 hard gate 尚未闭合。

### 3.4 Multi-hole 的代表点检查不是 topology 证书

`finish_mesh` 当前增加：

- triangle positive excess；
- 一个代表点位于 exterior-minus-holes；
- triangle area 总和匹配 polygon area。

但一个 triangle 可以穿过凹 bay 或 hole，而它的代表点仍在合法区域；多个 triangle 也可能 overlap，同时由遗漏区域抵消总面积差。当前没有：

- triangle edge 与 exterior/hole 的 proper-intersection 检查；
- hole vertex 被 triangle 包含检查；
- triangle pair interior overlap / containment 检查；
- 日期线多 hole 和 MultiPolygon 多 hole 正例；
- ring 相切/交叉等完整负例。

代表点还是经纬度算术平均，跨日期线时不是真正球面内部点。报告已承认未做通用 CDT；但 Case schema 并未缩窄为“仅当前 fixture 集”，因此该项仍是 M4-A1 P0。

## 4. 独立门禁

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                OK
  → 328 passed; 0 failed; 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python -m py_compile tools/validate_m4_a0_contracts.py          OK
python tools/validate_m4_a0_contracts.py                        OK
git diff --check                                                OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      OK, 1 passed, 8.08s
```

门禁全绿说明既有测试没有回归；A 的两个临时 parser 负例证明当前 328 tests 尚未覆盖关键合同边界。

## 5. A 最终裁决

本轮仍不接受 M4-A1 closeout。Fixup3 只需处理：

1. parser EOF、duplicate field 和 records/field_sets decode cap；
2. forensic/abort 错误传播及真实 runner terminal-persist 故障链；
3. on-disk digest 独立重算、identity 公式校验和真实跨 run 小矩阵；
4. multi-hole edge/overlap topology validator 与缺失 fixture。

不重写已接受的 external sort、WAL、warmup、digest 公式、unique forensic naming 或 `best_any` 删除。

## 6. 约束

- 未 commit；
- 未修改 `boundary/met_path.rs`；
- 未碰外层 `origo-validation-v1.json`；
- 不宣称 M4-A1/M4 完成。
