# Trajecta M3 B1 返工交付报告（工程收尾）

**状态：A 指出的主体缺口 + 三项工程收尾已修；不宣称 M3 完成；未 git commit。**

## 本轮收尾（A 最新要求）

| 缺口 | 修复 |
|---|---|
| 外排归并 O(run_count) FD | **固定 fan-in `MERGE_FAN_IN=32` 多轮归并**；`TempRunSet` RAII 清临时 run；峰值打开文件 O(32) |
| 下载器哈希失败不可自愈 | 完整性失败 / 416 → **删 `.part` 全量重下**；坏正式文件 `ensure_valid_existing` 删除后重下；`--self-test` 本地 HTTP 覆盖 |
| stdin 测绕过 spool | 抽出 `spool_jsonl_for_coverage` 单测；**集成测 `replay_stdin_without_coverage_spools_then_second_pass` 不传 coverage** |

## 既有已确认项（保持）

- candidate glob 区分 pgbl / 其他产品；截断目标 pgbl 硬失败
- 混合目录 CLI + stderr `kind=lock_note`
- epoch 0、多 unique-time、跨平台资料路径
- UniqueTimeTracker 分块读、不 `read_to_end`

## 门禁（本机）

```text
cargo fmt --all --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace                205 passed (~40 s)
cargo doc --offline --workspace --no-deps       OK

python tools/fetch_cfsr_pgbl.py --self-test     OK

TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met \
  --test real_cfsr_pipeline --test real_m3_query_chain --test real_met_manifest
  → 8 passed

TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-cli --test met_cli_contracts
  → 13 passed
```

## 关键实现要点

### UniqueTimeTracker
- 初始 run：`STREAM_CHUNK_POINTS` 有界读
- 归并：每轮 `chunks(MERGE_FAN_IN)`，k-way heap，中间层 dedup
- `TempRunSet::Drop` 保证异常路径也删临时文件

### fetch_cfsr_pgbl.py
- `IntegrityError` → unlink `.part`，`existing=0`，下一轮完整 GET
- HTTP 416 同路径
- 已有 dest size/sha 不匹配 → 删除后重下
- `python tools/fetch_cfsr_pgbl.py --self-test`：本地 `ThreadingHTTPServer` 验证 200/206/416/坏哈希/坏 dest

### stdin / coverage spool
- `spool_jsonl_for_coverage(&mut dyn BufRead)`：单遍 spool + min/max
- 无 coverage 时 CLI 走 spool，再二次读 spool（不回读 stdin）
- 单元测 + 真实 CFSR 集成测（无 `--coverage-*`）

## 外部阻断（可信）

NCEI 12z 官方 URL 在本环境 TLS/握手失败；未关闭证书校验。

## 关键文件

- `crates/trajecta-cli/src/command/met.rs` — fan-in 外排、spool 抽取
- `crates/trajecta-cli/tests/met_cli_contracts.rs` — 无 coverage stdin
- `tools/fetch_cfsr_pgbl.py` — 完整性自愈 + self-test
- `crates/trajecta-met/...` — candidate glob / 硬失败（既有）

## 非宣称

- 不宣称 M3 完成
- 不宣称三时次 CFSR / ERA5 / native / 百万点完成
- 无 git commit
