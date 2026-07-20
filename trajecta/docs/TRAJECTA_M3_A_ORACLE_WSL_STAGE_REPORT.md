# M3 A 阶段：真实 FLEXPART Oracle 与 WSL Linux 实跑报告

日期：2026-07-19

> 历史阶段报告：本文记录首次不完整 WSL 实跑，已由 2026-07-20 的
> `TRAJECTA_M3_A2_CLOSURE.md` 取代；不得用本文的旧 `a2_full_status=failed` 代表
> 当前状态。

## 结论

本阶段已经把真实 FLEXPART 数值路径、MIT 侧比较器和 WSL Linux 门禁接通，并获得了
可复核的科学结果：

- Linux core gate 全部通过；
- Rust/native 三套全场比较全部通过；
- FLEXPART ERA5 pressure 已产生 45 条真实记录，另外 30 条明确标为未实现；
- 45 条共同语义记录均能由 Trajecta 正常求值；
- 20 个比较规则组中 8 个通过、12 个 hard gate 失败；
- 失败来自 FLEXPART legacy 插值与 Trajecta 现代插值链的真实差异，未调整算法或容差。

因此，Linux 工程链已经闭合，但 M3/A2 仍未完成。下一阶段应先由 B 补齐三数据族
oracle 覆盖，再由 A 基于完整证据统一裁决插值差异。

## 1. 本阶段实现

### 1.1 GPL FLEXPART harness

新增的 GPL 驱动固定使用 FLEXPART commit：

```text
dace3affa2ba71677f12f3858b04aaf59f8ee51e
```

真实调用并链接：

- `verttransform_ecmwf`；
- `interpol_wind`；
- `interpol_partoutput_val`。

MIT crate 不链接 FLEXPART，只读取 GPL harness 产生的 JSON。

当前 ERA5 pressure harness 身份：

| 项 | 值 |
|---|---|
| harness version | `trajecta-flexpart-oracle/0.2.0-pressure-meter-a-stage` |
| build mode | `pressure_meter` |
| default REAL | 32 bit |
| compiler | GNU Fortran 13.3.0 |
| harness SHA-256 | `f885bd6d4f1952babb42243df975ef2a2f1f489f9d03772f744b963bf5cca6e9` |
| loader SHA-256 | `9c221589b1c4bffabe6793491c1aacb95056fd5189c3c3550ce5bc5b4b756658` |
| binary SHA-256 | `a66a6df21befacb7d080dc7702b30a7c2924772911713966a48189d301d8d796` |

`nm` 实测存在以下定义符号：

```text
__interpol_mod_MOD_interpol_partoutput_val
__interpol_mod_MOD_interpol_wind
__verttransform_mod_MOD_verttransform_ecmwf
```

### 1.2 MIT 侧 oracle 比较

新增：

- `crates/trajecta-met/examples/adjudicate_flexpart_oracle.rs`；
- `tools/run_m3_oracle_comparison.py`。

runner 会：

- 调用 Rust example；
- 运行前删除旧 subject/report，拒绝 stale artifact；
- 校验 FLEXPART oracle 和 comparison report 的 Draft 2020-12 Schema；
- 校验 query、Profile、manifest 和业务资料 SHA；
- 分别返回 `passed`、`hard_gate_failed`、`external_blocked`、`partial`；
- 不修改 tolerance registry。

### 1.3 Linux gate

`tools/linux_m3_gate.sh` 已把以下步骤拆开记录：

- `oracle_raw`：GPL FLEXPART 原始结果；
- `oracle_compare`：MIT Trajecta 对比；
- backend comparison；
- core gate；
- schema 汇总。

另外修复了：

- 声明的 Rust 1.85 MSRV 下 6 处 let-chain；
- ERA5 CLI 测试错误选择 Windows `.exe` 的跨平台问题；
- native 环境工具在 Linux 上误按 Windows `libclang.dll` 查找的问题。

## 2. WSL 实跑环境

```text
Ubuntu-24.04 / WSL2 / x86_64
Linux 6.18.33.2-microsoft-standard-WSL2
Rust 1.94.0
Python 3.12.3
netCDF-C 4.9.2
HDF5 1.10.10
ecCodes 2.34.1
libclang 18
gfortran 13.3.0
```

声明 MSRV 另外使用 Rust 1.85.1 执行：

```text
cargo check --offline --workspace --all-targets
```

结果通过。

主 Linux gate 使用：

```text
env -u LD_LIBRARY_PATH \
  RUSTUP_TOOLCHAIN=1.94.0-x86_64-unknown-linux-gnu \
  CARGO_TARGET_DIR=target/wsl-cargo \
  TRAJECTA_RUN_MILLION=0 \
  bash tools/linux_m3_gate.sh
```

## 3. Linux gate 结果

```text
fmt=0
clippy=0
tests=0
doc=0
era5_chain=0
backend_cmp=0
oracle_raw=3
oracle_compare=1
million=not_run_set_TRAJECTA_RUN_MILLION=1
schema_validate=0
```

workspace 共 236 项测试通过。最终状态：

```text
core_status=0
a2_full_status=failed
a2_reasons=oracle_partial oracle_hard_gate_failed million_not_run
```

这里 `a2_full_status=failed` 是已知科学 hard gate 失败，不是 Linux 编译、测试或
native reader 失败。

## 4. Rust/native 全场结果

冻结 registry：`m3-a-tolerance/v1.0.1`，SHA-256：
`9f72c1b5aabead53b2de08a856b24aff27e0bc61f98282ce6b8dd4427a72b080`。

| family | compared | Linux status |
|---|---:|---|
| ERA5 pressure | 945747 | passed |
| ERA5 hybrid | 2829123 | passed |
| CFSR pressure | 5897232 | passed |

合计比较 9672102 个数值对。未放宽 CFSR q 的 3 ULP 或 omega 的 2 ULP 规则。

## 5. FLEXPART ERA5 pressure oracle

冻结 query：

- ERA5 pressure：`dfc6818d5e10ef0201b9fbd72a99dbbcb040f022ceedd6c7438a59c6ebd96b6d`；
- ERA5 hybrid：`ccfdf320626e0e4795028d2534da63bb317cf75f2d4c9b735e34072299edd24b`；
- CFSR pressure：`d3b7d0a50781d8e78cc94723d1df9e007db557172d8c48911359dc2d93a4c809`。

当前 ERA5 pressure 结果：

| scope | ok | not_available |
|---|---:|---:|
| native_anchor | 27 | 0 |
| interpolated_common | 18 | 0 |
| surface_layer | 0 | 12 |
| modern_difference | 0 | 18 |
| 合计 | 45 | 30 |

最新一次 WSL artifact：

| artifact | SHA-256 |
|---|---|
| raw oracle | `d8a2e5270e78cf143c1daa96506786b48b8a3f8e363a8e649f78745ded449f5e` |
| Trajecta subject | `3459c1dc81aab1dc7619833c3a46db013b39068fb66a28ee127bd5434146d941` |
| comparison report | `534f70e605de43e10ad5571e19805db0223b2a6a7d484f15a868192c825e0bee` |
| comparison summary | `1e950b400bb0e6d9f39c25c9e2ba56842c1938f4a8b1bf79f2917bda70279a26` |
| Linux artifact index | `315885627dd7631682f6e50c70f6bbb49ff46d3ae22748bfbb31e2ee2ad40315` |
| clean Linux gate log | `9722af370861dc16f67976d8fde27f0bc85627a1cdd85f3315da73511545b2ac` |

raw oracle 含 `generated_at_utc`，因此整文件 SHA 会随实跑变化；每次 comparator 都会
固定并验证同一次 oracle SHA，而 query/Profile/manifest/资料文件 SHA 保持冻结。

## 6. 科学比较结果

20 个规则组中：

- 8 个通过；
- 12 个 hard gate 失败；
- report 总状态因 45/75 覆盖仍为 `incomplete`；
- runner 额外明确标为 `hard_gate_failed`，避免 partial 掩盖已知失败。

通过项：

- native U/V、T、q、p、geopotential height 全部通过；
- interpolated ASL geopotential height 通过；
- interpolated AGL geopotential height 通过；
- pressure-coordinate air pressure 通过。

失败最大差异：

| 字段 | 最大绝对差 |
|---|---:|
| 风矢量分量 | 0.486529 m/s |
| 温度 | 0.195572 K |
| specific humidity | 1.237576e-4 kg/kg |
| 高度坐标查询压力 | 189.629 Pa |
| pressure-coordinate geopotential height | 26.766 m |

这些是 FLEXPART legacy 插值顺序/坐标处理与 Trajecta 现代算法的真实差异。当前不发布
新 tolerance 版本，也不让 B 为通过测试而改公式、算法或阈值。

## 7. 未完成与下一步

- ERA5 pressure 的 12 条 surface-layer 和 18 条 modern-difference；
- ERA5 hybrid 137 层完整 75 条 oracle；
- CFSR 00/06/12 完整 75 条 oracle；
- MIT comparator 对三族和 report-only scope 的完整 rollup；
- WSL 百万点长测；
- 三族完整后由 A 统一裁决 common-semantics hard failures。

本报告不宣称 M3/A2 完成，不代表当前 12 个 hard failure 已获科学豁免。
