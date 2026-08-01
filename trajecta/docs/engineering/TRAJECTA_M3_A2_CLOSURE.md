# Trajecta M3 A2 最终验收与完成签署

日期：2026-07-20
验收对象：当前工作树（基线 `1697c24` 加本轮未提交修改）

## 结论

M3 的计划完成条件已经全部取得实际证据，A2 判定为 **通过**，M3 状态改为
**完成**。本结论没有修改 frozen query、正式资料锚点、Trajecta 科学算法、
FLEXPART commit 或任何数值阈值。

机器汇总的最终 Linux 状态为：

```text
core_status=0
a2_full_status=passed
a2_reasons=
backend_ran=1 oracle_ran=1 oracle_compare_ran=1 million_ran=1
```

本签署表示 M3 气象查询合同、真实资料链、跨后端/跨平台验证、FLEXPART 分层
oracle 和性能确定性门槛已经闭合；不表示 FLEXPART 是现代科学真值，也不表示
Trajecta 已通过独立观测证明在所有轨迹场景中优于 FLEXPART。

## 1. 冻结身份

| 项 | 身份 |
|---|---|
| algorithm | `trajecta/met_query/m3/v0` |
| tolerance registry | `m3-a-tolerance/v1.0.2` |
| registry SHA-256 | `882696e89cd7f41c57abc020ba92568b8595ffd274ba917b79d1606d8fe5a214` |
| backend matrix SHA-256 | `3bd43bc57f6fcc1a467cfd9fc50dfd038435aed2bd222c637c4f95f76f7355b4` |
| calibration SHA-256 | `c097417f96cde49751372bb56cac141766f1f660830b74b879adbf7514ffb520` |
| FLEXPART commit | `dace3affa2ba71677f12f3858b04aaf59f8ee51e` |

registry 的 `measured_partial` 状态继续诚实表示阈值校准包来自 Windows 单主机。
Linux 结果用于独立验证同一冻结阈值，没有反向拟合或放宽 registry，因此不需要发布
新的容差版本。

## 2. Windows 证据

Windows 最终门禁已通过：

- `cargo fmt --all -- --check`；
- `cargo clippy --offline --workspace --all-targets -- -D warnings`；
- workspace 239 项测试（包含两项百万点长测）；
- `cargo doc --offline --workspace --no-deps`；
- ERA5 pressure、ERA5 hybrid、CFSR pressure 三套 Rust/native 全场比较；
- 三套 FLEXPART raw oracle 75/75 与 MIT comparison；
- 三套共同语义 hard failure 均为 0。

Windows 的 registry 科学裁决详见
`TRAJECTA_M3_A2_ORACLE_REGISTRY_V1_0_2_DECISION.md`。

## 3. WSL Linux full gate

环境：

```text
Ubuntu 24.04 / WSL2 / x86_64
Linux 6.18.33.2-microsoft-standard-WSL2
Rust/Cargo 1.94.0
Python 3.12.3
netCDF-C 4.9.2
HDF5 1.10.10
ecCodes 2.34.1
libclang 18
GNU Fortran 13.3.0
```

实际执行：

```text
RUSTUP_TOOLCHAIN=1.94.0
CARGO_TARGET_DIR=/tmp/trajecta-m3-cargo-target
TRAJECTA_RUN_MILLION=1
bash tools/linux_m3_gate.sh
```

全部退出码：

```text
fmt=0
clippy=0
tests=0
doc=0
era5_chain=0
backend_cmp=0
oracle_raw=0
oracle_compare=0
million=0
schema_validate=0
```

`ARTIFACTS.json` 收录 54 个证据文件，`schema_errors=[]`。关键机器文件：

| artifact | SHA-256 |
|---|---|
| `target/m3-linux-gate/STATUS.txt` | `6407e99b0893bc04c406e5bd952e7768624bc7cef7feb0327d96774f20409c6c` |
| `target/m3-linux-gate/exit_codes.txt` | `972c2409cb829824a5d75ba86460705ffef1ababf1fade1e6e5896e2304310de` |
| `target/m3-linux-gate/ARTIFACTS.json` | `d868ee1f0fdceaf6c7776ddbe6dff7f225bef568229c3eb1d964efd10f624191` |
| WSL gate 完成时索引的 `target/m3-million-point-perf.json` | `9aa81982dca94492a052bbd68beb6e0353b8665212a7faead1533ce59a74ae3b` |
| `target/m3-oracle/FAMILY_ROLLUP.json` | `dbb9e78aa4c0d492e610920a6a595175c9d578d46bcc563d5db228b72750e2d5` |

WSL gate 完成后又执行了 Windows 全 workspace 回归；Windows 性能测试会写同一个
非版本控制 `target/m3-million-point-perf.json`，因此当前共享路径已被 Windows
结果覆盖，当前 SHA-256 为
`39e9f596fee71984e0410e206dd543131075fe0f78bda69b29d3d2791435d411`。
WSL 运行时文件的身份由不可歧义的 `ARTIFACTS.json` 条目和 gate 日志固定；该覆盖
不改变 WSL 的退出码、Schema 汇总或 A2 结论。

## 4. Backend 与 FLEXPART oracle

### 4.1 Rust/native 全场

| family | compared values | status |
|---|---:|---|
| ERA5 pressure | 945,747 | passed |
| ERA5 hybrid | 2,829,123 | passed |
| CFSR pressure | 5,897,232 | passed |

三套报告均无 blocker。Linux artifact 的 `runner` 明确记录 Linux/WSL。冻结 matrix 中
`report_id` 仍含历史名称 `windows-fullfield`；A 将它裁定为跨平台复用的 legacy 逻辑
身份，而不是运行平台声明。为避免仅为改名扰动冻结 matrix SHA 和 registry，本轮不改名；
真实平台以 gate 日志和 `ARTIFACTS.json.runner` 为准。

### 4.2 FLEXPART oracle

| family | raw coverage | hard failures | report-only rows | status |
|---|---:|---:|---:|---|
| ERA5 pressure | 75/75 | 0 | 15 | passed |
| CFSR pressure | 75/75 | 0 | 15 | passed |
| ERA5 hybrid | 75/75 | 0 | 3 | passed |

report-only 项继续保留逐点差异，且不改写总体 hard-gate 结果：

- `M3-ORACLE-PRESSURE-ADAPTER-LEGACY-METER-REMAP`；
- `M3-ORACLE-HYBRID-NATIVE-HEIGHT-ALGORITHM`；
- `M3-ORACLE-HYBRID-ASL-OPERATOR-ORDER`。

这些编号表示双方算法语义不同，不是把超差静默写成通过。若目标是 ERA5 官方
hybrid 位势/高度，Trajecta 的 ECMWF alpha 静力递推更贴近官方定义；若目标是逐值
复现 FLEXPART 内部 legacy meter grid，FLEXPART 路径是兼容性基准。其余算子顺序
差异不能仅凭 FLEXPART 自身作为 oracle 判定谁绝对更准确。

## 5. 百万点与 M4 接口

WSL 长测实际完成 1,024,017 点：

| 指标 | 结果 |
|---|---:|
| full million hot | 383.996 s |
| 整项测试 | 729.66 s |
| process peak working set | 163,139,584 bytes |
| dynamic budget | 469,762,048 bytes |
| execute reader/provider I/O delta | 0 |
| N→2N 时间比 | 2.0267 |
| inverse permutation | checked |
| chunk / order / worker determinism | passed |

生产 `IoCallCounters` 证明 frame load 后 execute 阶段没有 reader/provider 调用；不同
chunk、identity/Fisher-Yates 顺序和 1/4 worker 的结果摘要保持确定。

模拟 M4 消费测试 `prepared_queries_support_the_two_call_m4_rk2_shape` 已随 workspace
测试通过，覆盖起点 Transport 查询、测试侧半步位置/高度计算和中点 Transport 查询。

## 6. A2 清单裁决

- 三套正式资料、三个连续时次、Transport 与 NearSurfaceTransport：通过；
- ASL/AGL/Pa、海面/平原/山地、近地/高空和边界：通过；
- pure-Rust/native、NetCDF/GRIB、Windows/Linux：通过；
- native hyperslab、worker 生命周期和无静默回退：通过；
- execute I/O=0、百万点预算、确定性和近线性：通过；
- GPL/MIT 边界、oracle 身份、Schema、SHA 锁定：通过；
- hard-gate/report-only 分层与版本化 allowlist：通过；
- 机器 JSON 与本 Markdown 摘要：一致；
- 模拟 M4 两次查询接口：通过。

## 7. 完成边界

M3 到此完成，下一阶段可以进入 M4 粒子推进、球面 RK2 和边界策略。以下属于后续
科学增强，不反向阻塞 M3：

- 用解析制造解、分辨率收敛或独立观测进一步比较 Trajecta 与 FLEXPART 的绝对准确性；
- 将完整 GPL oracle/native/million 门禁迁入稳定的夜间 CI；
- 如未来需要平台中性的 backend report 名称，在下一次有实质性 matrix/registry
  变更时一并迁移 legacy `windows-fullfield` identity。
