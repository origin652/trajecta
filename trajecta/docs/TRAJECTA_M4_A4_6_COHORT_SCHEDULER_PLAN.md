# Trajecta M4-A4.6：cohort-local 调度与 50k 长测恢复计划

状态：**completed by A（2026-07-27）**。最终 post-stable-ID WSL 正式六格均为 attempt-1 passed，
forward/backward scaling 分别为 `2.13871967035736` / `2.2298053451229514`。

## 1. 背景

M4-A4 的首个正式 `S50-F` 在一小时外层超时前生成了 20 个 lifecycle output event、976,272 条 `particle_state` 和约 2.48 GiB WAL。只读复核确认，人口边界 birth 被送入全局 `StepPlanner` 后，每个新 birth 都切割所有既有粒子的数值步；约 50,000 个旧粒子因此被重复推进，耗时和写出量异常膨胀。

M4-A4.6 的唯一目标是切断这条全局重推进路径，同时保留每个新粒子的精确出生时刻、正反向科学语义、边界交点、质量守恒和 SQLite 生命周期证据。

## 2. 版本边界

本项目尚未发布，A4.6 直接修订当前未发布的 M4 v1 合同：

- 保持 `trajecta/particle_loop/m4/v1`；
- 保持 run-manifest、numerical-contract、SQLite 和 provenance bundle 的现有 v1；
- 不创建任何 v2、兼容层或平行 schema；
- 在现有 `M4_NUMERICAL_CONTRACT.v1.json` 的 `algorithms` 中登记 `cohort_scheduler = cohort_local_macro_step/v1`；
- 既有 release birth、domain-fill birth、RK2、boundary 和 mass-ledger 算法 ID 不改名。

## 3. 冻结语义

1. 全局宏步只由 meteorology frame、scheduled output 和 simulation end 截断。
2. population 一次准备宏步内全部 birth cohort；birth 保留精确 `Timestamp`。
3. 既有粒子从宏步首到宏步尾只推进一次；新粒子从自己的 birth time 推进到同一宏步尾。
4. RK2 每粒子使用自己的 signed dt；正反向批次不得混合方向。
5. birth 与 termination 仍在精确物理时刻写 lifecycle-only event，不复制无关粒子。
6. boundary termination 保存精确 time、可选 intersection fraction、相符的 signed offset 和 nonnegative age。
7. domain-fill residual 在宏步内一次应用；每个宏步只生成一条 mass-ledger record，新生粒子的同步终止必须进入 outgoing/normal/abnormal 分类。
8. 不通过放宽异常分类、输出覆盖、容差或质量门禁来修复性能。

## 4. 实施顺序

### P0：科学与生命周期

- `ParticleBatch` 保存 typed termination metadata；
- integrator 支持 per-row start time 和 common macro end；
- runner 改为 cohort-local birth；
- boundary fraction 确定性映射到纳秒；
- SQLite 写真实 termination time/fraction；
- domain-fill 聚合宏步 residual 与 ledger。

### P1：冻结反例

- 旧粒子结果等于无 birth 控制组；
- 新生粒子结果等于独立 exact-time release；
- forward/backward 均通过；
- 半步失败的 position/time/age/offset 一致；
- boundary fraction 的 time/age/offset 一致；
- lifecycle-only SQLite event 不写无关粒子；
- termination 与 terminal `particle_state` 时刻一致；
- mass-ledger 条数等于宏步数而不是 birth 数。

### P2：门禁与真实性能

- `fmt`、`clippy -D warnings`、core/workspace tests、doc、A0 contract validator；
- 三套真实资料 1k/10k smoke；
- WSL 复跑 `S50-F`，冻结目标不超过 12 分钟；
- `S50-F` 通过后才进入 `S100-F` 和剩余正式矩阵。

若 `S50-F` 仍超时，只允许继续归因两个已知工程边界：heterogeneous-time MetEngine batch 和 lifecycle SQLite 批事务。不得恢复 birth 全局切步，也不得新建版本来绕过失败。

## 5. 职责

A 独立负责调度、RK2、终止时间、质量账本、合同勘误、真实性能复验和最终裁决；不调用子代理。若用户另行把机械执行交给 B，只能按 A 后续书面 Prompt 运行冻结命令并整理 artifact，不得修改数值核心、版本、容差或验收结论。

## 6. 退出条件

A4.6 只有同时满足以下条件才可完成：

- 全部 P1 科学反例通过；
- v1 合同、代码和机器校验一致；
- 1k/10k smoke 无新增异常终止或 lifecycle 覆盖错误；
- WSL `S50-F` 不再出现 birth 驱动的旧粒子重复推进、异常 event/row/WAL 膨胀；
- 性能达到 12 分钟门限，或留下可重复、已归因且不伪装通过的后续工程阻断。

在这些条件满足前，不宣称 M4-A4 或 M4 完成。

最终裁决：上述条件现已全部满足；完整证据与 A 签署见
`TRAJECTA_M4_A4_A_COMPLETION_REPORT.md` 和 `TRAJECTA_M4_A_FINAL_COMPLETION_REPORT.md`。
