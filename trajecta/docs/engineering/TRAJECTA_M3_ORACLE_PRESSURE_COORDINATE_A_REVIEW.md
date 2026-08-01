# A 复核：Oracle pressure-coordinate 路径裁决

日期：2026-07-20

状态：**本轮 P0/P1 返工未通过终验；不 commit，不运行 WSL full+million，不宣称 M3/A2 完成。**

## 结论

ERA5 pressure 当前的 `status=failed` 可以保留为一份诚实的失败诊断，但不能作为
该资料族的最终 oracle 结论。A 裁决为：**另开并明确标识 pressure-coordinate
adapter 路径**，而不是把 ERA5 pressure 当作 ECMWF hybrid 路径运行，也不是把
ERA5 冒充为 NCEP 资料。

同时，当前 CFSR `complete 75/75` 也不能验收：ERA5 pressure 与 CFSR 共用的
pressure driver 实际调用了 `verttransform_ecmwf`，因此两套定压层插值证据都需
作废重跑。ERA5 hybrid 的 ETA 路径方向正确，但共享 harness 变更后仍需重建复跑。

## 已确认闭合的子项

- CFSR 官方 HPBL 已按 `centre=7,discipline=0,category=3,number=196` 解码并写入
  `blh`；当前样例范围和冻结文件证据可信。
- Richardson 失败不再被静默回退吞掉；`hard_fail` 产物诚实。
- 四个历史 patcher 均已退役为 `exit 2`，不再改写正式源码。
- 三个 frozen query SHA 未变。
- CFSR 四个 mountain cell 角点已对应到冻结 query cell。

这些子项通过不等于整个 pressure oracle 通过。

## P0 阻断

### P0-1：pressure driver 调用了错误的垂直变换

`tools/flexpart_oracle/src/pressure_meter_oracle_driver.f90` 当前：

- 导入 `verttransform_ecmwf`；
- 在 `load_slot` 中调用 `verttransform_ecmwf`。

冻结 FLEXPART 中：

- `verttransform_ecmwf` 明确处理 eta/hybrid 层；
- `verttransform_gfs` 才处理固定 pressure levels，并按每格 surface pressure 跳过
  地下层。

ERA5 pressure 的失败日志已经直接暴露该错误：首个失败格点
`ps=92606 Pa`，算法却从 975 hPa 地下层积分，得到 `h=-409.7 m`，随后
Richardson `ierr=-10`。这不是“ERA5 pressure 天生无法做 oracle”的证据，而是
coordinate path 选错的证据。

影响：

- ERA5 pressure 当前 `failed 0/75` 仅保留作诊断；
- CFSR 当前 `complete 75/75` 的 native anchors 可参考，但所有经过 meter transform
  的记录不得用于验收；
- 当前 CFSR 13 个 MIT hard fail 不进入 ADR，必须在正确 pressure transform 后重测。

### P0-2：必须拆开三种身份，不能再用一个 `metdata_format` 混写

A 前一轮把 `metdata_format` 简化成“资料族身份”不够准确，本轮正式纠正。Oracle
必须分别记录并驱动：

1. `source_family`：ERA5 或 CFSR；
2. `vertical_coordinate`：pressure 或 hybrid/eta；
3. `pbl_height_mode`：由 Richardson 诊断，或使用官方 prescribed BLH/HPBL。

冻结 FLEXPART 的 `metdata_format` 同时混合了资料中心、坐标和 PBL 分支，adapter
不得把它直接当成 source identity 输出。

A 冻结的路径如下：

| 资料族 | source | vertical transform | PBL height |
|---|---|---|---|
| ERA5 hybrid | ERA5/ECMWF | `verttransform_ecmwf` | ECMWF Richardson diagnosed |
| CFSR pressure | CFSR/NCEP | `verttransform_gfs` | official HPBL prescribed |
| ERA5 pressure | ERA5/ECMWF | `verttransform_gfs` | official ERA5 BLH prescribed |

ERA5 pressure 的 pressure/PBL 兼容层必须标为 adapter。它可以使用冻结 FLEXPART
pressure-coordinate 分支的同一运算，但不得在产物中宣称 `source_family=NCEP`，
也不得宣称这是 native ECMWF/ETA reader 路径。

裁决层级：

- `native_anchor` 与 `interpolated_common`：可作为 pressure-adapter variant 的 hard
  gate 候选；
- `surface_layer` 与 `modern_difference`：继续按现合同 report-only；
- adapter variant 在 A 发布新 registry 版本前必须是 `unvalidated`，B 不得自行修改
  registry 或复用旧 variant 冒充通过。

### P0-3：CFSR “center” 诊断仍是角点，不是 cell center

当前 diagnostics 把最近网格点 `(j=24,i=34)` 写成 `mountain_center`，得到
`5509.447 m`。冻结 query 的实际 cell center 位于四角中间；四角地形为：

```text
5509.447, 5167.124, 771.060, 2686.144 m
```

半格点双线性中心约为 `3533.444 m`。surface pressure、BLH 以及
`center_underground` 也必须按同一双线性权重计算，不能用一个角点代替。

### P0-4：finite 校验仍只拒绝 NaN

两个 driver 使用 `all(x == x)`，这只能拒绝 NaN，不能拒绝 `+/-Inf`。应使用
Fortran `ieee_arithmetic::ieee_is_finite`，并覆盖所有实际参与 oracle 的必需场、
坐标和输出。允许真实数值零，但缺变量、shape 不符、NaN 或 Inf 都不得标
`complete`。

## P1 证据缺口

- pressure build 尚未生成并强制校验 `nm_symbols.txt`；需要证明实际 binary 包含
  `verttransform_gfs`、`interpol_wind`、`interpol_partoutput_val`、
  `interpol_pbl`、`oracle_calcpar`，且 pressure 路径不依赖
  `verttransform_ecmwf`。
- pressure runner 的 provenance 字符串仍硬编码 `verttransform_ecmwf`，必须按实际
  binary/path 输出。
- CFSR diagnostics 要同时保留 grid anchor 和 bilinear cell center，字段名不得把
  二者混淆。

## 下一轮验收条件

1. 独立 WSL build 目录重建 pressure 与 ETA binary；不得复用旧对象。
2. ERA5 pressure/CFSR 明确调用 `verttransform_gfs`；ERA5 hybrid 明确调用
   `verttransform_ecmwf`。
3. 三族 raw oracle 均为 75/75，Richardson token 为 0，silent fallback 为 0；若
   pressure-coordinate Richardson 仍失败，保留 failed 并提交完整输入诊断，不得
   回退修绿。
4. ERA5 pressure 产物明确标为 pressure adapter；使用官方 BLH，字段缺失或非有限
   必须失败。
5. 三个 query SHA 精确不变；CFSR cell-center 诊断数值正确。
6. canonical/audit 独立 binary 的 compile identity、nm、ELF `.text` SHA 与 records
   一致性重新出证据。
7. 不改 query、registry、Trajecta 算法或容差；旧 CFSR 比较报告不得复用。
8. P0 真闭合后，A 才运行 WSL `linux_m3_gate.sh` full+million。
