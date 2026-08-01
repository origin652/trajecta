# Trajecta M4-A1：A 对 Fixup5 的复验与日期线收口

状态：Fixup5 的 exterior topology、production digest、forensic、streaming parser 四条链通过 A 复验；A 直接修复了 B 留下的最后一个已知合同阻断——用户提交未预切的跨日界线 `Polygon` 且同时含跨线 hole 与普通 hole 时，系统现在会自动生成可网格、可采样的规范 `MultiPolygon`。M4-A1 普通 release 工程闭环通过本轮验收；未 commit，不宣称整个 M4 完成。

日期：2026-07-23

依据：

- `docs/engineering/TRAJECTA_M4_PARTICLE_LOOP_PLAN.md`
- `docs/engineering/TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`
- `docs/engineering/TRAJECTA_M4_A1_A_FIXUP4_REVIEW.md`
- `docs/engineering/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`
- `docs/engineering/B_PROMPT_M4_A1_CLOSEOUT_FIXUP5.md`

## 1. 最终裁决

| 项 | A 裁决 | 证据 |
|---|---|---|
| Exterior 分段拓扑证明 | **accepted** | C-shell boundary chord、partial overlap、containment、shared edge 与 endpoint-only 正反例全绿 |
| Production digest 矩阵 | **accepted** | workers 1/4、identity/reverse、chunk 3/7；normalized content、canonical SQL/output 稳定 |
| Forensic 与 quarantine failure | **accepted** | 旧 forensic 保留、新文件精确使用 `.1`；primary + quarantine failure 聚合 |
| Streaming parser coverage | **accepted** | valid duplicate、missing、truncation、unknown、records/field_sets cap+1 |
| 未预切日期线 Polygon + 双 hole | **accepted after A fix** | canonicalize → MultiPolygon → component mesh → 16 点 sampling 全链通过 |
| 工程门禁 | **passed** | 350 passed、2 个旧 M3 百万点测试 ignored；真实 CFSR E2E 通过 |

## 2. 日期线阻断的真实根因

B 的最后报告把失败归在 multi-hole keyhole mesh。A 用公开入口复现后确认，更早的拓扑表达才是根因：

- exterior 跨日界线后会生成东西两个 half-shell；
- 跨日界线的 hole 被切成两块，每一块都会接触新生成的 antimeridian seam；
- 这种 piece 已不再是严格位于 shell 内部的 hole，而是从 seam 向内延伸的边界缺口；
- 若仍把它保存为 interior hole，就会得到 touching boundary / weakly-simple keyhole，桥点排序只能改变失败形状，不能修复拓扑。

因此本轮没有放宽面积容差，也没有继续增加桥接 heuristic。

## 3. A 的实现

实现位于 `crates/trajecta-core/src/release/geometry.rs`：

1. exterior 仍按真实大圆弧与 antimeridian 的交点切成 West/East half-shell；
2. 只对真正跨线的 hole 分别 clip；不跨线 hole 保持完整并按球面 containment 分配给唯一 component；
3. 对 clipped crossing-hole piece：
   - 识别 seam 与非 seam 的两个转换点；
   - 验证全部非 seam 顶点位于目标 half-shell；
   - 找到完整包含该 seam 区间的 shell 边；
   - 用 hole 的非 seam 边界路径替换该 seam 区间，使 hole piece 成为 exterior notch；
4. 每个 component 重新执行 ring orientation、lexicographic rotation、hole sorting、自交检查和球面面积守恒；
5. 最终切分前后面积相对误差继续使用冻结的 `1e-12`，没有修改 science contract。

关键实现入口：

- `split_polygon_dateline`
- `clipped_hole_seam_transitions`
- `validate_clipped_hole_piece_in_shell`
- `splice_dateline_hole_notch`

## 4. 冻结合同反例

输入为一个未预切 `Polygon`：

- exterior：`179°E ↔ 179°W`；
- hole 1：自身跨日界线；
- hole 2：完整位于东侧，不跨线。

新 hard gate `dateline_shell_with_two_holes_canonicalize_mesh_sample` 证明：

- public `canonicalize_geometry` 必须成功；
- canonical geometry 必须为两个 component 的 `MultiPolygon`；
- 一个 component 只有 notch exterior，另一个为 notch exterior + 完整普通 hole；
- 两个 component 都必须成功构建 spherical triangle mesh；
- `SphericalGeometrySampler` 必须实际生成 16 个点；
- 每个样本必须仍位于原始未切 Polygon 内；
- exterior/hole 全部反向且 hole 输入顺序互换后，canonical geometry SHA 与球面面积必须不变。

原先测试中接受 `Ok` **或** `InvalidGeometry` 的软门已经删除；当前是必须成功的 hard gate，诊断 `eprintln!` 也已清理。

## 5. A 独立门禁

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline -p trajecta-core release::geometry::tests  OK, 19 passed
cargo test --offline --workspace                                OK, 350 passed / 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python -m py_compile tools/validate_m4_a0_contracts.py          OK
python tools/validate_m4_a0_contracts.py                        OK
git diff --check                                                 OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      OK, 1 passed, 10.30s
```

两个 ignored 测试均为显式 legacy M3 百万点性能门禁。本轮按用户约定不运行百万点；M4 最终性能阶段使用 10 万粒子。

## 6. 阶段结论

Fixup5 及其最后的 unsplit dateline 双 hole 阻断已闭合。按照 `TRAJECTA_M4_MODEL_ASSIGNMENT.md` 的 M4-A1 退出条件，当前普通 release 已能在正向/反向合成场和真实 CFSR 小样本上生成完整 SQLite 轨迹与可自校验 manifest/provenance；M4-A1 工程实现可进入阶段状态更新与 commit。

这不代表整个 M4 完成。后续仍包括：

- M4-A2 air-mass domain-fill 与质量守恒；
- M4-A3 ozone/PV 规则与 oracle；
- M4-A4 三资料正反向、Windows/WSL、Rust/native、10 万粒子、性能和最终 A 审计。

## 7. 约束

- 未 commit；
- 未修改 `crates/trajecta-core/src/boundary/met_path.rs`；
- 未碰外层 `origo-validation-v1.json`；
- 未运行百万点；
- 不宣称 M4 完成。
