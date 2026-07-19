# 给 B：M3 A2 Fixup5（比较器、覆盖、计数、Oracle/Linux）

先完整阅读：

- `docs/TRAJECTA_M3_A2_B_ROUND_REVIEW.md`
- `docs/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`
- `testdata/M3_TOLERANCES.v1.json`
- 三个 M3 JSON Schema。

本轮不得修改 registry version、CFSR threshold、rule id、科学公式、FLEXPART commit
或 MIT/GPL 边界。A 已确认旧 2 ULP 校准有缺陷，但新 registry 必须等本轮证据闭合
后由 A 发布。不要把阈值改为 3 后宣称通过。

不 commit，不宣称 M3/A2 完成。

## P0-1：重做比较输入合同，完整执行 exact metadata

不要继续把所有 metadata 压进 `SamplePair`。建立明确的 field/slab 输入结构，至少
包含：

- stable case/slab id；
- valid time；
- grid signature（shape、坐标值/摘要、扫描规范、经度周期语义）；
- vertical topology/signature（kind、层数、层坐标/PV hash）；
- layout；
- mask 及长度；
- unit；
- temporal support；
- status/validity；
- values。

比较器必须逐项解释 registry 的 `exact_metadata`：

- 每个声明项都真实比较；
- 未实现或未知 metadata 名称是配置错误/incomplete，不能忽略；
- metadata/mask/non-finite 先于 numeric metric；
- subject/reference 数组、mask、布局元素数必须完全一致，禁止 `min(len...)`；
- comparison identity 与每个 field context 不一致时直接 incomplete。

registry loader 同时收紧：

- 结构体 `deny_unknown_fields`；
- schema 中 required 的字段不得用 `Option/default` 静默补齐；
- policy/enum 必须拒绝未知字符串；
- 校验 calibration path/SHA；
- 测试中用 Draft 2020-12 真正验证 registry 和生成 report。

## P0-2：覆盖必须来自冻结 spec，不能从 observed 反推

为三套 backend 比较建立显式 expected matrix：

- ERA5 pressure：冻结的 pressure/surface 文件、3 时次和全部字段；
- ERA5 hybrid：冻结的 hybrid/surface 文件、3 时次和全部字段，含 lnsp；
- CFSR：00/06/12、每文件 1 正式时次、7 个冻结字段。

要求：

- expected case/slab ids 在解码前由 spec/manifest 生成；
- 每个 slab 带 expected element count；
- 缺文件、缺字段、缺时次、重复 slab、额外/缺失元素均为 incomplete；
- `run_m3_backend_comparison.py` 必须要求三套 report 全部在本轮新生成；
- 任一族缺失时 SUMMARY=`incomplete`、exit 2；
- 不能以“至少存在一个 passed report”判总体 passed；
- summary 写本轮 run id、开始时间、每个 artifact mtime/SHA 和 required family 列表。

subject/reference identity 不得再使用“第一份源文件 hash / 最后一份源文件 hash”。冻结
组合输入 manifest hash，并记录 Rust/native implementation 与外部库版本。

## P0-3：vector 必须是不可拆的 U/V 对

重构 vector 输入：

- U/V 长度、case id、mask、unit、status、validity 必须逐项对齐；
- 缺 V、额外 V、乱序 V、重复 id、V metadata 不同均不得进入 scalar fallback；
- vector 数据遇到 scalar rule、scalar 数据遇到 vector rule均为配置错误/incomplete；
- 两分量 abs-rel 和有条件 direction gate 按合同逐点执行；
- report-only 仍记录所有失败统计，但不改变 hard-gate 总结。

## P0-4：统计必须对应实际 gate

- `exact_mismatch_count` 对所有 metric 统计有限样本的数值不等点；
- ULP 的 worst case 必须由 max ULP 选择，并输出 index、Rust/native bits 和数值；
- abs-rel 的 worst case 使用相对其门槛的最大超限尺度，或同时提供独立 max abs/max rel
  worst 信息，不能混用；
- vector 分别记录最坏 U、V、direction；
- invalid_count 明确定义并覆盖双方均 invalid 的样本；
- report 必须经过 `M3_COMPARISON_REPORT.schema.json` 验证后才原子写入成功路径。

补齐原 prompt 要求的全部负例：边界相等/越界、1/2/3 ULP、numeric/bitwise signed
zero、NaN、±Inf、mask、unit、所有 exact metadata、零/多规则、report-only、缺覆盖、
长度不等、vector 缺/乱序 V、comparison identity 不一致。

## P0-5：生成可复现 CFSR 校准 artifact

在不改 registry 的前提下，生成独立 raw measurement，至少记录：

- 三个冻结文件 size/SHA；
- grib-reader、ecCodes/C library 版本；
- q/omega 全场 compared/exact mismatch/nonfinite/max abs/max ULP；
- 所有 >2 ULP 点数；
- max ULP 点的 file、valid time、field、level、linear/y/x index、两边 decimal/hex bits；
- GRIB offset、template、bitsPerValue、referenceValue、binary/decimal scale。

必须复现 A 复核的 q=±9e-10、3 ULP、abs≈3.1019272970738538e-25。artifact 走
稳定 bytes、SHA 和自校验。B 仍不得据此修改 registry；交 A 发布 v1.0.1。

## P0-6：Oracle query 和失败 artifact

### query

- 用真实冻结地形按 A 规则选择 sea/plain/mountain；
- exact grid node 和真实 cell center，不得固定 `+0.025°`；
- hybrid 使用实际 active full-level index，不得把 20/50/80 百分位数字当 level id；
- 明确定义 query schema/canonical bytes；
- 用 bytes/LF 写盘，写后按磁盘 bytes 重算 SHA；
- INDEX 必须自检每个 size/SHA，Windows/Linux 相同内容应得到相同 SHA；
- skeleton 与正式 frozen query 使用不同状态/路径，不能覆盖正式 artifact。

### failed oracle

- 删除或改造旧 v0 `generate_oracle_stub.py`；环境探针只能输出 schema-valid v1
  `status=failed` 并返回非零；
- `run_oracle.sh` 的缺工具链 artifact 必须实际通过
  `M3_FLEXPART_ORACLE.schema.json`；
- input files 不可为空，若还无法形成真实 input identity，则只写外层 STATUS/日志，
  不伪造 oracle JSON；
- README 不得声称不存在的 `run_oracle.ps1`/`src` 已实现。

随后继续原任务：实现 GPL 侧 pressure/ETA 两构建、独立 loader、真实 FLEXPART 垂直
处理与 `interpol_wind`/`interpol_partoutput_val`。骨架不是完成。

## P1-1：IoCallCounters 接到完整生产链

- 一个 Arc instrumentation context 贯穿 lock inspector、ReaderFactory/MetReader、
  FrameLoader/provider 和实际 CLI/engine 入口；
- inspect/build_index/decode/provider attempt 在调用前计数，成功和失败都计；
- filesystem open 若不能完整可靠捕获，改名为
  `format_detection_open_attempt`，不要声称总 open 数；
- 普通生产入口可显式启用相同计数，不得只有百万点测试调用 `load_with_io`；
- 单测覆盖 inspect/build/decode/provider 的成功与失败；
- execute 前后 snapshot 必须相等，并实际重跑两条百万点长测。

## P1-2：Linux gate 分清 core 与 A2 full

- native env 必须跨平台：Linux 接受 `libclang.so`/系统 pkg-config，不注入 Windows
  路径；
- 分别探测 netCDF-C 和 ecCodes；
- 真实资料测试设置 `TRAJECTA_REQUIRE_REAL_MET=1`；
- 三套资料、native、百万点、oracle 任一未运行时，A2 full status 必须是
  incomplete/external_blocked，不能 passed；
- oracle non-zero 不得被 `|| true` 吞掉后仍输出总体通过；
- core smoke 可独立为 passed，但输出字段必须明确不是 A2；
- 记录 Rust、compiler、Python、netCDF、HDF5、ecCodes、libclang、FLEXPART build
  版本与每条命令退出码；
- 最终运行所有 JSON Schema 校验与 artifact SHA 自检。

## 交付和门禁

常规门禁继续全部运行。额外必须实际运行：

1. 比较器全部 unit/negative tests；
2. 三套 Windows native backend，当前 CFSR 按旧 registry 仍应 failed；
3. CFSR 独立 3 ULP raw artifact；
4. query SHA 自校验；
5. schema-valid failed oracle 负例；
6. 两条百万点长测；
7. Linux full gate，或诚实 external_blocked 且不得输出 passed。

报告仍按 `implemented / executed / passed / blocked` 四栏。把所有命令、退出码、
artifact path/size/SHA 写清楚。不得用“测试总数增加”替代上述逐项证据。
