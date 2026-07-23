# Trajecta M4-A1：A 对 B closeout 的复验裁决

状态：warmup 通过；真实 CFSR 小样本 bundle 内容通过独立重算；provenance 工程合同与通用多 hole 仍有 P0；未 commit，不宣称 M4-A1/M4 完成。
日期：2026-07-22

## 1. 裁决表

| 项 | A 裁决 | 说明 |
|---|---|---|
| P0-1 warmup | **accepted** | derived dependency closure 与逐 domain/profile 选择成立 |
| P0-2 bundle 小样本内容 | **accepted as anchor only** | CFSR artifact 的 schema、SHA、引用与 SQLite 覆盖独立重算通过 |
| P0-2 bundle 通用实现 | **rework required** | 仍存在无界内存、WAL busy、runtime 语义校验、失败清理和 digest 缺口 |
| P0-3 多 hole | **blocking** | 合法 Polygon 多 hole 仍可能 mesh 失败，不能降为 P1 |
| P1 完整矩阵 | **pending** | 等 P0 闭合后再跑 |
| B 报告门禁 | **not reproducible as written** | workspace tests/clippy/doc 通过，但 `cargo fmt --check` 当前失败 |

## 2. 已通过部分

### 2.1 Warmup

A 接受当前实现：

- 使用正式 `parse_expression`；
- capability target 映射到唯一 direct/derived id；
- 递归 identifier closure；
- unknown id、重复 target、缺历史 hard fail；
- 每个 domain 只绑定一个 profile，并独立计算 warmup；
- exact-hit guard 与 warmup 叠加语义保留；
- forward/backward 选择同一时间集合；
- CFSR 03 UTC 仍选择 00/06。

### 2.2 真实 CFSR bundle anchor

A 强制重跑真实 CFSR E2E，结果 `1 passed`，约 8 秒。另截取一次实际产物并使用独立 Python 路径重算：

```text
records      10
field_sets    2
samples       4
sqlite rows   4
bundle SHA    fd09eb4295b1be845025af532c89e35b498f9540a9cf6f1bc80ce341cd911446
sqlite SHA    9e5c4e97dd7f5ca47ec8dda0a0993952c4cf6609f94b9e76607bf855e8a605a2
```

该截取样本通过：Draft 2020-12 schema、record/field-set canonical SHA、slot→record.field、sample 排序/唯一性、SQLite 主键全覆盖、非 NULL SQL→非 NULL provenance、quality 对齐和 `integrity_check=ok`。

这些 SHA 只对应本次临时运行，不是冻结 fixture。

## 3. Provenance 仍需返工的 P0

### 3.1 假 spool：生产 finalize 仍无界

当前实现虽然先写 spool，但 finalize 随后：

- `load_spool_samples -> Vec`；
- 全量 `sort`；
- SQLite keys 与 non-null flags 全量 `Vec/BTreeMap`；
- `ProvenanceBundleDocument.samples` 全量驻留；
- `serde_json::to_vec_pretty` 再复制完整 bundle；
- SQLite SHA 使用 `fs::read` 全文件。

这违反 10 万粒子所需的有界流式合同。必须使用有界 sorted runs、多轮 fan-in merge、SQLite ordered cursor lockstep 校验、流式 JSON writer 和流式 SHA。

### 3.2 WAL checkpoint 未证明最终主文件完整

`PRAGMA wal_checkpoint(FULL)` 当前丢弃返回的 `busy/log/checkpointed`。存在 live reader 时，SQLite 可以正常返回 busy 而不是 Rust error；此时直接关闭并 hash `particles.sqlite` 可能漏掉仍在 WAL 的页。

终态必须使用可验证的 checkpoint（建议 `TRUNCATE`），检查 `busy == 0`，关闭 writer 后确认无非空 WAL，再计算主文件 SHA；否则 hard fail。

### 3.3 self-parse 不是 schema/semantic validation

当前 atomic replace 前只做 serde round-trip、counts 和内部 hashes，没有完整执行 v1 约束。例如 lowercase SHA、非空且唯一 sources、非空 operation/parameter/fallback、排序、quality/validity 对齐等未全部在 production validator 中证明。

应建立无文件系统副作用的 `validate_bundle_semantics`，并在 replace 前执行；JSON Schema 继续作为独立机器门禁。

### 3.4 失败路径没有 RAII 清理

当前 builder 没有 Drop/abort cleanup。event/transaction、checkpoint、bundle temp、atomic rename 或 manifest 持久化失败时，spool/tmp/final artifact 的状态未形成可测试合同。

失败不得发布 complete manifest；本轮 `.tmp`/spool 必须清理，并补 atomic replace、checkpoint busy、manifest persist failure 注入。

### 3.5 确定性摘要合同勘误

bundle 含随机 UUID，SQLite 也保存 run UUID，因此两个独立 production run 的 exact bytes/SHA 天然不同。此前“1/4 worker 独立 run exact artifact SHA 相同”的文字不可实现。

A 已在 `TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md` 冻结：

- exact manifest SHA：单次运行审计；
- normalized provenance content digest：排除 run-scoped identity；
- canonical output digest：canonical SQL digest + normalized provenance digest。

B 需按公式实现；不得用“从 SQL digest 删除 provenance_id”冒充已经纳入 provenance。

## 4. 多 hole 裁决

多 hole 仍是 M4-A1 P0 阻断。当前测试明确允许 `mesh_polygon_region` 返回错误，因而只证明 canonical area，不证明公开采样支持。

理由：Case/GeoJSON 合同允许 Polygon/MultiPolygon，未把多个 hole 声明为不支持。一个合法输入在 release 时偶发失败，表示普通 release 文件级闭环仍不完整。必须使用可靠约束三角化，或由 A 正式缩窄 schema；本轮不接受把它降为 P1。

## 5. 本轮复验门禁

```text
cargo fmt --all -- --check                                      FAILED
  release/geometry.rs:2492 仅需 rustfmt 机械换行
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                319 passed, 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python tools/validate_m4_a0_contracts.py                         OK
git diff --check                                                 OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      1 passed
independent CFSR bundle/SQLite recomputation                    passed
```

B 报告中的“fmt OK”与当前工作树不一致，因此本轮总体门禁不是全绿。下一轮说明见 `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP.md`。

## 6. 明确未宣称

- 未宣称 provenance 通用实现通过；
- 未宣称多 hole 通过；
- 未宣称 M4-A1/M4 完成；
- 未运行 10 万、WSL、native 或三套资料完整 trajectory；
- 未 commit；未碰 `origo-validation-v1.json`。
