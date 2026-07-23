# Trajecta M4-A1：A 对 Fixup4 的复验裁决

状态：Fixup4 已修复 terminal Failed lifecycle 的上一轮确定性 bug，并补上真实 SQLite terminal-persist 故障链；工程门禁也可独立复现全绿。但 exterior topology 仍有可复现反例，production digest 测试没有执行冻结的 1/4 worker、逆排列、不同 chunk 矩阵，forensic rename-failure 与日期线多 hole fixture 也尚未交付。**本轮仍不通过 M4-A1**。未 commit，不宣称 M4-A1/M4 完成。

日期：2026-07-23

依据：

- `docs/TRAJECTA_M4_A1_A_FIXUP3_REVIEW.md`
- `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP4.md`
- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`

## 1. 裁决表

| 项 | A 裁决 | 说明 |
|---|---|---|
| Terminal Failed lifecycle | **accepted** | terminal 前冻结 clock；任意较早失败都会补合法 `finished_at`；上一轮 `InvalidLifecycle` 反例已闭合 |
| 真实 SQLite terminal-persist 链 | **partial accepted** | 正式 bundle 会 quarantine，Failed manifest 合法；但 `.1` 冲突与 quarantine rename failure 未实测 |
| Exterior topology | **blocking** | `edge_exits_polygon` 仍是固定 11 点采样；“两端点在 exterior”被错误当作 boundary edge，A 新反例被接受 |
| On-disk digest 两真实 run | **implementation accepted** | 两次真实 `build_runner` 均从磁盘重算五类 digest，UUID 归一化主体有效 |
| Production determinism matrix | **blocking / incomplete** | 两次均为 1 worker、相同顺序、相同 production chunk；不是冻结的 1/4 worker + inverse + chunk 矩阵 |
| Parser implementation | **accepted** | duplicate/cap/parser 主体不变且可接受 |
| Parser 新 coverage | **partial** | duplicate 用 `null` 注入，可能在 duplicate guard 前因类型错误失败；cap+1 与 missing field 未补 |
| 工程门禁 | **passed** | A 独立复现 339 passed / 2 ignored；真实 CFSR E2E 通过 |

## 2. 已接受项

### 2.1 Terminal lifecycle 的上一轮 bug 已修复

`finalize_success` 现在先取得 terminal clock；若 clock 失败，以 `started_at` 作为生命周期合法 fallback，同时保留 clock error。其后 contribute、termination、Complete persist 任一步失败，错误分支都会设置：

- `status = Failed`；
- `finished_at = Some(...)` 且不早于 `started_at`；
- `failure = Some(...)`；
- `provenance = None`。

`contribute_manifest_failure_yields_legal_failed_manifest` 已覆盖 A 上轮的确定性反例，Failed manifest 能通过 `validate()` 并进入 manifest store。该实现接受。

### 2.2 真实 SQLite terminal-persist 主链有效

`real_sqlite_terminal_persist_fail_quarantines_bundle` 使用真实 `build_runner` 和 `ParticleStateSqliteSink`，第二次 manifest persist 强制失败。测试确认：

- 内存 manifest 为合法 Failed；
- 正式 `provenance-bundle.json` 不存在；
- forensic bundle 存在；
- spool/tmp 无残留；
- 磁盘没有 Complete manifest。

这条真实链本身接受。

### 2.3 两次真实 run 的 on-disk digest 审计有效

`production_digest_matrix_two_runs_content_and_sql_match` 已不再传伪 SQL SHA，并对两个真实 runner 的磁盘 bundle/SQLite 重算：

- exact bundle SHA；
- exact SQLite SHA；
- normalized content SHA；
- canonical SQL SHA；
- canonical output SHA。

两个 UUID 不同但科学内容相同的 run，其 content/SQL/canonical output 相同。该部分证明了 run-id 归一化与磁盘审计主体。

## 3. 仍阻断的 P0

### 3.1 Exterior topology 仍不是确定性证书

当前实现有两个相关问题。

第一，`edge_exits_polygon` 沿球面边只检查固定 11 个内点：

```rust
for k in 1..12 { ... }
```

这是固定探针，不是 topology certificate；窄小的出域区间可以落在探针之间。Fixup4 prompt 已明确要求不要用更多采样代替边界关系证明。

第二，代码使用：

```rust
let boundary_edge = point_on_ring_edges(a, exterior)
    && point_on_ring_edges(b, exterior);
```

“两个端点都在 exterior 上”不等于整条弦沿 exterior。此时 `edge_exits_polygon` 被跳过，proper crossing 分支也因 `boundary_edge` 为真而不拒绝。

A 用随后已删除的临时单测复现：

```text
exterior C-shell:
  (-20,0) (5,0) (5,1) (1,1) (1,4)
  (5,4) (5,5) (-20,5) (-20,0)

bad triangle:
  (5,1) (1,4) (-20,2.5)
```

三个 triangle 顶点全部位于 exterior，但连接它们的弦穿过 C-shell 凹口。当前 `validate_mesh_topology` 返回 `Ok`，临时测试失败为：

```text
validator accepted chords crossing the concave exterior bay
```

因此 `rejects_c_shell_triangle_with_edge_exiting_exterior` 只闭合了一个端点不全在边界上的反例，未闭合一般情况。

Fixup4 仍未加入 prompt 中冻结的：

- 日期线 shell + 两个 holes；
- MultiPolygon，其中至少一个 component 含两个 holes；
- ring 起点/方向/hole 排列变化后的 canonical determinism；
- exterior partial overlap 与 triangle containment 的人工坏 mesh。

### 3.2 所谓 production digest “矩阵”实际没有变更执行维度

`base_profile` 固定：

```rust
worker_threads: 1
```

`production_digest_matrix_two_runs_content_and_sql_match` 两次都直接调用同一个 `base_profile`，没有把第二次改为 4 workers；两次 particle/input order 相同，也没有切换 provenance spool chunk。测试也没有断言两个 UUID 不同导致 exact bundle SHA 确实不同，且没有“改一个 SQL/sample assignment → canonical output 改变”的 production 敏感性负例。

所以该测试应准确命名为“两真实 UUID run 的 normalized digest 稳定性”，不能当作 Fixup4 prompt 冻结的 production matrix。closeout 前仍须补：

| run | workers | order | chunk |
|---|---:|---|---:|
| A | 1 | identity | A |
| B | 4 | inverse/permuted | B |

两边都必须走真实 SQLite/bundle 路径并从磁盘独立重算。

### 3.3 Forensic 冲突与 rename failure 尚未实测

真实 harness 中的注释写“Pre-place forensic name”，但实际代码只是：

```rust
let run_root_placeholder = dir.path().join("_will_find_run_dir");
let _ = run_root_placeholder;
```

没有创建 `provenance-bundle.json.forensic-aborted`。最终断言也是 `forensic_0 || forensic_1`，所以没有证明正式链会保留旧 forensic 并选择 `.1`。

同时没有注入 quarantine rename failure，故 `primary terminal persist + quarantine failure` 的错误聚合仍只由静态代码支持，没有真实 sink/runner 证据。该项保持 rework required。

## 4. P1 测试覆盖问题

`streaming_validator_table_driven_duplicates_and_truncation` 对每个字段插入：

```text
"field": null,
```

对 String/object/array 字段，parser 可能在第一个值就因类型不匹配失败，尚未走到第二个同名字段，因此该测试不能证明 duplicate guard 被触发。应插入同字段的一个**有效值**并断言错误含 `duplicate <field>`。

上一轮要求的 records/field_sets `cap + 1` 和 missing-field 负例也未加入。实现主体已接受，这里只需补测试，不需重写 parser。

## 5. A 独立门禁

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                OK
  → 339 passed; 0 failed; 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python -m py_compile tools/validate_m4_a0_contracts.py          OK
python tools/validate_m4_a0_contracts.py                        OK
git diff --check                                                OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      OK, 1 passed, 7.94s
```

A 的临时 topology 反例已经删除，正式源码未留下诊断代码。

## 6. A 最终裁决

Fixup4 **不通过 M4-A1 closeout**。Fixup5 只需处理四个小而明确的洞：

1. 删除固定 11 点 `edge_exits_polygon` 证书，按所有边界交点分段并证明每个开区间归属；不得把“两个端点 on-ring”当作 boundary edge；
2. 补日期线双 hole、MultiPolygon 多 hole 与 A 新 C-shell boundary-chord 反例；
3. 真正执行 1/4 worker、逆排列、不同 chunk 的两 run production digest 矩阵；
4. 真实 terminal harness 中实际预放 forensic 名并注入 quarantine rename failure；同时修正 parser duplicate 测试。

Terminal lifecycle、on-disk digest 主体、external sort/WAL/warmup/CFSR E2E 等已接受内容不要重写。

## 7. 约束

- 未 commit；
- 未修改 `crates/trajecta-core/src/boundary/met_path.rs`；
- 未碰外层 `origo-validation-v1.json`；
- 不宣称 M4-A1/M4 完成。
