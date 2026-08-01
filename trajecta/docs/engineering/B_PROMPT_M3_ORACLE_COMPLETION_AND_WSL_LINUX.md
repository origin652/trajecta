# 给 B：补齐三族 FLEXPART Oracle 与 WSL Linux 全矩阵

你接手的是 A 已完成真实 ERA5 pressure A-stage oracle 和 WSL core gate 后的剩余工程
工作。先完整阅读：

- `docs/engineering/TRAJECTA_M3_A_ORACLE_WSL_STAGE_REPORT.md`
- `docs/engineering/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`
- `docs/engineering/TRAJECTA_M3_MET_QUERY_PLAN.md`
- `testdata/M3_TOLERANCES.v1.json`
- `testdata/M3_FLEXPART_ORACLE.schema.json`
- `testdata/M3_COMPARISON_REPORT.schema.json`
- `tools/flexpart_oracle/README.md`

本轮不宣称 M3/A2 完成，不 git commit。你只交工程实现和实跑证据，科学裁决仍由 A
完成。

## 一、冻结边界，禁止修改

不得修改：

- FLEXPART commit：`dace3affa2ba71677f12f3858b04aaf59f8ee51e`；
- 三份 formal query 的记录、顺序、point ID、地形选点和 SHA；
- `M3_TOLERANCES.v1.json` 的任何规则、阈值或 selector；
- Trajecta 插值、垂直坐标、密度、几何 W、地形高度或近地层科学公式；
- FLEXPART legacy 算法；
- default REAL=32 bit 和禁止 `-fdefault-real-8` 的合同；
- MIT/GPL 隔离边界。

当前 12 个 common-semantics hard failure 必须原样保留和复现。不得为了变绿而放宽
容差、换点、删记录、改时间、改单位、改高度定义或做结果后处理。

## 二、补齐 ERA5 pressure 75/75

当前已有：

- `native_anchor` 27/27；
- `interpolated_common` 18/18。

需要实现：

- `surface_layer` 12/12：三类地形、两个中点时刻、AGL 10 m/50 m；
- `modern_difference` 18/18：W、density、geometric terrain，三类地形、两个时刻。

要求：

- GPL 侧继续调用真实 FLEXPART 例程和 PBL/profile 路径；
- raw oracle 不做容差裁决；
- 只有 FLEXPART 科学上确实无对应量时才允许明确 `not_available`，不能以“尚未实现”
  代替记录；
- surface/modern 结果按冻结 registry 的 `report_only` 规则进入 MIT difference report；
- common semantics 仍使用 hard gate；
- 扩展 `adjudicate_flexpart_oracle` 时不能把 report-only 与 hard-gate slabs 混为一个
  comparison target。

ERA5 pressure 完成标准：raw oracle `status=complete`、75 条均有明确科学结果、schema
通过，并生成 common-semantics report、difference report 和 family rollup。

## 三、实现 CFSR pressure 75/75

输入固定为官方 NCEI 00/06/12：

- `pgbl00.gdas.2009010100.grb2`；
- `pgbl00.gdas.2009010106.grb2`；
- `pgbl00.gdas.2009010112.grb2`。

要求：

- 使用 `pressure_meter` 构建；
- GPL 侧可用 ecCodes 做最小 loader，但不得调用 Trajecta reader；
- loader 后必须进入 FLEXPART `verttransform_ecmwf`、`interpol_wind`、
  `interpol_partoutput_val` 等真实路径；
- 地形使用 formal query 已冻结的 CFSR orography 身份，00/06/12 必须一致；
- 27 native + 18 common + 12 surface + 18 modern，共 75 条；
- 冻结业务文件、loader、binary、harness、query 和 Profile SHA。

不得用 ERA5 结果、CF 派生 NetCDF 或简化插值器冒充 CFSR FLEXPART oracle。

## 四、实现 ERA5 hybrid 137 层 75/75

要求：

- 使用 FLEXPART `ETA` 构建；
- 137 个完整 model levels，00/03/06 三时次；
- 正确加载完整 hybrid PV A/B、`lnsp`、surface geopotential 和必要近地场；
- surface pressure 必须由冻结语义 `exp(lnsp)` 得到；
- native level 按 1-based from top 的 query 合同解析，输出中记录 resolved coordinate；
- eta-dot/omega 不能互相冒充，必须按 FLEXPART 实际 build/path 记录；
- 同样完成 27 + 18 + 12 + 18 = 75 条。

必须提供 `nm` 或等价符号证据，证明 ETA binary 实际包含并调用预期 FLEXPART 例程。

## 五、把 MIT runner 泛化到三族

扩展 `tools/run_m3_oracle_comparison.py`：

- 逐族运行，不复用旧输出；
- 每族先清理 canonical subject/report；
- 校验 oracle/query/Profile/manifest/资料文件 SHA；
- 校验 oracle 和 report Draft 2020-12 Schema；
- 检查新鲜度，拒绝 stale artifact；
- 每族分别输出 common-semantics hard-gate report 与 modern/surface difference report；
- 最后输出三族 rollup；
- 明确区分 `passed`、`hard_gate_failed`、`partial`、`external_blocked`；
- raw oracle partial 不能掩盖已知 hard failure；
- report-only 超尺度不能伪装成 hard-gate pass，也不能自动改 registry。

保留最坏 point ID、两边数值、FLEXPART routine、Trajecta provenance 和规则 ID，供 A
终审。

## 六、完成 WSL Linux 全矩阵

使用现有发行版：

```text
Ubuntu-24.04 / WSL2
```

推荐环境：

```bash
env -u LD_LIBRARY_PATH \
  RUSTUP_TOOLCHAIN=1.94.0-x86_64-unknown-linux-gnu \
  CARGO_TARGET_DIR=target/wsl-cargo \
  TRAJECTA_RUN_MILLION=1 \
  bash tools/linux_m3_gate.sh
```

必须实际运行：

- fmt；
- Clippy `-D warnings`；
- workspace tests/doc；
- ERA5 pressure/hybrid 与 CFSR 查询链；
- 三套 Rust/native 全场比较；
- 三套 FLEXPART oracle；
- 三套 MIT comparison；
- 百万点长测、inverse permutation、1/多线程、冷热缓存和 IO delta=0；
- 所有 Schema 校验。

当前 WSL 已安装 netCDF-C、HDF5、ecCodes、gfortran、libclang、Rust 1.94 的 rustfmt 和
Clippy。若仍遇外部阻断，保留命令、退出码和完整日志，不得软跳过后写成 passed。

另外：backend report ID 中仍有历史 `windows-fullfield` 名称。不要直接修改冻结 matrix
或 registry；先在交付报告中标为 legacy identity，若需发布跨平台新名字，提交最小
迁移方案给 A 裁决。

## 七、科学结果处理

当前 ERA5 pressure 已知最大差异约为：

- 风 0.486529 m/s；
- 温度 0.195572 K；
- specific humidity 1.237576e-4 kg/kg；
- 压力 189.629 Pa；
- geopotential height 26.766 m。

这些结果不得“修到通过”。三族齐全后只做以下工作：

1. 证明差异可重复；
2. 按 family/scope/coordinate/field 汇总；
3. 标出共性与数据族特有差异；
4. 保留 raw evidence；
5. 交 A 决定是维持 hard failure、形成算法差异 ADR，还是发布新的有界规则。

## 八、门禁和交付

除 Linux full gate 外，仍需通过：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- \
  --skip cfsr_pgbl_million_point_budget_and_threading \
  --skip cfsr_pgbl_million_point_performance_matrix
cargo doc --offline --workspace --no-deps
```

交付报告必须逐项列出：

- implemented；
- executed；
- passed；
- hard_gate_failed；
- partial；
- external_blocked；
- artifact path、size、SHA-256；
- runner OS、Rust/Python/Fortran、netCDF/HDF5/ecCodes/libclang 版本；
- 三个 family 各 75 条的 scope 统计；
- `nm` 符号证据；
- 未完成项。

不要 commit，不要宣称 M3/A2 完成，等待 A 最终验收和科学裁决。
