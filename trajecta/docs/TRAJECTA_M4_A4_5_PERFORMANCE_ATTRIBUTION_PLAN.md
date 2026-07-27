# Trajecta M4-A4.5 性能归因与优化决策计划

状态：**completed by A（2026-07-27）**。观测矩阵、instrumentation-off baseline、O1 provenance buffering、
O2 boundary stencil reuse 与后续正式 scaling 验证均完成；WSL profiler 诚实标为 `external_blocked`，按本计划
允许的分支不构成阻断。A4 与 M4 的最终签署见相应完成报告。

## 0. 当前执行检查点（2026-07-25）

- A 已实现阶段墙钟、固定 64 桶直方图、boundary 路径分布、schema、validator、聚合器和 WSL profiler 驱动；
- B 已按冻结 Prompt 完成 3×(w1/w4) attribution 与 3× instrumentation-off baseline；观测开销约 0.37%，三类 normalized digest 一致；
- profiler 因 WSL 无 `perf` executable 标为 `external_blocked`；
- O1 用 128 KiB 有界缓冲消除 drvfs 上的小写放大，w4 median 48.976 s → 12.749 s；
- O2 在硬执行预算内保留跨 boundary path exact stencil，并跳过容量内无意义 eviction 排序；3× w4 median 12.749 s → 10.719 s，boundary 下降 53.41%；
- O2 3/3 `Complete`、abnormal=0、execute I/O delta=0，content/SQL/canonical digest 与 O1 完全一致；
- 50k/100k、peak RSS、SQLite size、writer concurrency 与正式 scaling 已在 post-stable-ID 六格矩阵完成；
- 当前实现与验证报告见 `TRAJECTA_M4_A4_5_A_IMPLEMENTATION_REPORT.md`，机械执行 Prompt 见 `B_PROMPT_M4_A4_5_EXECUTION.md`。

## 1. 定位

M4-A4.5 是 M4-A4 预检与正式 50k/100k 长矩阵之间的性能归因关卡。它回答的不是“还能不能再改快一点”，而是：当前真资料运行时间分别消耗在什么阶段、主要瓶颈是否可并行、下一项优化的理论上限是多少。

已知背景仅作为立项依据，不作为 A4.5 验收结果：

- WSL ERA5-hybrid forward、1,000 粒子、4 workers 的原始路径约为 98.415 s；
- 当前候选路径 attempt-19 为 50.876 s，正确性门禁通过；
- 单次运行约有 38,918 次 boundary/boundary-corners 请求；
- 单纯扩大缓存曾显著减少 miss，却把运行时间恶化到 55.473 s，因此不能再用 miss 数量代替性能归因。

## 2. 目标与非目标

目标：

1. 同时取得 CPU 采样、墙钟阶段计时、并行缩放和路径分布证据；
2. 区分数值计算、boundary 求根/证书、window preparation、网格定位、cache 锁等待、stencil 构建、内存分配和输出开销；
3. 冻结可复现的 primary/secondary bottleneck；
4. 只基于测量结果授权下一项单一优化，并给出可解释的最大收益上限。

非目标：

- 本阶段不运行正式 50k/100k A4 矩阵；
- 不修改科学公式、容差、边界判定或数值路径来“修绿”；
- 不在证据出现前引入矩阵化、SIMD、新缓存架构或批量求根；
- 不把 profiler 可运行写成 profiler 已运行；
- 不形成 M4-A4 或 M4 完成裁决。

## 3. 冻结场景

默认归因场景沿用 A4 P0：

- WSL Ubuntu；
- ERA5 hybrid；
- forward；
- 1,000 粒子；
- 4 workers；
- 1 小时模拟、600 秒步长、600 秒输出；
- 相同输入 SHA、case、profile、seed、release binary 和环境身份。

并行对照只增加同场景 1 worker。需要判断测量噪声时，每格最多运行 3 次并取 median；每个 attempt 必须单独保留，禁止覆盖或只保留最佳值。

## 4. 执行阶段

### A4.5-0：baseline 身份冻结

- 记录源代码 commit/diff identity、release binary SHA、Rust/LLVM、WSL kernel、CPU、输入文件 SHA；
- 冻结 case/profile/seed 和 artifact 目录；
- 重算 complete、abnormal、mass ledger、SQLite integrity、execute I/O 和 normalized digest；
- profiler 或计时代码不得改变规范输出。

### A4.5-1：CPU sampling profile

首选 WSL `perf record`，release 保留有限调试符号并启用 frame pointers。产物至少包含：

- `perf.data`；
- folded stacks；
- flamegraph；
- inclusive/self CPU top functions；
- unresolved sample 比例和 profiler 配置。

`perf` 权限或 WSL 内核能力不足时必须标为 `external_blocked`，不得用无调用栈的猜测替代。

### A4.5-2：低扰动墙钟阶段计时

计时器只做线程本地或低竞争累计，不逐调用写日志。至少拆分：

- preload / runner / integrator / boundary / output-sink；
- boundary path segmentation；
- root/certificate；
- `prepare_for_domain` / window preparation；
- grid locate + horizontal weights；
- column-cache lock wait 与 lock hold；
- cache lookup/insert/trim；
- `ColumnStencil::build`；
- boundary column sampling；
- roughness、transport floor 与 vertical bounds。

每项同时记录 calls、total ns、maximum ns 和固定分桶直方图。阶段总和应能解释至少 85% 的 runner wall time；计时开启相对关闭的 median 开销目标不超过 3%。

### A4.5-3：并行与锁竞争对照

运行同一场景 1 worker 与 4 workers，要求 normalized output digest 一致。记录：

- wall time、CPU time 和 worker 利用率；
- cache lock wait/hold；
- context switches；
- 每阶段缩放比。

若 4 workers 无明显收益或变慢，必须先裁决串行段/锁竞争，不能直接增加线程数。

### A4.5-4：条件式硬件与分配分析

仅在 A4.5-1/2 指向相关热点时启用：

- `perf stat`：cycles、instructions、IPC、branches、branch misses、cache misses、context switches；
- allocator/heap profiler：allocation count、bytes、热点调用栈；
- 容器审计：临时 `Vec`、`BTreeMap`、字符串 key 和 Arc/pin clone。

不得为了“证据更全”无条件增加重型 profiler。

### A4.5-5：粒子与路径长尾

按粒子/step 聚合但不逐点落盘：

- boundary probes；
- corner probes；
- root/certificate recursion depth；
- grid-cell crossings；
- cache hit/miss；
- top 1% 粒子占用的调用数和时间比例。

该阶段用于判断 38k 级查询是均匀成本还是少数近地面、网格边界或 outflow 粒子的长尾。

### A4.5-6：归因报告与单一优化授权

报告必须给出：

- primary bottleneck 与 secondary bottleneck；
- CPU profile 和 wall-clock profile 是否一致；
- 1/4 worker 缩放解释；
- 已解释 wall time 比例；
- 下一项单一优化、预计影响范围和理论最大收益；
- 被证据否决的方案。

只有以下条件满足时才允许矩阵化/SIMD：密集数值循环被证明是主要热点、数据布局适合批处理，且其 self/inclusive time 足以覆盖实现复杂度。若热点是锁、树结构、字符串、分配或自适应控制流，则矩阵化不进入下一轮。

## 5. Artifact 布局

计划使用：

```text
target/m4-a4.5/
  baseline/<attempt>/
  profiles/<attempt>/perf.data
  profiles/<attempt>/flamegraph.svg
  timings/<attempt>/stage-timings.json
  scaling/<attempt>/worker-scaling.json
  paths/<attempt>/boundary-path-histogram.json
  summary/M4_A4_5_PERFORMANCE_ATTRIBUTION.json
```

所有 JSON 必须包含 schema version、binary/input identity、命令、exit code、开始/结束时间和完整/失败状态。

## 6. 退出条件

M4-A4.5 只有同时满足以下条件才可由 A 裁决完成：

1. baseline correctness、digest、I/O 和 artifact identity 完整；
2. 有可审计 CPU profile，或诚实记录 profiler external blocker；
3. 阶段计时解释至少 85% wall time，测量扰动已量化；
4. 1/4 worker 对照完成且数值摘要一致；
5. 粒子/路径长尾已量化；
6. primary/secondary bottleneck 有数值占比，不只给主观判断；
7. 下一项优化限定为一个可回退假设，并冻结验收指标；
8. A 给出书面裁决。

退出后才决定是否实施下一项优化，以及何时恢复 M4-A4 正式 50k/100k 矩阵。

## 7. 职责与约束

- A 负责 profiler 设计、计时器实现、数值/性能解释、优化授权和最终裁决；
- A 不调用子代理；
- 如用户另行使用 B，B 只能依据 A 写出的书面 Prompt 执行已冻结的机械命令和整理 artifact，不得修改数值核心或自行裁决；
- 本计划创建时不执行任何命令矩阵，不 commit、不 push、不修改 `origo-validation-v1.json`。

## 8. A 最终裁决（2026-07-27）

- instrumentation-on 3×w1 + 3×w4 与 instrumentation-off w4×3 均完成，观测开销 `0.37094%`；
- 顶层计时解释比例约 `98.96%～99.09%`，primary bottleneck 的 provenance 小写放大经 O1 消除，
  secondary boundary 重建经 O2 缩减；
- profiler 分支因 WSL 无 `perf` executable 保持 `external_blocked`，没有伪造 profile；本计划退出条件
  明确允许诚实记录该外部阻断；
- 后续正式六格在相同科学输出身份下通过两方向 scaling、RSS、SQLite、I/O、query、lifecycle 与确定性门；
- 因此 A 裁决 M4-A4.5 完成，profiler 缺失不再是 M4 blocker。
