# 给 B：M4-A1 closeout Fixup5（最后四个精确验收洞）

你是 B 模型。本轮完整阅读：

- `docs/engineering/TRAJECTA_M4_A1_A_FIXUP4_REVIEW.md`
- `docs/engineering/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/engineering/B_PROMPT_M4_A1_CLOSEOUT_FIXUP4.md`
- `docs/engineering/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`

保留全部现有未提交改动；不修改 `crates/trajecta-core/src/boundary/met_path.rs`；不碰外层 `origo-validation-v1.json`；不 commit；不宣称 M4-A1/M4 完成。

A 已接受：terminal Failed lifecycle、真实 SQLite terminal-persist 主链、两真实 UUID run 的 on-disk digest 审计、streaming parser 主体、external sort/WAL/warmup/CFSR E2E。不要重写这些主体。

## P0-1：把 exterior edge gate 改成真正的分段拓扑证明

当前两个错误必须删除：

1. `edge_exits_polygon` 固定检查 11 个点；
2. `point_on_ring(a) && point_on_ring(b)` 直接视作 `boundary_edge`。

### 实现要求

对每条 triangle geodesic edge：

1. 与 exterior 和所有 hole ring edges 求完整球面 segment relations / intersection fractions；
2. 收集 `0`、`1` 和全部交点 fraction，去重、排序；
3. 对相邻 fraction 的每个**开区间**取确定性球面中点；由于区间内部无 ring crossing，该区间的 polygon membership 必须恒定；任一区间位于 exterior-minus-holes 外即拒绝；
4. proper crossing 不得再由固定采样二次决定；
5. exact shared full edge 可以接受；
6. triangle edge 只有在被一个 ring edge或一条连续、同大圆、无间隙的 ring-edge chain 完整覆盖时，才可视为 boundary-covered；仅端点 on-ring 不够；
7. partial collinear overlap 必须证明覆盖全集，否则拒绝；
8. endpoint touch 要结合相邻开区间 membership，不能自动代表合法；
9. 删除/退休固定 11 点 `edge_exits_polygon` 作为证书；采样只能留作非验收诊断。

冻结 A 新反例：

```text
exterior C-shell:
  (-20,0) (5,0) (5,1) (1,1) (1,4)
  (5,4) (5,5) (-20,5) (-20,0)

bad triangle:
  (5,1) (1,4) (-20,2.5)
```

三顶点都在 exterior，但弦穿过凹口；`validate_mesh_topology` 必须拒绝。

保留 Fixup4 已通过的原 C-shell 反例和合法 shared exterior edge 正例。

### 必须补的 geometry fixture

- 日期线 shell + 两个 holes：canonicalize、mesh、sampling 全链；
- MultiPolygon：至少一个 component 含两个 holes；
- ring 起点旋转、方向翻转、hole 输入排列变化：canonical geometry SHA / mesh digest 保持冻结规则；
- artificial exterior partial overlap；
- triangle overlap 与 containment；
- 合法 endpoint-only touch 和完整 boundary sub-edge 正例。

若当前 keyhole/ear-clip 无法通过合法 fixture，保留最小反例并标 blocked，不得放宽面积容差或恢复固定探针。

## P0-2：真正执行 production digest A/B 矩阵

现有测试两边都是 worker_threads=1、同顺序、同 chunk。保留其 UUID 归一化价值，但新增真正矩阵：

| run | workers | particle/input order | provenance chunk | run_id |
|---|---:|---|---:|---|
| A | 1 | identity | small A | UUID A |
| B | 4 | exact inverse/permutation | different small B | UUID B |

要求：

1. 两边都使用真实 `ParticleStateSqliteSink` + provenance bundle；不得传伪 SQL SHA；
2. 若 production chunk 当前不可注入，增加仅测试可用、但调用同一 production external-sort 实现的 limits 配置；不得复制一套测试算法；
3. 科学记录集合相同，只改变 worker/order/chunk/run_id；
4. 从两边实际磁盘独立重算五类 digest；
5. 断言：
   - UUID A != UUID B；
   - exact bundle SHA 因 run_id 不同而不同；
   - normalized content 相同；
   - canonical SQL 相同；
   - canonical output 相同；
6. 记录 exact SQLite SHA 是否相同，不预设错误结论；
7. 增加 production 敏感性负例：只改一个 SQL 数值或一个 sample assignment，canonical output 必须变化。

保持极小 fixture，不跑 10 万。

## P0-3：补真实 forensic 冲突与 quarantine failure

修正 `real_sqlite_terminal_persist_fail_quarantines_bundle`：

1. runner build 后定位实际 run directory；
2. 在 run 前真实创建 `provenance-bundle.json.forensic-aborted`，写入冻结旧内容；
3. terminal persist 失败后断言旧文件未被覆盖，新的正式 bundle 被移动到 `.forensic-aborted.1`；
4. 不得再用 `forensic_0 || forensic_1`；必须精确断言 `.1`。

再加 test-only fault injection，走真实 sink/runner：

1. bundle 已正式 publish；
2. terminal manifest persist 失败；
3. quarantine rename 强制失败；
4. 返回错误必须同时包含 primary terminal persist 与 quarantine failure；
5. 不得声称 formal bundle 已清理；实际文件集合必须与错误一致；
6. failed-manifest 二次 persist 结果也要记录。

fault hook 只用于测试，生产默认关闭；不要依赖 Windows ACL 或偶然文件锁。

## P1：让 parser coverage 真正命中 duplicate/cap

不要用 `"field": null` 冒充 duplicate 测试。对每个顶层字段插入一个**类型与内容均有效**的重复值，并断言错误包含 `duplicate <field>`。

同时补：

- records 数组 `MAX_UNIQUE_RECORDS + 1`；
- field_sets 数组 `MAX_UNIQUE_FIELD_SETS + 1`；
- 每个必填顶层字段 missing；
- truncated / unknown 保留。

只补测试，不重写已接受 parser。

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

更新 `docs/engineering/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，必须列出：

- A 新 boundary-chord 反例测试名；
- 日期线双 hole / MultiPolygon 多 hole 测试名；
- A/B 两个 production run 的 worker/order/chunk 与五类 digest 表；
- forensic 旧文件与 `.1` 的 exact 文件集合；
- quarantine failure 聚合错误原文；
- parser valid-duplicate 与 cap+1 测试名；
- 未闭合项标 blocked。
