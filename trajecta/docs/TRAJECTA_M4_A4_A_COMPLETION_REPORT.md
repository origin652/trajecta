# Trajecta M4-A4：A 最终验收报告

状态：**M4-A4 complete**
日期：2026-07-27
仓库：`E:\flexpart\trajecta`
Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
提交状态：**未 commit / 未 push**

> 责任声明：正式长矩阵的机械执行由用户另行启动的 B 按 A 冻结 Prompt 完成；A 没有调用子代理，
> 也没有把差异裁决、baseline 接受或最终签署交给 B。A 本轮独立重算正式 SHA、逐格复核 hard gates、
> 全量比较 A2/A3 Windows/WSL SQLite、复核 Rust/native 与 oracle、运行当前源码 Windows/WSL 普通门禁，
> 并作出本文裁决。无 C 参与。

## 1. 裁决

A4 的退出条件现已全部满足：正式 WSL 50k/100k 六格、两方向 scaling、RSS、SQLite、执行期 I/O、
exact query reuse、1/4 worker 决定性、Windows/WSL 当前源码门禁、A2/A3 跨平台差异、Rust/native 复核、
baseline 与 artifact index 均通过或已按冻结政策诚实裁决。

因此 A 正式裁决：**M4-A4 完成**。

正式执行源码身份：

```text
8028c2eb403aa4bce6ff8776b7555d47201093b6080e4dc6924ab9fcbea70da1
```

正式 aggregate：

```text
target/m4-a4.6/post-stable-id-formal/summary/formal-20260727T093415Z.json
SHA-256 c946299179f6dc0ad56b1ffbd09f387ef7b7e25da0281d03cf7b5bbc3c71e868
aggregate.passed = true
```

## 2. 正式六格

| Cell | runner ms | peak RSS bytes | SQLite bytes | 结果 |
|---|---:|---:|---:|---|
| S50-F · forward · 50k · w4 | 202,401 | 725,340,160 | 102,432,768 | passed |
| S100-F · forward · 100k · w4 | 432,879 | 1,437,671,424 | 208,076,800 | passed |
| S50-B · backward · 50k · w4 | 206,057 | 665,329,664 | 101,646,336 | passed |
| S100-B · backward · 100k · w4 | 459,467 | 1,340,452,864 | 206,700,544 | passed |
| D100-F · forward · 100k · w1 | 683,509 | 1,398,718,464 | 208,076,800 | passed |
| D100-B · backward · 100k · w1 | 694,590 | 1,323,024,384 | 206,700,544 | passed |

六格全部为 `attempt-1`，无 partial attempt、无重试、无 external blocker。每格均满足：

- `RunOutcome::Complete`、manifest `complete`、abnormal termination = 0；
- lifecycle、mass ledger、finite、population 与 provenance semantic validation 通过；
- execute-phase `inspect/build_index/decode/provider_frame_load/format_detection_open_attempt` delta 全为 0；
- particle-loop exact query 没有重复执行同一 exact key；
- SQLite `integrity_check=ok`、terminal WAL=0；
- 100k RSS `< 2 GiB`，SQLite `< 512 MiB`；
- concurrent reader 满足 indexed-high-water 与 active snapshot 合同；
- backward boundary origin/direction 与 exact-birth termination 审计通过。

缩放：

```text
forward  = 432879 / 202401 = 2.13871967035736   <= 2.4
backward = 459467 / 206057 = 2.2298053451229514 <= 2.4
```

forward 与 backward 的 100k w1/w4 normalized content、canonical SQL、canonical output digest
分别完全一致；新六格 normalized triplet 也与 post-sorted-SQLite 对应代次一致。

## 3. Windows/WSL 全量差异裁决

A 使用全部逻辑行比较了：

- A2：三族 × 双方向，10,000 粒子，Windows 对 WSL，共 6 对；
- A3：三族 × 双方向，1,000 粒子，Windows 对 WSL，共 6 对。

比较没有抽样或截短，覆盖 `particle`、`particle_mass`、`output_event`、`particle_state`、
`termination` 全表。结果：

| hard 项 | 结果 |
|---|---:|
| logical key mismatch | 0 |
| status/termination/validity/quality 等 categorical mismatch | 0 |
| nonfinite | 0 |
| value-presence mismatch | 0 |
| blocked / not-comparable pair | 0 / 0 |

原始 `run_profile_sha256` 跨平台不同，原因是 resolved profile 含临时目录、输出目录和资料根路径；A 没有
忽略该差异，而是比较完整的 path-independent execution projection，并同时 hard-gate case SHA、dataset
lock/profile/content SHA、reader backend 与 numerical identity。12 对均可比。

浮点 exact mismatch 共 200,420 个，但完整轨迹数值差异按冻结合同为 `report_only`。观测最大值为：

| 字段 | max abs | max rel |
|---|---:|---:|
| longitude | `1.5565326805244695e-13 deg` | `1.0929041687900773e-13` |
| latitude | `1.1368683772161603e-13 deg` | `1.3835364475205148e-15` |
| height ASL | `2.9103830456733704e-11 m` | `3.2621722464413254e-14` |
| U | `1.2856382625159313e-12 m/s` | `1.7810505431140076e-11` |
| V | `1.5663026431411708e-12 m/s` | `1.7425272770625192e-10` |
| W | `4.1397441030710525e-14 m/s` | `8.272945826903016e-10` |
| pressure | `4.511093720793724e-10 Pa` | `5.411188428191401e-15` |
| temperature | `8.412825991399586e-12 K` | `3.034587350389567e-14` |
| ozone mass | `1.52587890625e-5 kg` | `6.976099532158784e-15` |

A2 pressure/hybrid 中另有 12 个整数字段 exact mismatch，分布在 6 个状态行的
`integration_offset_ns` / `elapsed_age_ns`，最大差异为 **1 ns**；物理 output time、状态、终止原因和覆盖
均相同。A 不为此新建容差，也不修改 RK2、边界或时钟；依照完整轨迹 `report_only` 政策接受这些有限、
已量化且不改变分类的跨平台舍入差异。

机器报告：

```text
target/m4-a4/platform-diff/M4_A4_WINDOWS_WSL_DIFF.json
SHA-256 28548e94f5e58965119f7698d8579fbec5e528e1b63e1d81083a7cc8cdef1bbc
```

## 4. Rust/native 与 oracle 复核

A 重新验证 A3 Rust/native 六对报告及其 summary、manifest、SQLite、bundle 当前 SHA：

- 6/6 pair 可用，0 logical-key mismatch；
- 0 categorical mismatch、0 nonfinite、0 value-presence mismatch；
- 261 个 numeric exact mismatch 全部来自 CFSR W 的末位舍入；最大绝对差
  `6.938893903907228e-18 m/s`；
- GPL PV60 scalar oracle 状态仍为 passed，10/10 records passed。

机器复核：

```text
target/m4-a4/native-review/M4_A4_NATIVE_EVIDENCE_REVIEW.json
SHA-256 e026158db56f194b44f6f243401443a67f5788e7d0c36e62bdecf1b47ceed1ef
```

## 5. A4.5 与 baseline 裁决

A4.5 的 WSL `perf` executable 缺失继续诚实记为 external blocker；冻结计划本身允许“可审计 CPU
profile，或诚实记录 profiler external blocker”。其余退出证据已经满足：

- instrumentation-on 3×w1 + 3×w4、instrumentation-off w4×3 完成；
- 观测开销 `0.37094%`；顶层计时解释约 98.96%～99.09% wall time；
- provenance 小块写放大、boundary stencil 重建均有数值归因并经 O1/O2 修复；
- 后续正式矩阵证明正确性、确定性与两方向 scaling。

因此 A 裁决 profiler 缺失不阻断 A4.5/A4。

首个完整合格的 post-stable-ID 100k w4 正反向结果已接受为 WSL baseline；后续只有 runner identity、
参数与测量方法可比时才应用 25% 回退门：

```text
target/m4-a4/baseline/M4_A4_WSL_BASELINE_ACCEPTED.json
SHA-256 5a6a3b3886149ec62af030c58a0cd182b4aecbb391a5b5c98577c361743e843c
```

## 6. 当前源码普通门禁

Windows 与 WSL Ubuntu-24.04 均实际执行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile ...
python tools/validate_m4_a0_contracts.py
git diff --check -- .
```

两平台结果均为 `455 passed / 11 ignored`，其余门禁通过。11 个 ignored 包含显式真实长链入口与两个
旧 M3 百万点门禁；本报告没有把 ignored 冒充执行，也没有运行百万点。

## 7. 最终机器交付

```text
target/m4-a4/summary/M4_A4_A_FINAL_SUMMARY.json
SHA-256 07d31e27255903c0f1af817b3213e946c14c3ddfa08e2f350cfa04c4a73a06d1

target/m4-a4/M4_A4_ARTIFACT_INDEX.json
```

最终 summary：`implemented=true`、`executed=true`、`passed=true`、`blocked=false`、无 failures、无待 A
裁决项。artifact index 的最终 SHA 在报告写入后重新生成，以避免自引用。

## 8. 约束与提交状态

- 未修改或读取 `E:\flexpart\origo-validation-v1.json`；
- 未增加 schema/manifest/numerical-contract v2；
- 未放宽容差、caps、异常分类或 hard gate；
- 未运行百万点；
- 未 commit / 未 push。
