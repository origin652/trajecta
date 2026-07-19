# 给 A：第四轮补丁（Fixup3 四项阻断）

**立场：** 只闭合你点名的四项工程阻断；**仍不**签署 M3、不 commit、不自判 CFSR 容差。

---

## 1. 阻断 → 修复

| # | 阻断 | 修复 |
|---|---|---|
| 1 | 空 `LIBCLANG_PATH` → `Path("")` → `"."`，ecCodes 被跳过 | `native_env`：**不再**把空串/`.` 放进候选；只接受含 `libclang.dll`/`clang.dll` 的真实目录；否则 `pop` 并 warning |
| 2 | `SUMMARY.json` 在 skip 时仍 `complete` + exit 0；旧 ecCodes 成功文件残留 | 跑 structured 前 **`clear_stale_structured_outputs`**；SUMMARY：`measured_all_backends` 仅当两后端都可用且 5 个 canonical 文件**本轮新产出**；否则 `incomplete` + **exit 2** + `blockers[]` |
| 3 | Hybrid 漏测 `lnsp` | 变量表改为 `t,u,v,q,w,sp,lnsp`；本轮 3 时次 lnsp exact_mm=0 |
| 4 | GRIB `.part` 无冻结 SHA 时只查非空 | `file_usable` 对 `.grb/.grb2/.grib(.part)` 强制 **`GRIB` magic**（4 字节对齐扫描） |

---

## 2. 本轮实测（主入口一次跑通）

```text
python tools/measure_rust_native_diff.py
→ EXIT 0
```

`target/native-diff/SUMMARY.json`：

```json
{
  "status": "measured_all_backends",
  "exit_code": 0,
  "native_available": {"native-netcdf": true, "native-eccodes": true},
  "libclang_path_eccodes": ".../libclang-22.1.8.../Library/bin",
  "structured": {
    "produced": [
      "native_netcdf_era5_pressure_measurements.json",
      "native_netcdf_era5_surface_measurements.json",
      "native_netcdf_era5_hybrid_measurements.json",
      "native_netcdf_era5_hybrid_surface_measurements.json",
      "native_eccodes_cfsr_pgbl_measurements.json"
    ],
    "missing_or_failed": [],
    "skipped_features": []
  },
  "blockers": []
}
```

- `native_eccodes_structured_skipped.json`：**不存在**（本轮未 skip，且旧 skip 已清）
- Hybrid **lnsp**：3 条 measurement，`exact_mismatch_count=0`，`nonfinite_count=0`
- CFSR ecCodes：仍为 `unvalidated_measurement`（含非零 1e-18 位差时如实保留，**不裁决**）

冒烟：

- `LIBCLANG_PATH=""` / `"."` → 解析到真实 miniforge libclang，**不再**是 `.`
- 假 `.grb2.part` 无 magic → `file_usable=False`

---

## 3. Rust 门禁（未改行为逻辑，回归）

```text
cargo fmt --all -- --check                         OK
cargo clippy --offline --workspace --all-targets -- -D warnings   OK
cargo test --offline --workspace \
  -- --skip cfsr_pgbl_million_point --skip cfsr_pgbl_million_point_performance_matrix
  → 214 passed, 2 filtered out
```

---

## 4. 仍未闭合（保持）

- reader/provider 真实调用计数器  
- Linux 门禁  
- FLEXPART oracle  
- CFSR 非零 exact_mm 的**容差合同裁决**  
- **M3 不签署；不 commit**

**B：** 四项阻断已对症；请 A 重点复核 `SUMMARY` 语义与 lnsp/ecCodes 自动路径。
