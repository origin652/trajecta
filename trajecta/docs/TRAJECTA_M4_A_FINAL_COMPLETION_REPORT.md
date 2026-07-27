# Trajecta M4：A 最终科学验收与完成裁决

状态：**M4 complete**
日期：2026-07-27
仓库：`E:\flexpart\trajecta`
Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
提交状态：**未 commit / 未 push**

## 1. 最终裁决

A 已完成 M4-A0～M4-A4 的合同、实现、科学审计、真实资料、跨平台、native、性能与输出证据复核。
M4 计划列出的十项完成条件全部满足，因此 A 正式签署：**Trajecta M4 完成**。

这表示最小拉格朗日粒子模拟闭环已经成型并通过冻结范围验收；它不等于项目已经发布，也不等于所有未来
资料源、物理过程或生产运维能力都已完成。

## 2. 十项完成定义逐项验收

| # | 完成条件 | 证据与裁决 |
|---:|---|---|
| 1 | release、air-mass、ozone 三条链闭合 | A1/A2/A3 已完成；生产 RunnerBuilder→MetEngine→SimulationRunner→SQLite/provenance 全链可运行 |
| 2 | 支持范围无可达 `NotImplemented` | 内置注册表、三类 population、边界、sink 和三套冻结资料路径已闭合；未支持范围没有伪装成支持 |
| 3 | 解析解、不变量、边界、守恒、输出 hard gate | workspace、A0 validator、科学反例、质量账本、lifecycle、finite、digest 全部通过 |
| 4 | 三套真实资料正反向实际运行 | A2 air-mass 12/12；A3 ozone 12/12；均 Windows/WSL、abnormal=0 |
| 5 | Windows 与 WSL 门禁实际运行 | 当前源码两平台均 `455 passed / 11 ignored`，fmt/clippy/doc/A0/diff-check 通过 |
| 6 | Rust/native 小矩阵实际运行 | A3 三族双方向 6/6 pair；A4 重新核 SHA，0 hard blocker |
| 7 | WSL ERA5 hybrid 100k 长测 | forward/backward、w1/w4 正式 100k 均 Complete；正式六格 aggregate passed |
| 8 | SQLite/manifest/hash/性能自校验 | integrity、WAL、bundle、content/SQL/output digest、artifact index、baseline 均可重算 |
| 9 | 异常数值终止为 0 | A2、A3 与 A4 正式验收矩阵均 abnormal=0 |
| 10 | A 最终科学审计并书面签署 | 本报告与 `TRAJECTA_M4_A4_A_COMPLETION_REPORT.md` |

## 3. M4 现在可以实现什么

### 3.1 输入与场景

- 解析并验证冻结的 Case、RunProfile、dataset lock 与内容 SHA；
- 加载 ERA5 pressure、ERA5 hybrid 137 层和 CFSR pressure 三套真实资料；
- 通过 Rust reader 生产链预加载气象场，并在执行阶段保持 provider I/O delta 为 0；
- 正向和反向模拟；
- 普通 release、air-mass domain-fill、PV60 ozone domain-fill。

### 3.2 粒子与科学核心

- 确定性 Philox 播种和稳定粒子 ID；
- GeoJSON 点、线、Polygon、MultiPolygon 的球面采样，含跨日界线 Polygon 自动切分与多 hole；
- AGL/ASL/pressure 等冻结垂直释放解析；
- 球面 RK2、正反向时间、cohort-local 精确 birth；
- 连续地面反射、模式顶与有限域终止；
- air-mass 初始化、边界通量、residual 与逐步/最终质量守恒；
- 现代球面 Ertel PV 与 FLEXPART PV60 legacy 臭氧赋值规则；
- pressure/hybrid 的 U/V/几何 W、pressure、temperature 与质量/质量标记输出。

### 3.3 输出与审计

- SQLite v1 保存粒子、物质质量、scheduled/lifecycle events、每个采样状态与 termination；
- 按 particle ID + sample sequence 取得从 birth 到 termination/模拟结束的完整**离散轨迹**；
- manifest、resolved Case/Profile、provenance bundle、输入与输出 SHA；
- streaming validator、WAL checkpoint/truncate、并发只读、finite 与 lifecycle coverage 审计；
- 1 worker/4 workers、不同 chunk/order 下的 normalized deterministic output；
- 10 万粒子、1 小时、每 10 分钟输出的冻结真实场景在 2 GiB/512 MiB 门内运行。

## 4. 最终跨平台与性能边界

- Windows/WSL 不承诺文件字节或每个浮点 bit 完全相同；12 对全量逻辑行比较已证明无 key、状态、终止、
  validity/quality、nonfinite 或 presence blocker，数值差异已量化并按合同 `report_only` 接受；
- 最大时间坐标漂移为 1 ns；最大位置、风、压、温差均为浮点末位量级；
- Rust/native 六对无 hard blocker；CFSR W 的最大绝对差约 `6.94e-18 m/s`；
- WSL accepted baseline 只用于相同 runner identity、参数和测量方法的后续 25% 回退检查；
- 正式 50k→100k 比例为 forward `2.1387`、backward `2.2298`；
- WSL `perf` executable 缺失没有被伪装成 profile，但低扰动阶段计时、O1/O2 优化与正式 scaling 已完成
  性能因果和验收闭环。

## 5. 诚实边界

- M4 没有运行或要求百万粒子；两个旧 M3 百万点测试继续显式 ignored，后续常规性能测试按用户要求最高
  使用 10 万粒子；
- “完整轨迹”是按照 configured output interval 与 birth/termination lifecycle events 保存的完整离散序列，
  不是无限时间分辨率的连续解析曲线；
- 当前完成范围是冻结的三套资料与三类 population，不代表任意新资料、任意沉降/化学/湍流模块已经支持；
- 项目尚未发布，因此继续维护当前 v1，没有为返工制造 v2、兼容层或平行 schema；
- M4 完成不等于产品发布完成；后续里程碑仍可增加用户接口、更多物理过程、运维与发布工程。

## 6. 权威证据

- A1：`TRAJECTA_M4_A1_A_DELIVERY_REPORT.md` 与后续工程验收链；
- A2：`TRAJECTA_M4_A2_A_COMPLETION_REPORT.md`；
- A3：`TRAJECTA_M4_A3_A_COMPLETION_REPORT.md`；
- A4：`TRAJECTA_M4_A4_A_COMPLETION_REPORT.md`；
- 机器终审：`target/m4-a4/summary/M4_A4_A_FINAL_SUMMARY.json`；
- artifact index：`target/m4-a4/M4_A4_ARTIFACT_INDEX.json`；
- 跨平台差异：`target/m4-a4/platform-diff/M4_A4_WINDOWS_WSL_DIFF.json`；
- native 复核：`target/m4-a4/native-review/M4_A4_NATIVE_EVIDENCE_REVIEW.json`；
- WSL baseline：`target/m4-a4/baseline/M4_A4_WSL_BASELINE_ACCEPTED.json`。

## 7. 约束与提交状态

- A 未调用子代理；B 仅由用户按书面 Prompt 单独执行冻结机械矩阵；
- 未触碰 `E:\flexpart\origo-validation-v1.json`；
- 未放宽公式、容差、异常分类、caps 或 hard gate；
- 未增加无意义版本；
- 未 commit / 未 push。
