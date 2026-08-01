# B 交付：M3 A2 Fixup6（比较器/冻结矩阵/query/校准/Linux）

对照 A 对本轮 Fixup5 的 P0 漏洞清单。  
**未修改 registry。未 commit。不宣称 M3/A2 完成。暂不请求 A 发布 v1.0.1**（formal query 与校准 packing 证据仍有开放项）。

---

## 四栏

| 项 | implemented | executed | passed | blocked |
|---|---|---|---|---|
| 比较器 hard-gate / report_only / blockers / config | **yes** | unit | unit OK | — |
| 冻结 matrix SHA/size/times/fields | **yes** | backend full-field | 三族按 matrix 跑通 | — |
| Schema 禁止弱回退 | **yes** | backend + oracle stub | 缺 jsonschema → exit 2 | — |
| Query 地形单位/cell center/hybrid/INDEX | **yes** | Windows | 单位修正后 **诚实 fail**：mountain 490.78 m < 500 m | formal 0/3；交 A 扩域或改门槛 |
| CFSR packing + ecCodes 2.47.0 | **partial** | yes | version + grib_ls packing 行 | 最坏点级 packing 逐消息对齐可再加细 |
| Linux `run()` set -e | **yes** | 脚本已改 | n/a | Linux 主机未跑 → external_blocked |

---

## 1. 比较器状态语义

- `report_only` 成功/失败均 → `Reported`，**不再** `Pass`
- overall `passed` 仅当存在 **hard_gate Pass** 且 `blockers` 为空
- config/shape（缺 V、长度、规则形状错配）→ `config_error` → overall **`incomplete`**，不计入 hard_fail
- missing/unexpected coverage → incomplete
- blockers 非空不能 passed

---

## 2. 冻结 expected matrix

- `testdata/M3_BACKEND_EXPECTED_MATRIX.v1.json`：每文件 `relative_path/size/sha256/valid_times_unix/fields`
- `run_m3_backend_comparison.py`：校验磁盘 size/SHA；调用  
  `adjudicate_backend_fields --manifest … --manifest-family …`
- example：解码前用冻结 times×fields 建 expected ids；inspect times 必须 **精确等于** 冻结集合（非仅数量）
- subject/reference identity = matrix SHA + 文件摘要组合

本轮结果（旧 registry）：

| family | status | compared |
|---|---|---:|
| era5_pressure | passed | 297075 |
| era5_hybrid | passed | 888675 |
| cfsr_pressure | **failed** (3 ULP > 2) | 5897232 |

SUMMARY `failed` exit 1。

---

## 3. Schema

- backend / oracle stub：**require** `jsonschema` Draft202012；ImportError → incomplete exit 2，无弱检查。

---

## 4. Oracle query

- 解析 `units=m**2 s**-2` → 米 = z/g0  
- 本冻结域最高地形 **490.78 m < 500 m** → **formal 生成失败**（不编造山地）  
- cell center：向邻点半步，不越域  
- hybrid level：1-based from top（~28/69/110）  
- INDEX：`path` 为 **repo-relative** posix  
- 清除 formal 目录 CFSR skeleton 残留  
- stub 仅用 formal query SHA（拒绝 skeleton 冒充）

`generate_queries.py` exit 2：`0/3 formal` — 诚实 incomplete，请 A 裁决域/门槛。

---

## 5. CFSR raw calibration

- `target/m3-comparison/cfsr_ulp_raw_calibration.json`
- `eccodes_version: 2.47.0`
- `grib_packing_q_omega`：`grib_ls` offset / template40 / bitsPerValue / ref / binaryScale / decimalScale / shortName q|w
- 仍复现 3 ULP 与 abs≈3.10e-25  
- **仍不改 registry**

---

## 6. Linux gate

- `run()` **始终 return 0**，rc 写入 `exit_codes.txt`，避免 set -e 在 backend/oracle 非零时提前退出  
- A2 full 与 core 分离字段不变  

本会话未在 Linux 实跑。

---

## 门禁（Windows）

```text
fmt / clippy -D warnings     OK
validation unit              18 passed
workspace tests              OK（million 长测本轮未重跑；Fixup5 已有证据）
backend 3-family             OK（CFSR failed 符合旧 registry）
oracle stub schema           OK exit 1
query formal                 incomplete（mountain < 500 m）
registry                     未改
```

---

## 不宣称

- ❌ M3/A2 完成  
- ❌ 请求立即发 v1.0.1（等 A 对 formal query 域/500 m 规则与校准终态签字）  
- ❌ git commit  
- ❌ 放宽 registry ULP  

## 请 A

1. 冻结域最高 ~491 m：扩 ERA5 盒、或书面调整 mountain 门槛、或指定替代地形源  
2. v1.0.1 是否仍等 formal query 三族齐套后再发  
3. CFSR GRIB 地表高度是否纳入 terrain 选点器范围  
