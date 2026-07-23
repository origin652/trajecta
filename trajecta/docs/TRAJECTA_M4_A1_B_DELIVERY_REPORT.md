# Trajecta M4-A1：B fixup5 交付报告

状态：按 `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP5.md` 修四个精确验收洞；本机门禁全绿；**未 commit**；**不宣称 M4-A1/M4 完成**。
日期：2026-07-23
依据：`docs/TRAJECTA_M4_A1_A_FIXUP4_REVIEW.md`

---

## 总表

| 项 | 状态 | 说明 |
|---|---|---|
| P0-1 exterior 分段拓扑证明 | **implemented / executed / passed** | 删固定 11 点证书；交点 fraction 开区间 membership；禁止“两端点 on-ring ⇒ boundary” |
| P0-2 production digest A/B 矩阵 | **implemented / executed / passed** | workers 1 vs 4、reverse scan、chunk 3 vs 7；五类 digest 磁盘重算 |
| P0-3 forensic `.1` + quarantine rename fail | **implemented / executed / passed** | 预放旧 forensic；断言 `.1`；rename fault 聚合错误 |
| P1 parser valid-duplicate / cap+1 / missing | **implemented / executed / passed** | 真 duplicate 文案；records/field_sets cap+1；missing 字段 |
| A `met_path` | **未修改** | — |
| `origo-validation-v1.json` | **未碰** | — |
| commit / complete claim | **无** | — |

---

## P0-1 Exterior topology certificate

### 实现
- `certify_triangle_edge_in_region`：与 exterior+hole 求 segment relations，收集 0/1/交点 fraction，开区间中点 membership
- boundary-covered 仅当 collinear ring edge（链）完整覆盖区间；**端点 on-ring 不够**
- 删除作为证书的固定 11 点 `edge_exits_polygon`（仅留 diagnostic 死代码路径名）

### 测试名
| 测试 | 结果 |
|---|---|
| `rejects_c_shell_boundary_chord_with_vertices_on_exterior` | **passed**（A 冻结反例 `(5,1)(1,4)(-20,2.5)`） |
| `rejects_c_shell_triangle_with_edge_exiting_exterior` | **passed**（Fixup4 反例保留） |
| `accepts_shared_exterior_edge_on_simple_triangle` | **passed** |
| `accepts_boundary_sub_edge_and_endpoint_only_touch` | **passed** |
| `rejects_artificial_exterior_partial_overlap_and_triangle_containment` | **passed** |
| `dateline_shell_with_two_holes_canonicalize_mesh_sample` | **passed**（half-shell MultiPolygon 成功路径） |
| `multipolygon_component_with_two_holes_mesh` | **passed** |
| `ring_rotation_and_hole_permutation_keep_canonical_geometry_sha` | **passed** |

### blocked / partial
- **unsplit** 单 Polygon 跨日界线 + 双 hole 的 keyhole mesh：诚实 `InvalidGeometry`（测试接受 Ok 且面积达标 **或** InvalidGeometry）；**不 silent-pass**。成功路径为 split 后的双 half-shell MultiPolygon。

---

## P0-2 Production digest 矩阵

### 测试
`production_digest_matrix_workers_order_chunk`

| run | workers | order | chunk | run_id |
|---|---:|---|---:|---|
| A | 1 | identity | 3 | UUID A |
| B | 4 | reverse particle scan | 7 | UUID B |

### 断言（磁盘独立重算）
- UUID A ≠ UUID B
- exact bundle SHA 因 run_id 不同而不同
- normalized **content** 相同
- canonical **SQL** 相同
- **canonical output** 相同
- exact SQLite SHA 仅记录，不预设

### 敏感性负例
`production_canonical_output_sensitive_to_sample_change`：篡改 sample `field_set_sha256` → validator fail 或 content/canon 变化。

保留 `production_digest_matrix_two_runs_content_and_sql_match` 作为 UUID 归一化稳定性探针。

---

## P0-3 Forensic 冲突与 quarantine failure

### `real_sqlite_terminal_persist_fail_quarantines_bundle`
1. build 后定位真实 run dir
2. **预写** `provenance-bundle.json.forensic-aborted` = `FROZEN-OLD-FORENSIC-v1`
3. terminal persist 失败后：
   - 旧 forensic **内容未覆盖**
   - 新 forensic = **`.forensic-aborted.1`**（精确，非 `||`）
   - 无正式 bundle；无 spool/tmp

### `real_sqlite_terminal_persist_and_quarantine_rename_both_fail`
- `arm_quarantine_rename_fault()` 注入 rename 失败（默认 off）
- 错误同时含 primary persist 与 quarantine/forensic rename
- **正式 bundle 仍在盘**（与错误一致，不谎称已清理）
- Failed manifest 合法

### 聚合错误原文模式
```text
terminal failed: ...forced terminal persist failure...; then quarantine failed: ...forensic rename: fault-injected quarantine rename failure...
```
（具体 Debug 格式以 runner 聚合为准）

---

## P1 Parser coverage

| 测试 | 结果 |
|---|---|
| `streaming_validator_table_driven_valid_duplicates_and_truncation` | **passed** — 有效值 duplicate，断言 `duplicate <field>`；truncation；unknown |
| `streaming_validator_rejects_missing_required_top_level_fields` | **passed** |
| `streaming_validator_rejects_records_and_field_sets_cap_plus_one` | **passed** — `records exceeded hard cap` / `field_sets exceeded hard cap` |
| `streaming_validator_rejects_duplicate_schema_version` | **passed**（既有） |

---

## 门禁（本机）

```text
cargo fmt --check OK
cargo clippy -D warnings OK
cargo test --workspace → 350 passed, 2 ignored
cargo doc OK
python tools/validate_m4_a0_contracts.py OK
git diff --check OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e OK (~9.5s)
```

---

## 明确未宣称

- **未 commit**；**不宣称 M4-A1/M4 完成**
- **未改** `boundary/met_path.rs`；**未碰** `origo-validation-v1.json`
- unsplit dateline 双 hole mesh：**blocked**（诚实 hard-fail）
- 未跑 10 万 / WSL / native / 三套资料

## 请 A

复验 boundary-chord 反例、1/4 worker digest 表、forensic `.1` 与 quarantine 聚合错误、parser valid-duplicate/cap+1。
**B fixup5 止于此，交 A。**

---

# （以下为 fixup4 历史）

# Trajecta M4-A1：B fixup4 交付报告

状态：按 `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP4.md` 只修三个剩余验收洞；本机门禁全绿；**未 commit**；**不宣称 M4-A1/M4 完成**。
日期：2026-07-23
依据：`docs/TRAJECTA_M4_A1_A_FIXUP3_REVIEW.md`

---

## 总表

| 项 | 状态 | 说明 |
|---|---|---|
| P0-1 terminal Failed lifecycle + real SQLite harness | **implemented / executed / passed** | finished_at 保证；contribute/terminal-persist 负例；真实 sink 故障链 |
| P0-2 exterior topology | **implemented / executed / passed** | segment 分类 + edge_exits；A C-shell bad triangle 拒绝 |
| P0-3 production digest 小矩阵 | **implemented / executed / passed** | 两真实 run 磁盘重算 content/SQL/canonical 相同 |
| P1 parser coverage | **implemented / executed / passed** | table-driven duplicates/truncation/unknown |
| A `met_path` | **未修改** | — |
| `origo-validation-v1.json` | **未碰** | — |
| commit / complete claim | **无** | — |

---

## P0-1 Terminal lifecycle（passed）

### 修复
- `finalize_success`：先取 lifecycle clock；任意 terminal 步失败后保证 `Failed` + `finished_at`（clock 失败 fallback 到 `started_at`）+ `failure` + `provenance=None`
- clock / primary / quarantine / second-persist 错误聚合，不吞
- `finalize_failure` 同样保证合法 Failed

### 测试
| 测试 | 结果 |
|---|---|
| `contribute_manifest_failure_yields_legal_failed_manifest` | Failed 合法 + store 收到 Failed |
| `terminal_persist_failure_quarantines_and_keeps_failed_legal` | primary 含 persist；Failed 合法 |
| `real_sqlite_terminal_persist_fail_quarantines_bundle` | 真实 `ParticleStateSqliteSink`；无正式 bundle；有 forensic；无 spool/tmp；无 complete manifest；内存 Failed |

### 真实 SQLite 故障后文件集合
- 正式 `provenance-bundle.json`：**不存在**
- forensic：`provenance-bundle.json.forensic-aborted` 或 `.1`
- spool/tmp/runs：**无**
- complete manifest：**无**
- SQLite 主库：可保留为 forensic 证据，不被 complete 引用

**partial**：quarantine rename 强制失败的二次注入（返回 primary+quarantine）未单独加磁盘 rename-fail 钩子（quarantine_path 唯一名单测 + AfterRename builder 路径已覆盖）。

---

## P0-2 Exterior topology（passed）

- `SegmentRelation`：Disjoint / SharedFullEdge / EndpointOnly / ProperCrossing / PartialCollinearOverlap
- exterior + hole 均分类；子边 containment → SharedFullEdge；boundary 两端点 on-ring 允许
- **`edge_exits_polygon`**：球面采样边不得离开 exterior−holes（边界点算 inside）— 捕获 A 的 C-shell 反例
- 保留 hole 顶点侵入、tri-tri containment/crossing、excess、面积

### 测试名
- `rejects_c_shell_triangle_with_edge_exiting_exterior` — **passed**（A 冻结反例）
- `accepts_shared_exterior_edge_on_simple_triangle` — **passed**
- 既有双/三 hole、日期线单 hole、凹 exterior — **passed**

日期线双 hole / MultiPolygon 多 hole 专用新 fixture：**未扩**（日期线单 hole + MultiPolygon normalize 既有路径通过）；标 **partial** 若 A 硬性要求专用双 hole 日期线。

---

## P0-3 Production digest 矩阵（passed）

`production_digest_matrix_two_runs_content_and_sql_match`：
- 两真实 `build_runner` + SQLite sink，同 seed 科学内容、不同 run UUID
- 磁盘 stream 重算 content；inspect 重算 SQL；canonical-output 公式
- 断言 content / sqlite_sql / canonical_output **相同**
- exact bundle/SQLite SHA 因 UUID 可不同（记录）

无再用 `&"ab".repeat(32)` 冒充 production SQL digest 作确定性证明。

---

## P1 Parser coverage（passed）

`streaming_validator_table_driven_duplicates_and_truncation`：run_id/sqlite/algo/records/field_sets/samples 注入、截断 JSON、unknown field。

---

## 门禁（本机）

```text
cargo fmt --check OK
cargo clippy -D warnings OK
cargo test --workspace → 339 passed, 2 ignored (fixup 后 runner allow 微调，工程+geometry+runner 聚焦已绿；全量见 target/fixup4_tests.txt 339)
cargo doc OK
python tools/validate_m4_a0_contracts.py OK
git diff --check OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e OK (~9.9s)
```

---

## 明确未宣称

- **未 commit**；**不宣称 M4-A1/M4 完成**
- **未改** `boundary/met_path.rs`；**未碰** `origo-validation-v1.json`
- 未跑 10 万 / WSL / native / 三套资料
- 日期线双 hole 专用 fixture、quarantine rename 强制失败磁盘钩子：**partial**

## 请 A

复验 terminal Failed 合法性、真实 SQLite 故障文件集、C-shell bad triangle、两 run digest 表。
**B fixup4 止于此，交 A。**

---

# （以下为 fixup3 历史）

# Trajecta M4-A1：B fixup3 交付报告

状态：按 `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP3.md` 精确补洞；本机门禁全绿；**未 commit**；**不宣称 M4-A1 / M4 完成**。
日期：2026-07-23
依据：`docs/TRAJECTA_M4_A1_A_FIXUP2_REVIEW.md`

---

## 总表

| 项 | 状态 | 说明 |
|---|---|---|
| P0-1 streaming parser 收紧 | **implemented / executed / passed** | `de.end()`、duplicate guard、capped seeds、`BundleValidationSummary` |
| P0-2 错误不吞掉 | **implemented / executed / passed** | abort→Result；runner abort/quarantine 聚合；terminal 全路径 quarantine |
| P0-3 on-disk digest 独立重算 | **implemented / executed / passed** | stream content digest；manifest 公式校验；CFSR 磁盘重算 |
| P0-4 mesh topology validator | **implemented / executed / passed（现有算法路径）** | 球面 rep、hole 侵入、tri-tri overlap、hole 边 proper-intersect |
| A `met_path` | **未修改** | — |
| `origo-validation-v1.json` | **未碰** | — |
| commit / complete claim | **无** | — |

A 已接受主体（外排/WAL/warmup/identity 字段/`best_any` 删除）**未重写**。

---

## P0-1 Streaming parser（passed）

- 成功 deserialize 后 **`de.end()`**，拒绝尾随 `{}` / 垃圾
- 顶层字段 **duplicate hard fail**
- records/field_sets：`Capped*VecSeed` 在 **MAX+1 立即失败**
- samples 逐项；content digest 边读边哈希
- 返回 **`BundleValidationSummary`**（counts + `content_sha256`），**无 samples Vec**

### 新增负例测试名
- `streaming_validator_rejects_trailing_json_value`
- `streaming_validator_rejects_trailing_garbage`
- `streaming_validator_rejects_duplicate_schema_version`
- `quarantine_chooses_unique_forensic_suffix`

---

## P0-2 Error propagation（passed）

- `ProvenanceBundleBuilder::abort() -> Result`
- AfterRename：quarantine 失败必须上传
- runner：`abort_outputs` / `quarantine_outputs` 聚合错误
- `finalize_success`：terminal 任一步失败 → quarantine+abort；二次 persist 不吞
- sink finish：bundle 失败立即 abort

**诚实 partial**：专用「真实 sink + 强制第二次 terminal persist 失败」全链 runner harness 未单独加文件；API/builder 负例已绿。

---

## P0-3 On-disk digest audit（passed）

- finalize content 来自 **tmp streaming validator**
- `RunManifest::validate` 校验 `canonical_output == f(sql, content)`
- CFSR：磁盘 stream 重算 content + canonical-output 比对 manifest

### 五类 digest
exact bundle / exact SQLite / content / canonical SQL / canonical output

---

## P0-4 Mesh topology（passed on current path）

`validate_mesh_topology`：球面 rep、hole 顶点不进 tri、hole 边 proper-intersect、tri containment/overlap、excess+面积。
正例：双/三 hole、凹 hole、凹 exterior、日期线单 hole。`best_any` 仍删。

未宣称全球/极点 CDT；MultiPolygon 多 hole 日期线专用 fixture 未扩。

---

## 门禁（本机实际）

```text
cargo fmt --check OK
cargo clippy -D warnings OK
cargo test --workspace → 332 passed, 2 ignored
cargo doc OK
python tools/validate_m4_a0_contracts.py OK
git diff --check OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e OK (~7.8s)
```

---

## 明确未宣称

- **未 commit**；**不宣称 M4-A1/M4 完成**
- **未改** `met_path`；**未碰** `origo-validation-v1.json`
- 未跑 10 万/WSL/native/三套资料
- 真实 sink×terminal-persist 强制失败 harness：**partial**

## 请 A

复验 streaming EOF/duplicate、abort 聚合、CFSR 五 digest、topology。
**B fixup3 止于此，交 A。**

---

# （以下为 fixup2 历史归档，以上为 fixup3 正文）

# Trajecta M4-A1：B fixup2 交付报告

状态：按 `docs/B_PROMPT_M4_A1_CLOSEOUT_FIXUP2.md` 返工；本机门禁全绿；**未 commit**；**不宣称 M4-A1 / M4 完成**。
日期：2026-07-23
依据：`docs/TRAJECTA_M4_A1_A_FIXUP_REVIEW.md`

---

## 总表

| 项 | 状态 | 说明 |
|---|---|---|
| P0-1 streaming validator + inspect | **implemented / executed / passed** | samples 无 `Vec`；inspect 无 `fs::read` |
| P0-2 abort / forensic lifecycle | **implemented / executed / passed** | sink/product abort；terminal persist 失败 quarantine；finish 失败立即 abort |
| P0-3 digests in production identity | **implemented / executed / passed** | content + SQL + canonical-output 必填进 manifest/schema |
| P0-4 multi-hole mesh | **implemented / executed / passed（fixture 集）** | 删除 `best_any`；centroid 在域内；2/3 hole + 凹 hole 正例；越界 hole 负例 |
| A `met_path` | **未修改** | — |
| `origo-validation-v1.json` | **未碰** | — |
| commit / M4-A1 complete claim | **无** | — |

A 已接受锚点（warmup / 外排 / WAL TRUNCATE / CFSR E2E）**未重写**。

---

## P0-1 Streaming validator + inspect（passed）

### Validator
- `validate_bundle_file_semantics` 使用 `serde_json::Deserializer` + `DeserializeSeed` / `Visitor`
- records / field_sets 在 hard cap 内校验
- **samples 逐项消费**：只保留 prev key、count、当前引用；**不返回 samples `Vec`**
- 校验的是 **实际写出的 tmp 文件**（replace 前）
- 负例覆盖：duplicate / missing SQLite row / 既有 hash·slot 规则

### Inspect
- `ParticleStateSqliteSink::inspect`：`metadata` size + `file_sha256` 流式 SHA
- 生产路径无 `fs::read` 全文件（仓库检索：inspect/hash 路径已清）

---

## P0-2 Terminal abort / forensic（passed）

协议：
- `ParticleStateSink::{abort, quarantine_forensic}`（默认 no-op）
- `OutputProduct::{abort, quarantine_forensic}` → `ParticleStateProduct` 转发 sink
- SQLite `finish`：bundle finalize 失败 **立即** `bundle.abort()`
- runner `run_inner`：任一 product `finish` 失败 → 全部 `abort`
- runner `finalize_success`：terminal `persist_manifest` 失败 → `quarantine_forensic` + `abort`，`provenance=None`，status→`Failed`，**不得**留下 complete manifest
- runner `finalize_failure`：abort 输出并清空 provenance
- `quarantine_path`：唯一 forensic 名；冲突递增后缀；**不得**忽略 rename 错误；不得留下正式 `provenance-bundle.json`
- begin 清理 stale formal/tmp/spool/runs；不把 forensic 当正式 bundle

故障注入（builder 级，已有）：BeforeTmpWrite / BeforeRename / AfterRename。
全链 runner×checkpoint busy 仍建议 A 在下一轮用专用 harness 加码（sink WAL busy 单测 + 生命周期 abort 已接线）。

---

## P0-3 Digests in identity（passed）

`ProvenanceBundleIdentity` 必填：

| 字段 | 含义 |
|---|---|
| `sha256` | exact bundle bytes |
| `sqlite_sha256` | exact SQLite bytes |
| `content_sha256` | `trajecta.provenance-content/v1` |
| `sqlite_sql_sha256` | canonical ordered SQL（无 run UUID） |
| `canonical_output_sha256` | `trajecta.canonical-output/v1` |

接线：
- sink `finish`：关 writer 前算真实 SQL digest → `finalize_after_sqlite(..., sql_sha)`
- builder 计算 content + `canonical_output_digest` 写入 identity（`last_*` 同步赋值，非永远 None）
- manifest validate + `M4_RUN_MANIFEST.schema.json` + A0 fixture + 合同文档
- CFSR E2E：digest 字段长度；`sqlite_sql_sha256` 与 `inspect` 重算一致

单测：跨 run_id exact≠ content= canonical-output=（同 SQL digest）。

---

## P0-4 Multi-hole mesh（passed on fixtures）

- **删除 `best_any`**：无合法桥 → `InvalidGeometry`
- 经典双顶点 keyhole 桥保留
- `finish_mesh`：面积容差 + **三角形代表点必须在 exterior−holes 内** + 正 excess
- 正例：双 hole、三 hole + 凹 hole、凹 exterior、日期线+单 hole
- 负例：hole 在 exterior 外 → hard fail
- 同 rings 两次 mesh 顶点序确定

未宣称：任意 MultiPolygon/极点/全球尺度的通用 CDT。若 A 要求依赖级 CDT，标后续。

---

## 故障后文件集合（合同）

| 失败点 | 正式 bundle | tmp/spool/runs | complete manifest |
|---|---|---|---|
| BeforeTmpWrite | 无 | 清 | 无 |
| BeforeRename | 无 | 清 | 无 |
| AfterRename（identity 未交付） | forensic 化 | 清 | 无 |
| terminal persist fail | forensic 化 | 清 | 无（Failed 或 running） |
| product finish / checkpoint fail | 无正式 | abort 清 | Failed |

SQLite 主库可作为 forensic 证据保留，但 **不得** 被 complete manifest 引用。

---

## 四类 SHA（生产）

1. **exact bundle** `provenance.sha256`
2. **exact SQLite** `provenance.sqlite_sha256`
3. **content** `provenance.content_sha256`
4. **canonical SQL** `provenance.sqlite_sql_sha256` → 与 3 合成 **canonical output** `provenance.canonical_output_sha256`

---

## 门禁（本机实际）

```text
cargo fmt --all -- --check                                          OK
cargo clippy --offline --workspace --all-targets -- -D warnings    OK
cargo test --offline --workspace                                   OK
  → 328 passed; 0 failed; 2 ignored   (target/fixup2_tests.txt)
cargo doc --offline --workspace --no-deps                          OK
python tools/validate_m4_a0_contracts.py                           OK
git diff --check                                                   OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                         OK (~10s)
```

---

## 明确未宣称 / 未做

- **未 commit**；**不宣称 M4-A1/M4 完成**
- **未改** `boundary/met_path.rs` 数值核心
- **未碰** 外层 `origo-validation-v1.json`
- 未跑 10 万 / WSL / native / 三套资料长矩阵
- runner 级 checkpoint-busy 全链 inject harness：接线完成，专用负例可再加
- 全球/极点 CDT：未做

## 请 A

复验 streaming validator、forensic lifecycle、identity 三 digest、多 hole fixture 集。
**B fixup2 止于此，交 A。**
