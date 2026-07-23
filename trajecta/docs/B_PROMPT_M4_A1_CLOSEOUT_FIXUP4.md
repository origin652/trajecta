# 给 B：M4-A1 closeout Fixup4（只修三个剩余验收洞）

你是 B 模型。本轮完整阅读：

- `docs/TRAJECTA_M4_A1_A_FIXUP3_REVIEW.md`
- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_RUN_MANIFEST.schema.json`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`

保留全部现有未提交改动；不修改 `crates/trajecta-core/src/boundary/met_path.rs`；不碰外层 `origo-validation-v1.json`；不 commit；不宣称 M4-A1/M4 完成。

A 已接受 streaming parser 主体、on-disk content/canonical-output 重算、manifest 公式校验、abort/quarantine 返回 `Result`、external sort/WAL/warmup/CFSR E2E。不要重写这些主体。

## P0-1：修 terminal failure lifecycle，并加入真实 runner harness

### 1. 所有 terminal 失败都必须得到合法 Failed manifest

当前 `finalize_success` 在 `contribute_manifest` 或 termination summary 失败时，`finished_at` 尚未设置；错误分支随后持久化 Failed manifest 会得到 `Manifest("InvalidLifecycle")`。

修复要求：

1. 冻结 terminal timestamp 的取得与 fallback 规则，使 contribute、termination、clock、manifest validation、terminal persist 任一步失败后，Failed manifest 都满足：
   - `status = Failed`；
   - `finished_at = Some(...)` 且不早于 `started_at`；
   - `failure = Some(...)`；
   - `provenance = None`；
2. lifecycle clock 自身失败不得被吞掉；将 clock、primary terminal、quarantine/abort、failed-manifest persist 错误按固定优先级聚合；
3. quarantine/abort 失败不得覆盖 primary，也不得错误宣称已清理；
4. 正式 complete manifest 不得残留；失败 manifest 若无法 persist，错误必须明确包含原因；
5. 关键路径继续禁止 `let _ =`。

冻结最小单测：

- output `contribute_manifest` 强制失败；断言内存 manifest 是生命周期合法的 Failed，并且 store 最后收到 Failed；
- termination summary 或 lifecycle clock 注入失败；断言同上，且错误信息含 primary；
- failed-manifest persist 再失败；断言同时含 primary + second persist。

### 2. 加上一轮明确要求的真实 SQLite 故障链

必须使用真实 `ParticleStateSqliteSink` / `ParticleStateProduct`，不是只测 builder：

1. running manifest 第一次 persist 成功；
2. 小型 simulation 正常写 SQLite、checkpoint、发布正式 provenance bundle；
3. manifest store 在 terminal persist 强制失败；
4. runner 走 quarantine + abort；
5. 断言：
   - 正式 `provenance-bundle.json` 不存在；
   - 唯一 forensic 文件存在；预放 `.forensic-aborted` 时选择 `.1`；
   - tmp/spool/run 文件全部不存在；
   - 磁盘没有 Complete manifest；
   - 内存 manifest 为合法 Failed，且 provenance 为 None；
6. 再注入 quarantine rename failure，断言返回错误同时包含 terminal persist 与 quarantine failure；不得只返回原 persist error。

保持 fixture 很小，不跑 10 万。

## P0-2：补齐 exterior topology 证书

当前 validator 整体跳过 triangle edge × exterior edge，这是确定性漏洞。不要用更多随机采样、代表点或放宽面积容差修绿。

### 必须实现

1. 构造 exterior edges 与 hole edges；
2. 对 triangle edge × ring edge 做球面 segment relation 分类，至少区分：
   - disjoint；
   - exact shared full edge；
   - endpoint-only touch；
   - proper crossing；
   - partial collinear overlap；
3. 只允许 exact shared full edge 与拓扑合法的 endpoint-only touch；
4. proper crossing、partial overlap 一律 `InvalidGeometry`；
5. keyhole bridge 的共享边必须通过“明确分类/明确允许”处理，不得以跳过整个 exterior 代替；
6. 保留现有 hole vertex intrusion、triangle-triangle crossing/containment、positive excess 与面积守恒；
7. 若现有 keyhole/ear-clip 算法无法对所有合法 fixture 给出证书，保留最小反例并报告 blocked，不得降级 gate。

冻结 A 的负例：

```text
exterior C shell:
  (0,0) (5,0) (5,1) (1,1) (1,4) (5,4) (5,5) (0,5) (0,0)

bad triangle:
  (0.1,0.1) (4.9,0.5) (0.1,1.5)
```

它的代表点在域内，但边穿出 exterior；`validate_mesh_topology` 必须拒绝。

补齐测试：

- artificial bad mesh：exterior proper crossing；
- exterior partial overlap（不是 exact shared full edge）；
- triangle overlap / containment；
- 日期线 shell + 两个 holes 的 public canonicalize + sampling；
- MultiPolygon，至少一个 component 含两个 holes；
- ring 起点、方向、hole 输入排列变化后的 canonical mesh/digest 确定性；
- 合法共享 ring edge / endpoint touch 正例，防止把正常 constrained mesh 全误拒。

## P0-3：真实 production digest 小矩阵

不要再用 `&"ab".repeat(32)` 证明 production determinism。至少运行两个真实 `ParticleStateSqliteSink` 或完整小型 runner：

| run | workers | particle/input order | spool chunk | run_id |
|---|---:|---|---:|---|
| A | 1 | identity | small A | UUID A |
| B | 4 | inverse/permuted | small B | UUID B |

科学内容保持相同，断言：

- exact bundle SHA：因 run_id 不同允许/要求不同；
- exact SQLite SHA：可因 representation identity 不同而不同，按合同记录；
- normalized `content_sha256`：相同；
- `sqlite_sql_sha256`：相同；
- `canonical_output_sha256`：相同；
- 两边都从实际磁盘 bundle + SQLite 独立重算，再与各自 manifest 五字段逐项比较。

再做最小敏感性负例：改一个 SQL 数值或一个 sample assignment，canonical output 必须变化。

矩阵保持小型；本轮不跑 10 万、WSL/native/三套资料长矩阵。

## P1：Parser 测试覆盖补齐，不重写实现

现有实现已有全部 duplicate guard，但正式测试只明确覆盖 duplicate schema_version。用 table-driven 小测试补：

- 其余顶层字段 duplicate；
- records / field_sets 在 cap+1 拒绝；
- truncated JSON；
- unknown / missing field。

这部分只补 coverage，不重写 parser。

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

更新 `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，必须列出：

- terminal 三类故障注入的实际错误与最终 manifest 状态；
- 真实 SQLite terminal-persist 故障后的实际文件集合；
- A 的 C-shaped bad triangle 测试名与结果；
- 日期线双 hole / MultiPolygon 多 hole fixture；
- 两个真实 production run 的五类 digest 表；
- 所有未闭合项标 blocked。
