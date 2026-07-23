# Trajecta M4-A1：A 对 Fixup3 的复验裁决

状态：Fixup3 的 streaming parser 与 on-disk digest 主体可接受，工程门禁也可独立复现；但 terminal failure lifecycle 与 mesh exterior topology 均有可复现 P0 反例，真实 production digest 矩阵仍未完成。**本轮不通过 M4-A1**。未 commit，不宣称 M4-A1/M4 完成。

日期：2026-07-23

依据：

- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/TRAJECTA_M4_A1_A_FIXUP2_REVIEW.md`
- `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP3.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`

## 1. 裁决表

| 项 | A 裁决 | 说明 |
|---|---|---|
| P0-1 streaming parser | **accepted** | `de.end()`、顶层 duplicate guard、capped records/field_sets、逐项 samples 与 `BundleValidationSummary` 已接通 |
| P0-2 abort/quarantine API | **partial accepted** | `abort() -> Result`、runner 聚合错误和二次 persist 错误传播均有实质改进 |
| P0-2 terminal guarantee | **blocking** | terminal 较早失败时未补 `finished_at`，Failed manifest 自身校验失败；真实 SQLite sink × 第二次 persist 故障链仍缺 |
| P0-3 on-disk digest | **implementation accepted** | finalize 与 CFSR E2E 已从磁盘流重算 content/canonical output；manifest 公式校验已接 |
| P0-3 production determinism matrix | **rework required** | 跨 run 测试仍传伪 SQL SHA，未交两个真实 sink/runner 的 1/4 worker、chunk、逆排列矩阵 |
| P0-4 topology | **blocking** | validator 明确跳过 triangle edge × exterior edge；A 的凹外壳反例被错误接受 |
| 工程门禁 | **passed** | A 独立复现 332 passed / 2 ignored，真实 CFSR E2E 通过 |

## 2. 已接受项

### 2.1 Streaming parser 主体闭合

`provenance_bundle.rs` 当前具备：

- 单文档完成后 `de.end()`，拒绝第二个 JSON value 与尾随垃圾；
- `schema_version/run_id/sqlite/record_hash_algorithm/records/field_sets/samples` duplicate guard；
- records/field_sets 通过 capped `DeserializeSeed` 在 `MAX+1` 拒绝，而非先构造任意大数组；
- samples 逐项消费，不返回 `Vec`；
- `BundleValidationSummary` 只返回 counts 与从实际磁盘内容流式重算的 `content_sha256`。

新增的 trailing JSON、trailing garbage、duplicate schema_version 负例均通过。实现锚点接受；Fixup4 不应重写 parser 主体。

### 2.2 On-disk digest 主体闭合

已确认：

- bundle finalize 使用 tmp 文件 semantic streaming validator 的 content digest；
- `RunManifest::validate` 重算并校验 `canonical_output_digest(sqlite_sql_sha256, content_sha256)`；
- CFSR E2E 从磁盘 bundle 重算 content，再重算 canonical output，与 manifest 对比。

这些生产接线接受。剩余问题是明确要求的真实跨 run 确定性矩阵没有交付，而不是公式主体错误。

### 2.3 Error propagation 有效进展

`ProvenanceBundleBuilder::abort`、runner `abort_outputs` / `quarantine_outputs` 已返回并聚合错误；failed-manifest 二次 persist 也不再被静默丢弃。这一方向接受。

## 3. P0 阻断

### 3.1 Terminal 较早失败无法持久化合法 Failed manifest

根因是 `finalize_success` 中 `finished_at` 只在以下步骤之后赋值：

1. 所有 product `contribute_manifest`；
2. termination summary；
3. Complete / CompletedWithParticleErrors 状态选择。

若前两步任一步失败，错误分支会把状态改为 Failed 并设置 failure，但没有补 `finished_at`。而 `RunManifest::validate` 明确要求 Failed 同时具备 `finished_at` 与 `failure`。

A 用随后已删除的临时 runner 单测注入 `contribute_manifest = Err(Io(...))`，实际得到：

```text
failed terminal manifest is missing finished_at
terminal failed: Output("Io(\"forced contribute failure\")");
failed-manifest persist also failed: Manifest("InvalidLifecycle")
```

因此报告中的“terminal 全路径 quarantine + Failed persist”尚不成立。必须保证 contribute、termination、clock、manifest validation、terminal persist 中任何一步失败后，都能形成一个生命周期合法的 Failed manifest；若连 failure timestamp 获取也失败，需要冻结可审计的 fallback 与错误聚合规则。

此外，Fixup3 明确承认没有加入上一轮要求的真实 `ParticleStateSqliteSink`：

```text
running persist 成功 → sink finish 发布正式 bundle → terminal persist 强制失败
```

这一 harness 仍是 P0，不能由 builder 单测替代。

### 3.2 Topology validator 接受穿出凹 exterior 的 triangle

`validate_mesh_topology` 当前注释和实现都明确：只构造 `hole_edges`，不检查 triangle edge 与 exterior edge 的 proper intersection。代表点在 exterior-minus-holes 内并不能证明整条 triangle 在域内。

A 用随后已删除的临时单测复现：

```text
exterior = C-shaped shell:
  (0,0) (5,0) (5,1) (1,1) (1,4) (5,4) (5,5) (0,5)

bad triangle:
  (0.1,0.1) (4.9,0.5) (0.1,1.5)
```

该 triangle 的球面代表点在合法区域，但一条边穿过 C 形凹口。当前 validator 返回 `Ok`，测试失败为：

```text
topology validator accepted a triangle crossing the exterior boundary
```

所以 P0-4 的“passed on current path”不成立。需要对 exterior/hole/triangle edge 做确定性的关系分类：只允许完全共享边和合法端点接触，拒绝 proper crossing 与部分共线重叠；不能继续整体跳过 exterior。

仍缺的验收 fixture 也没有交付：

- 日期线 shell + 至少两个 holes；
- MultiPolygon，其中至少一个 component 含多个 holes；
- 人工 bad mesh：exterior crossing、partial overlap、triangle overlap/containment；
- ring 起点/方向/hole 排列变化后的 canonical determinism。

### 3.3 真实 production digest 矩阵仍缺

`normalized_digest_stable_across_run_ids` 仍调用：

```rust
finalize_after_sqlite(..., &"ab".repeat(32))
```

它证明 normalized bundle content 对 run_id 稳定，但没有证明两个真实 production SQLite 的 canonical SQL 与 canonical output 一致。仓库中也没有 1 vs 4 workers、不同 chunk、输入逆排列的两个真实 sink/runner 矩阵。

该项不推翻已接受的 digest 实现，但必须在 closeout 前用小型 production path 补齐；不要跑 10 万。

## 4. A 独立门禁

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                OK
  → 332 passed; 0 failed; 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python -m py_compile tools/validate_m4_a0_contracts.py          OK
python tools/validate_m4_a0_contracts.py                        OK
git diff --check                                                OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      OK, 1 passed, 11.48s
```

两条 A 临时反例测试均已删除；正式源码未留下诊断代码。门禁全绿说明既有测试未回归，但不能覆盖上述缺失 hard gate。

## 5. A 最终裁决

Fixup3 **不通过 M4-A1 closeout**。Fixup4 只需集中处理：

1. terminal 任意阶段失败都能持久化生命周期合法的 Failed manifest，并加入真实 SQLite terminal-persist harness；
2. topology 不再跳过 exterior edge，冻结 A 的 C-shaped bad triangle，并补日期线多 hole / MultiPolygon 多 hole；
3. 用两个真实小型 sink/runner 补 production digest 确定性矩阵。

Parser、on-disk digest 公式、external sort、WAL、warmup、CFSR E2E 等已接受主体不要重写。

## 6. 约束

- 未 commit；
- 未修改 `crates/trajecta-core/src/boundary/met_path.rs`；
- 未碰外层 `origo-validation-v1.json`；
- 不宣称 M4-A1/M4 完成。
