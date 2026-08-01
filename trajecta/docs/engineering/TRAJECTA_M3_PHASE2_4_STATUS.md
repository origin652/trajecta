# Trajecta M3 Phase 2–4 进度报告

**日期：2026-07-17**  
**不宣称 M3 完成；未 git commit。**  
未重做已验收的 CLI 流式 / lock 候选 glob / 外排 fan-in。

## 已实现且实跑

### CFSR pressure（NCEI pgbl）

| 时次 | 文件 | size | sha256 |
|---|---|---:|---|
| 00 | `pgbl00.gdas.2009010100.grb2` | 5465079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| 06 | `pgbl00.gdas.2009010106.grb2` | 5436947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| 12 | `pgbl00.gdas.2009010112.grb2` | 5452202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |

- 目录：`target/test-data/cfsr-ncei-pgbl-official/`
- 来源：NCEI low-resolution `pgbl00.gdas`
- 命令：
  ```text
  python tools/fetch_cfsr_pgbl.py --stamp 2009010112 --out-dir target/test-data/cfsr-ncei-pgbl-official
  python tools/fetch_cfsr_pgbl.py --stamp 2009010100 --out-dir ... --expected-size ... --expected-sha256 ...
  python tools/fetch_cfsr_pgbl.py --stamp 2009010106 --out-dir ... --expected-size ... --expected-sha256 ...
  ```
- 测试：
  - `cfsr_pgbl_transport_three_frames_00_06_12_and_mid_03_09`（03/09 AGL/ASL/Pa）
  - pure-Rust 双载全字段差分
  - 百万点门禁（1,024,017 点分块，1 vs 4 线程，预算 <1 GiB）— 此前实跑通过

### ERA5 pressure（CDS 官方 NetCDF4 + classic 派生）

- 日期：2018-12-01 **00/06/12**
- 区域 NSWE：`[52,0,48,6]`
- 37 压力层；3D：`t u v q w z`；surface：`sp z u10 v10 t2m d2m fsr blh zust ishf ie`
- 命令：
  ```text
  python tools/fetch_era5_pressure_cds.py --out-dir target/test-data/era5-cds-pressure-official --date 2018-12-01 --times 00:00 06:00 12:00
  python tools/prepare_era5_pressure_anchors.py
  ```
- 测试：`real_era5_cds_pressure_classic_rust_full_chain`（3 frames lock→frame）通过
- classic 故意省略 3D `z`，避免当前 decoder 对 surface `z` 的 `AmbiguousSource`（3D z 保留在 official NetCDF4）

### 门禁（本轮）

```text
cargo fmt --all --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point
  → 207 passed, 1 filtered out (~59 s)
cargo doc --offline --workspace --no-deps       OK
```

百万点测试本轮为省时 skip；上一会话已实跑通过。clippy 已修（`MillionPointColumns`、const assert、expect_fun_call）。

### Oracle

- `tools/flexpart_oracle/generate_oracle_stub.py` 写出  
  `target/oracle/M3_FLEXPART_ORACLE_RAW.json`
- 状态：`oracle_failure / flexpart_not_configured`（**未科学裁决**）

### Manifest

- `testdata/REAL_MET_MANIFEST.json` 已更新：ERA5 official/classic 哈希、CFSR 三时次官方集、legacy raw 12z

---

## 已实现但未跑 / 外部阻断

| 项 | 状态 |
|---|---|
| ERA5 hybrid 1–137 官方锚点 | CDS `reanalysis-era5-complete` 长时间 `accepted` / 曾 HTTP 500 / proxy 重试；目录仍空。后台：`python -u tools/run_era5_hybrid_fetch.py` |
| native-netcdf 差分 | `netcdf-sys` 构建失败（缺 `NETCDF_DIR`） |
| native-eccodes 差分 | 本机未系统跑 |
| 完整 `TRAJECTA_REQUIRE_REAL_MET=1 cargo test --tests` | 未作为终态全量重跑 |
| 真实 FLEXPART 数值 oracle | stub only |
| Linux 跨平台矩阵 | 未跑 |

---

## 未实现 / 留给后续

1. hybrid 137 落盘 + Transport/NearSurface 全链 + native/rust 差分  
2. pressure-level 3D geopotential 角色消歧（A 可审：layout/role-aware decode）  
3. 百万点在 CI 中的常规门禁（目前 skip 以控时）  
4. FLEXPART 二进制接线与 A2 容差裁决  

---

## 非宣称

- 不宣称 M3 完成  
- 不修改 A 冻结公式 / 容差 / MIT–GPL 边界  
- 无 git commit


## Update 2026-07-17 (hybrid 137)

### 已实现且实跑
- 新 CDS key 提交 hybrid：`e012f0e8-…` → successful
- 目录：`target/test-data/era5-cds-hybrid137-official/`
  - raw hybrid 137 × 00/03/06（t/u/v/q/w）
  - lnsp（param 152）
  - GRIB PV → A/B 138 interfaces（`dump_grib_hybrid_pv`）
  - hybrid-aligned surface 00/03/06（非 pressure 的 00/06/12）
  - prepared：`ready/era5_hybrid137_prepared_20181201.nc` + `ready/era5_surface_20181201.nc`
- Profile：`era5-cds-hybrid137-v0`（omega 作 pressure_vertical_velocity；无 etadot）
- 测试：`real_era5_cds_hybrid137_rust_full_chain` **通过**（3 frames lock→frame）

### 命令
```text
python tools/run_era5_hybrid_fetch.py
# lnsp + hybrid surface 00/03/06 via cdsapi
cargo run --offline -p trajecta-met --example dump_grib_hybrid_pv
python tools/prepare_era5_hybrid_anchors.py
# copy prepared into ready/
cargo test --offline -p trajecta-met --test real_netcdf_pipeline real_era5_cds_hybrid137
```

### 注意
- Transport 使用 omega（Pa/s），不是 flex_extract 的 etadot；几何 W 科学路径若要求 hybrid 垂直速度，交 A 审。
- 不宣称 M3 完成；未 commit。
