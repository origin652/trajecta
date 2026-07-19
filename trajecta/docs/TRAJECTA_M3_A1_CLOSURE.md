# Trajecta M3 A1 科学合同收尾

状态：2026-07-16 已实现并通过 A1 门禁；不代表 M3 或 A2 完成。

## 1. 本轮裁决

### 1.1 公共物理维度

- `trajecta_case::quantity::Dimension` 统一为 SI 基本量指数：质量、长度、时间、热力学温度。
- 原有 `dimensionless/time/length/pressure/mass/temperature/velocity` serde 字符串保持兼容；复合维度使用固定对象 `{mass,length,time,temperature}`。
- Profile 图的 `DimensionVector` 直接复用公共 `Dimension`，不再维护第二套维度系统。
- canonical 语义冻结真实维度，包括 pressure tendency、density、geopotential、potential vorticity、precipitation rate 和 energy flux。
- `FieldRegistry::canonical()` 是唯一公共完整 canonical registry；符号正确但维度错误也会拒绝。

### 1.2 CFSR 热通量和动量输入

- NCEP GRIB2 热通量源按官方约定解释为向下为正。
- CFSR Profile 先发布 namespaced downward-positive 扩展源，再用显式一元负号派生向上为正的 canonical sensible/latent heat flux。
- provenance 的 `profile_frame_graph` transform 同时记录派生 node 和原始 expression，因此翻号可审计。
- `FRICV` 与 `(UFLX,VFLX)` 是互斥替代路径：存在 FRICV 时不读取、不要求两分量应力；没有 FRICV 时才由应力诊断 `u*`。
- 不再把 `frame_interval_seconds=21600` 当作 `pgbl00` UFLX/VFLX 六小时平均的证据；CFSR 当前正式 Profile 不映射这两个不可靠 interval 字段。
- centre 7 的 `0-2-197=FRICV` 与 `0-3-196=HPBL` 继续冻结。

官方依据：

- NCEP GRIB2 Table 4.2：通量正方向约定为向下；
- NCEP discipline 0/category 2 local table：参数 197 为 FRICV；
- NCEP discipline 0/category 3 local table：参数 196 为 HPBL；
- NCEI CFSR inventory：`flxf00` 是 `0-0 day ave`，`flxf06` 才是 `0-6 hour ave`。

## 2. Explain 合同

- `ExplainMode::Disabled` 是默认值；关闭时执行器不创建逐点 Explain Vec，`output.explain()` 返回 `None`。
- `ExplainMode::Full` 在 plan 编译时冻结，`prepare_batch` 把 Explain 开销计入每点内存模型。
- Full 记录包含 domain、cell、前后帧及时间权重、水平四角和实际权重、bilinear/valid-triangle/time-blended 方法、垂直路径与括号、surface model、逐字段 quality 和 provenance。
- CLI `--explain` 编译 Full plan；没有该参数时不得在引擎内部先生成再丢弃。

## 3. 关键实现

- `crates/trajecta-case/src/quantity.rs`
- `crates/trajecta-met/src/profile/graph.rs`
- `crates/trajecta-met/src/field/mod.rs`
- `crates/trajecta-met/src/derive/surface.rs`
- `crates/trajecta-met/src/query/request.rs`
- `crates/trajecta-met/src/query/output.rs`
- `crates/trajecta-met/src/query/engine.rs`
- `crates/trajecta-met/profiles/cfsr_pgbl_pressure_v0.yaml`
- `crates/trajecta-cli/src/command/met.rs`
- `crates/trajecta-met/tests/real_m3_query_chain.rs`

## 4. 验证结果

2026-07-16 在 Windows 工作区实际运行：

```text
cargo fmt --all --check                                      OK
cargo clippy --offline --workspace --all-targets -- -D warnings
                                                               OK
cargo test --offline --workspace                              191 passed
cargo doc --offline --workspace --no-deps                     OK
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met --tests
                                                               113 passed
```

真实 CFSR 2009-01-01 00/06 UTC 的 03 UTC 中间时刻也通过 CLI 正向实跑：默认模式不输出 `explain`，`--explain` 模式输出 domain/cell、时间与水平权重、垂直路径、surface model 和 8 个字段的 quality/provenance。

CLI 直接指向同时含 `pgbl/flxl/spllnl` 的原始业务目录时，会把不属于该 Profile 的文件也交给同一锁构建流程并失败。该问题不影响 A1 引擎和科学合同，但属于 B 的数据发现/CLI 编排缺陷，已列入 `B_PROMPT_M3_PHASE2_4.md`。

## 5. 尚未完成

以下仍属于 B1 Phase 2--4 或后续 A2，不得因本轮通过而标记 M3 完成：

- CFSR 00/06/12 三连续时次；
- ERA5 hybrid 全层加近地正式锚点；
- ERA5 pressure 全字段正式锚点；
- Rust/native 全字段差分；
- Windows/Linux 真实门禁；
- 百万点预算与确定性；
- FLEXPART oracle 及 A2 科学终审。
