# Trajecta M4-A0 交付与审计报告

状态：A0 合同冻结已通过；未提交，不宣称 M4 完成。
日期：2026-07-21

## 1. 结论

M4-A0 的 Case、RunProfile、粒子状态、时钟、RNG、release、边界、population、输出、manifest、SQLite、公式、常量、算法 ID、容差和错误语义已形成可编译且机器可自检的冻结合同。

这只允许后续进入 M4-A1。`SimulationRunner`、生产 RK2、边界实现、GeoJSON 规范化/采样、SQLite writer、domain-fill 和 PV 尚未完成，因此不得宣称 M4 或普通粒子闭环完成。

## 2. 已落地

- `NumericsSpec.random_seed` 与显式 release events；
- typed GeoJSON source/geometry、ASL/AGL/pressure release vertical；
- 单一 domain-fill domain 与 count/mass 二选一；
- typed output product/sink 与 endpoints/interval schedule；
- RunProfile `output_root`；
- stable 63-bit particle ID、typed origin、signed offset、elapsed age、carrier/substance mass、sensitivity 和 termination；
- signed `StepPlanner`；
- `philox4x32-10/v1` 与 Random123 零向量；
- `integer_stratified_birth/v1`；
- 球面 RK2、格面积、pressure interface、dry-air mass/density、PV60 ozone、补偿求和和质量容差独立参考；
- integrator 与 boundary 生命周期职责分离；
- 连续边界 path segment 合同；
- population per-step direction/signed-step/index/seed 合同；
- typed serde `RunManifest` 与 Draft 2020-12 schema；
- SQLite v1 public schema、WAL/NORMAL、关系约束和索引；
- 所有新增核心错误的稳定 `code()`；
- legacy M3 百万点测试改为显式 ignored；M4 长测固定为 10 万粒子。

## 3. A 审计中修正的关键缺口

1. `ParticleState` 原先漏掉各 substance mass，现与 `ParticleBatch` 完整对齐。
2. integrator 原先持有 boundary chain，现由 runner 在 RK2 proposal 后统一应用。
3. boundary sampler 原先不能表达插值网格分段，现冻结连续有序 `[0,1]` partition。
4. release 出生文档原先写浮点 `U`，实现使用 64-bit 乘高；现统一为版本化整数算法。
5. release sampler 原先缺 seed/population/ordinal 显式输入，现由 `ReleaseSamplingRequest` 冻结。
6. population context 原先缺 direction、signed dt、step index 和 seed，现不再要求实现者猜测。
7. manifest 原先只存 output 字符串，现保存完整 typed effective output 配置并加强生命周期、SHA、数据集和 SQLite 校验。
8. SQLite 原先只有文本存在性检查，现可实际执行 schema 并验证 WAL、表、索引、foreign keys 和 integrity。

## 4. 实际门禁

~~~text
cargo fmt --all --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings  OK
cargo test --offline --workspace                             263 passed, 2 ignored
cargo doc --offline --workspace --no-deps                    OK
python -m py_compile tools/validate_m4_a0_contracts.py        OK
python tools/validate_m4_a0_contracts.py                      passed
git diff --check                                             OK
~~~

两条 ignored 均为版本化保留的旧 M3 百万点性能门禁：

- `cfsr_pgbl_million_point_budget_and_threading`；
- `cfsr_pgbl_million_point_performance_matrix`。

它们仍可显式运行，但不再因本机存在真实资料而进入默认 workspace tests。

## 5. A1 入口与未完成范围

下一步由 A 先完成并科学验收：

- `Rk2Spherical::advance`；
- `SimulationRunner` 生命周期与 registry；
- 地面连续反射、模式顶、有限域和全球周期边界；
- ReleaseDriven lifecycle、stable collision detection 和 exact event emission；
- 输出时刻正式气象查询语义。

随后 B 可在冻结合同下接 GeoJSON 日期线切分/采样、SQLite crate、manifest 原子 I/O 和真实资料工程链；C 可接 schema/golden/docs。A1 之前不得让 B/C 自行决定上述数值与生命周期语义。

## 6. 明确未宣称

- 未宣称 M4 完成；
- 未宣称普通 release 闭环可运行；
- 未宣称 SQLite writer 已实现；
- 未运行 M4 Windows/WSL 10 万粒子链；
- 未运行 M4 Rust/native 轨迹差分；
- 未提交 git commit。
