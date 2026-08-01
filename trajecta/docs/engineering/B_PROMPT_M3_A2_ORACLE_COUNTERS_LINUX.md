# 给 B：M3 A2 剩余工程链执行 Prompt

你接手的是 A 已冻结后的工程实现。不要修改科学公式、容差数值、selector、
FLEXPART commit、query 最小矩阵或 MIT/GPL 边界；发现矛盾时停止对应分支并提交
最小复现给 A。

本轮不宣称 M3 完成，不 git commit。必须区分“实现”“实际运行”“外部阻断”。

## 一、先读并锁定

必须先完整阅读：

- `docs/engineering/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`
- `testdata/M3_TOLERANCES.schema.json`
- `testdata/M3_TOLERANCES.v1.json`
- `testdata/M3_TOLERANCE_CALIBRATION.v1.json`
- `testdata/M3_FLEXPART_ORACLE.schema.json`
- `testdata/M3_COMPARISON_REPORT.schema.json`
- `docs/engineering/TRAJECTA_M3_MET_QUERY_PLAN.md`

容差 registry 是 A 冻结输入。B 不得根据本轮结果调整阈值，不得增加宽松 fallback。

## 二、实现真实 reader/provider 调用计数器

建议模块：`crates/trajecta-met/src/io/metrics.rs`。

实现 engine/reader 所有权范围内的 `IoCallCounters` 与不可变 snapshot，至少统计：

- inspect；
- build index；
- decode；
- provider/frame load；
- 若现有抽象可可靠捕获，再统计 filesystem open。

合同：

- 不使用全局静态计数器；由 engine/reader/provider 共享一个 `Arc`；
- 在真实生产入口计数，不允许测试 mock 自报零；
- Rust/native、成功/失败路径都计数；
- `prepare` 可以增加计数；
- `PreparedBatch::execute` 前后 snapshot 必须完全相同；
- 多线程使用原子计数，计数器不得改变结果或缓存行为。

补百万点和小批测试：证明 prepare 期间至少发生真实读取，execute delta 为零；同时
保留 1/多线程、chunk、排列、冷热缓存和预算断言。

## 三、实现 registry 解析与统一比较器

建议模块：

- `crates/trajecta-met/src/validation/tolerance.rs`
- `crates/trajecta-met/src/validation/report.rs`

要求：

1. 精确实现合同中的 rule selector；`all` 展开后零匹配为 `unvalidated`，多匹配为
   配置错误。
2. 实现 numeric/bitwise exact、ordered binary64 ULP、absolute-relative 和 vector
   metric；公式不得自行变更。
3. metadata、mask、unit、status、validity 和 non-finite 在数值门槛前处理。
4. 所有有效样本逐点 hard gate，不得用平均值或 percentile。
5. report-only 只输出 `reported`，不能改变 hard-gate 总结。
6. 输出必须验证 `M3_COMPARISON_REPORT.schema.json`。
7. `passed` 必须覆盖完整、至少有一个 hard gate、无 blocker；缺 case/oracle failure
   为 `incomplete`；无规则为 `unvalidated`。

必须写合成负例：阈值边界、超 1 ULP、signed zero、NaN/Inf、mask 不同、单位不同、
零匹配、多匹配、report-only 超尺度、缺 coverage。

## 四、把 native 测量接入正式 registry

更新现有 native 测量链：

- ERA5 pressure：两文件、三时次、全部冻结源字段；
- ERA5 hybrid：两文件、三时次，必须含 `lnsp`；
- CFSR：00/06/12 三个官方 GRIB，而不是只测 00；
- 输出 raw measurement 后再由统一比较器生成 comparison report；
- CFSR q/omega 使用 registry 的 2 ULP，其他字段 exact；
- 不再在测试代码里保留可被误认作认证阈值的 legacy `1e-*` 参数。

Windows 本轮应产生三个 backend comparison report。Linux 运行相同冻结输入后，
cross-platform 尚无 registry 规则时必须诚实输出 `unvalidated`，不得复用 backend
规则冒充跨平台认证；将原始测量交 A 冻结下一版规则。

## 五、实现真实 FLEXPART GPL oracle harness

FLEXPART checkout 固定 commit：

```text
dace3affa2ba71677f12f3858b04aaf59f8ee51e
```

实现边界：

- harness 属 GPL 侧，文件带 `SPDX-License-Identifier: GPL-3.0-or-later`；
- 不复制 FLEXPART 源码到 MIT crate，不让 Cargo/build.rs 编译或链接 GPL；
- harness 只引用独立 FLEXPART checkout；
- pressure/CFSR 构建 `pressure_meter` 非 ETA 版本，ERA5 hybrid 构建 ETA 版本；
- 不要求生成传统 FLEXPART 运行目录或执行粒子模拟；
- 原 FLEXPART reader 能无歧义读取时使用它，否则在 GPL 侧用 ecCodes/netCDF-C
  实现最小 loader，填充 FLEXPART 气象数组；loader 不得调用 Trajecta reader；
- loader 后必须使用 FLEXPART 的垂直处理、`interpol_wind`、
  `interpol_partoutput_val` 等实际路径，不得重写简化插值器；
- 锁定编译器、flags、preprocessor definitions、`default_real_bits`、binary、loader
  和 harness SHA；v1 variant 必须实际断言默认 REAL 为 32 bit，不能使用
  `-fdefault-real-8`。

生成 query 文件时严格执行 A 合同的 sea/plain/mountain 选择、27 native anchors、18
interpolated-common、12 surface-layer、18 modern-difference；每数据族至少 75 条。
query、记录顺序和 point ID 必须稳定，并冻结 SHA。

原始输出必须验证 `M3_FLEXPART_ORACLE.schema.json`：

- complete 时 records 非空且 failures 为空；
- 失败写 partial/failed + failure；
- 无 NaN/Inf；invalid sample 省略 value；
- 不在 GPL raw artifact 内做容差裁决。

现有 `generate_oracle_stub.py` 不是 v1 科学 oracle。可以保留为明确失败的环境探针，
但不得继续产出旧 v0 形状或被主门禁当成成功。

## 六、MIT 侧 oracle 比较

MIT 侧只读取冻结 oracle JSON 和 Trajecta 对同一 query 的输出：

- common semantics 使用 registry hard gate；
- W、density、geometric terrain、surface-layer 使用 report-only；
- point ID、status、validity、unit 必须精确关联；
- oracle commit/query/profile/data hash 任一不符直接 incomplete；
- 生成每数据族独立 report 和一份 rollup，不自动修改 registry。

如果真实 FLEXPART 结果超过预注册 hard gate：保留原始 artifact、最坏点、两边数值、
FLEXPART routine 和 Trajecta provenance，交 A 裁决；B 不得放宽门槛。

## 七、Linux 门禁

至少提供一个可在干净 Linux x86_64 环境执行的脚本/CI job，实际运行：

- fmt、clippy `-D warnings`、workspace tests；
- 三套真实资料查询链；
- pure-Rust；
- 可用时 native-netcdf/native-eccodes；
- 百万点计数器/确定性矩阵；
- oracle harness 构建与三套 oracle；
- comparison report Schema 校验。

不得只写 YAML 不运行。报告必须附 runner OS、工具链、外部库版本、命令、退出码和
artifact SHA。若当前没有 CI 权限，先交可复现 Linux 脚本和本地/容器实跑日志，状态
标 `external_blocked`，不要写通过。

## 八、门禁与交付报告

常规门禁：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- \
  --skip cfsr_pgbl_million_point_budget_and_threading \
  --skip cfsr_pgbl_million_point_performance_matrix
cargo doc --offline --workspace --no-deps
```

另外必须实际跑：

- 两条百万点长测；
- Windows native 三套 comparison report；
- 三套 FLEXPART oracle 与 comparator；
- Linux 矩阵或明确外部阻断证据。

交付报告按以下四栏逐项写：`implemented / executed / passed / blocked`。明确列出所有
artifact 路径和 SHA；不宣称 M3/A2 完成，不 commit，等待 A 最终审计。
