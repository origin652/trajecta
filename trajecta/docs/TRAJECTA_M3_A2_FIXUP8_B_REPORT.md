# B 交付：M3 A2 Fixup8（证据链收尾，仍不发 v1.0.1）

对照 A：科学上批准 CFSR **q → 3 ULP**，omega 保持 2；发布前修 packing 循环 SHA + 扩盒校准输入/统计。

**未改任何 hard-gate 阈值/rule id。** 仅更新  
`testdata/M3_TOLERANCE_CALIBRATION.v1.json` 与 registry 内 **calibration_report.sha256 指针**（rules 字节级未变）。  
**未 commit。不宣称 M3 完成。不自行发布 v1.0.1。**

---

## 1. Packing 单向证据链

`tools/measure_cfsr_worst_q_packing.py`：

- **读取** `cfsr_ulp_raw_calibration.json` 并 pin 其 SHA  
- **只写** `cfsr_worst_q_packing.json`（+ `.sha256`）  
- **不再反写** raw calibration  

校验：

```text
packing.source_calibration.sha256 == sha256(cfsr_ulp_raw_calibration.json)
packing.sha256 == sha256(cfsr_worst_q_packing.json)
regenerate packing does not mutate raw
```

当前 pin（重建 clean raw 后）：

| artifact | sha256 (prefix) |
|---|---|
| raw ULP calib | `2583243f564d4e5b…` |
| worst-q packing | `e1ff20e378ce3556…` |
| frozen calibration json | `c097417f96cde497…` |

packing 仍含 level=5 / template=40 / decimalScale=10 / ecCodes 2.47.0。  
raw 已去掉历史反写字段（`grib_packing_q_omega` / packing artifact 指针）。

---

## 2. 校准文件扩盒同步

`testdata/M3_TOLERANCE_CALIBRATION.v1.json`：

- `frozen_inputs` SHA/size = 扩盒后矩阵（ERA5 pressure/hybrid + CFSR）  
- ERA5 pressure `compared_values`: **945747**（原 297075）  
- ERA5 hybrid `compared_values`: **2829123**（原 888675）  
- CFSR q `maximum_ulps`: **3**；omega **2**  
- 附 backend report SHA + packing/raw 指针  

registry：

- `calibration_report.sha256` → 新校准文件 digest  
- **rules 未改**（CFSR q hard gate 仍为 2 ULP，待 A 发 v1.0.1）

可重复脚本：`tools/update_m3_tolerance_calibration_expanded.py`

---

## 3. 门禁

```text
fmt / clippy -D warnings     OK
validation unit              21 passed（含 calibration SHA load）
workspace tests              OK
registry loader              OK（calibration pointer 一致）
ULP rules                    仍为 q=2（未发 v1.0.1）
```

---

## 请 A

证据包收尾完成后，可发布 **m3-a-tolerance/v1.0.1**：

- **仅** `grib:0.1.0:isobaric`（q）`maximum_ulps: 2 → 3`  
- omega 保持 2  
- 新 registry_version + 更新 calibration 引用  

B 不代发。
