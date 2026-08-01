# Trajecta M4-A1：A 级复验与当前裁决

状态：A 级数值核心和 production CFSR 锚点已通过本机复验；B 工程闭环仍有明确缺口；未提交，不宣称 M4-A1 或 M4 完成。
日期：2026-07-22

## 1. 一句话结论

普通粒子循环已经具备真实 CFSR `RunnerBuilder -> MetEngine -> SimulationRunner -> SQLite` 的完整成功路径，连续边界的 interval isolator 也已由 A 重写为“不能证明就失败”的实现；当前阻止 M4-A1 签署的主要项目是正式 provenance bundle 接线、通用凹 Polygon/多 hole 网格，以及 capability warmup 的传递依赖和逐 domain/profile 计算。

## 2. 本轮 A 复验

### 2.1 连续边界 segmentation

`crates/trajecta-core/src/boundary/met_path.rs` 当前采用：

- outward-rounded interval arithmetic；
- 二阶自动微分 `Jet2`；
- 精确 great-circle 参数化；
- bilinear terrain/model-top 与线性时间 corner；
- 只有 `residual'` 整区间排除零，或 `residual''` 严格排除零且存在端点 bracket 时才接受证明；
- bracket 后固定 64 次二分；
- 无法证明、节点/深度/宽度预算耗尽均为 `RootFindingFailed`，不得静默放行。

冻结反例已覆盖“两个端点导数同号、内部仍有两个根”，实测根约为：

```text
0.4332244580660234
0.6067551181568030
```

A 接受该实现作为当前 M4-A1 boundary certificate 基线。B 后续不得修改该数值核心；若出现新反例，应交 A 裁决。

### 2.2 真实 CFSR production E2E

强制真实资料运行：

```powershell
$env:TRAJECTA_REQUIRE_REAL_MET='1'
cargo test --offline -p trajecta-core --test m4_a1_cfsr_e2e -- --nocapture
```

结果：`1 passed`，约 9 秒。测试只接受：

- `RunOutcome::Complete`；
- terminal manifest `status == complete`；
- `terminations.abnormal_count == 0`；
- 00/06 UTC 两帧 content SHA 与 reader backend 写入 manifest；
- SQLite 的 U/V/W/P/T 输出列存在。

因此该锚点不再允许 `CompletedWithParticleErrors` 伪装成功。

### 2.3 选帧 warmup 裁决

B 已正确删除固定左扩一帧，并保留连续 bracket、exact-hit previous/next 和缺 warmup hard fail。当前实现对 CFSR 锚点正确，因为现有正式 profile 的相关 `warmup_frames` 均为 0。

但还剩两个工程正确性缺口：

1. `max_warmup_frames_for_capabilities` 只匹配 capability 直接输出，没有沿 derived expression 的 identifier 依赖递归展开；能力输出若由累积源间接派生，可能漏算 warmup。
2. 所有 domain/profile 共用一个全局最大 warmup；一个资料域的长 warmup 会让无关域被过度扩展甚至错误 hard fail。必须按实际 `(domain, profile)` 计算和选择。

在这两项修复前，只能认定当前 warmup 对已测 profile 成立，不能称为通用合同闭合。

### 2.4 Provenance bundle v1

A 已冻结：

- `testdata/M4_PROVENANCE_BUNDLE.schema.json`；
- `testdata/M4_PROVENANCE_BUNDLE.example.json`；
- `docs/engineering/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`；
- terminal manifest 的 provenance identity 条件。

合同不修改 SQLite v1。正式 `provenance-bundle.json` 以 `(particle_id, sample_sequence)` 映射 U/V/W/P/T 五个字段，各字段再引用完整、内容寻址的 `ProvenanceRecord`。record 与 field set 固定使用 `sha256-rfc8785`；输出排序、SQLite 全覆盖、原子写和 terminal manifest SHA 均已冻结。

机器 validator 已包含 record hash 错误、重复 sample、错误字段 slot 三类负例并通过。生产 Rust 尚未接线，所以旧 `provenance-table.json` 不能作为验收证据。

## 3. M4-A1 尚未闭合

### P0

1. 正式 provenance bundle 的 Rust 类型、逐字段提取、RFC 8785 hash、外排/归并、原子写、SQLite 全覆盖校验、terminal manifest 和 canonical digest 接线。
2. 一般凹 Polygon 与多个 hole 的可靠球面 mesh/采样；当前只覆盖部分单 hole、日期线和面积反例。
3. warmup 沿 derived dependency 传递，并按 `(domain, profile)` 独立选择。

### P1

- 补齐 solid-body、AGL/pressure release、boundary failure injection、1/4 worker、chunk 和逆排列的完整文件级矩阵；
- C 独立重算 record、field-set、SQLite、bundle 四类 SHA，并核对 sample 全覆盖；
- 后续性能门禁使用 10 万粒子，不恢复旧百万点为默认测试。

## 4. 本机门禁

```text
cargo fmt --all -- --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings OK
cargo test --offline --workspace                                310 passed, 2 ignored
cargo doc --offline --workspace --no-deps                       OK
python tools/validate_m4_a0_contracts.py                         OK
git diff --check                                                 OK
TRAJECTA_REQUIRE_REAL_MET=1 m4_a1_cfsr_e2e                      1 passed
```

两条 ignored 是旧 M3 百万点性能测试；没有把它们带入 M4 默认门禁。

## 5. 明确未宣称

- 未宣称 M4-A1 完成；
- 未宣称 M4 完成；
- 未运行 M4 WSL/Windows 10 万粒子矩阵；
- 未运行三套真实资料的完整 M4 trajectory 矩阵；
- 未完成 C 独立验证；
- 未 commit。

下一轮 B 的工程任务见 `docs/engineering/B_PROMPT_M4_A1_CLOSEOUT.md`。
