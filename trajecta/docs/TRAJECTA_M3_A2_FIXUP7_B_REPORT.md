# B 交付：M3 A2 Fixup7（扩 ERA5 盒 + formal query 3/3 + 比较器收尾）

对照 A 裁决：扩 ERA5 CDS area `[53,0,45,10]`；mountain≥500 m；H=Φ/g₀；CFSR `0.3.5:surface`/orog；暂不发 v1.0.1。

**未改 registry。未 commit。不宣称 M3/A2 完成。**

---

## 四栏

| 项 | implemented | executed | passed | blocked |
|---|---|---|---|---|
| ERA5 扩盒 fetch+prepare | **yes** | CDS force | pressure+hybrid ready | — |
| Formal query 3/3 + 清空 canonical | **yes** | Windows | **3/3 frozen_terrain_selected** | — |
| CFSR orog 00/06/12 一致 | **yes** | grib_ls/get_data | exact identical | — |
| 比较器 duplicate / hard-fail+blocker / report-only-only | **yes** | unit | 3 新测 + 全 validation | — |
| CFSR worst-q packing 结构化 | **yes** | script | template40 / decScale10 / level5 / offset | — |
| Linux full | 脚本已修 | 未在 Linux 跑 | n/a | external_blocked |
| registry v1.0.1 | — | — | — | **等 A**（B 不改） |

---

## 1. ERA5 扩盒

- `AREA_NSWE = [53, 0, 45, 10]`（pressure / hybrid / hybrid_cds）
- `--force` 时忽略旧 FETCH size/sha，接受新盒字节
- surface H 范围约 **−4.6 … 2417.5 m**（Φ/g₀），mountain ≥ 500 m 成立

## 2. Formal queries（canonical）

- 每次运行 **wipe** `target/m3-oracle/queries/`
- **仅 3/3 成功才**写入 queries + INDEX；否则目录保持空、exit 2
- INDEX `path` 为 repo-relative
- 高度定义：`geopotential_height_m = Phi / 9.80665`
- CFSR：`shortName=orog` ≡ 0.3.5 surface，单位 m；三时次 **array_equal**

| family | mountain_m | status |
|---|---:|---|
| era5_pressure | 2417.50 | frozen_terrain_selected |
| era5_hybrid | 2417.50 | frozen_terrain_selected |
| cfsr_pressure | 5509.45 | frozen_terrain_selected |

## 3. 比较器收尾

- duplicate slab → `config_error` → **incomplete**
- hard-fail **与** 任意 blocker 并存 → **incomplete**（纯 hard-fail 且 blockers 空 → failed）
- report-only-only → 行 `Reported`，overall **incomplete**
- 单测：`report_only_only_is_incomplete_not_passed`、`hard_fail_with_blocker_is_incomplete`、`duplicate_slab_is_config_incomplete`

## 4. CFSR packing 证据

脚本：`tools/measure_cfsr_worst_q_packing.py`  
产物：`target/m3-comparison/cfsr_worst_q_packing.json`

| file | level | template | bitsPerValue | decimalScale | binaryScale | offset |
|---|---:|---:|---:|---:|---:|---:|
| 00 | 5 | 40 | 13 | 10 | 0 | 218705 |
| 06 | 5 | 40 | 13 | 10 | 0 | 222332 |
| 12 | 5 | 40 | 13 | 10 | 0 | 219692 |

`eccodes_version: ecCodes Version 2.47.0`  
与 A 复核点（5 hPa、template 40、decimal scale 10）对齐。

## 5. Backend（扩盒后，旧 registry）

| family | status | compared_total |
|---|---|---:|
| era5_pressure | passed | 945 747 |
| era5_hybrid | passed | 2 829 123 |
| cfsr_pressure | **failed** (3 ULP > 2) | 5 897 232 |

SUMMARY `failed` — 符合「v1.0.1 前 CFSR 仍 failed」。

冻结矩阵已按新 ERA5 SHA 重写：`testdata/M3_BACKEND_EXPECTED_MATRIX.v1.json`。

## 6. 门禁

```text
fmt / clippy -D warnings     OK
workspace tests              OK（含 validation 21 + real_met_manifest 更新后通过）
formal query 3/3 + INDEX     OK
oracle stub schema           OK（query_sha 指向 formal）
backend 3-family             OK（CFSR failed 预期）
REAL_MET_MANIFEST.json       已随扩盒更新 size/sha（非 registry）
registry                     未改
```

---

## 不宣称

- ❌ M3/A2 完成  
- ❌ 擅自发 v1.0.1  
- ❌ git commit  

## 请 A

1. formal query 3/3 + packing 结构化是否满足 **发布 v1.0.1（q/omega → 3 ULP）** 的前置  
2. 扩盒后 ERA5 backend 是否需要重做校准条目/evidence id  
3. CFSR global orog 选点（mountain 5509 m）是否接受为 formal  
