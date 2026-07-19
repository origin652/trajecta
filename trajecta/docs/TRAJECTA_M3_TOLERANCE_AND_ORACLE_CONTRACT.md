# Trajecta M3 容差与 FLEXPART oracle 合同

## 1. A 级冻结状态

本合同从 v0 的“只有 Schema、没有规则”升级为可执行的 v1。A 已冻结：

- 容差 Schema：`testdata/M3_TOLERANCES.schema.json`；
- 容差注册表：`testdata/M3_TOLERANCES.v1.json`；
- 校准证据：`testdata/M3_TOLERANCE_CALIBRATION.v1.json`；
- FLEXPART 原始 oracle Schema：`testdata/M3_FLEXPART_ORACLE.schema.json`；
- 统一比较报告 Schema：`testdata/M3_COMPARISON_REPORT.schema.json`。

注册表版本为 `m3-a-tolerance/v1.0.1`，算法版本为
`trajecta/met_query/m3/v0`。当前校准状态为 `measured_partial`：Windows
Rust/native 后端规则已经有三套真实锚点证据；FLEXPART hard gate 在看到任何
真实 oracle 数值前按 FLEXPART binary32 存储和运算预算预注册，因此禁止事后
为了通过而放宽。Linux、格式等价和真实 oracle 结果尚未产生，不能据此宣称
M3 完成。

## 2. 唯一规则匹配

比较器必须用以下完整上下文匹配规则：

- dataset family；
- comparison target；
- comparison variant；
- sample scope；
- field namespace 和 field；
- coordinate；
- vertical region。

`all` 只在 coordinate/vertical region 中表示通配。展开通配后，零条匹配为
`unvalidated`，多条匹配为验收配置错误；比较器不得选择最宽松规则、不得使用
程序内默认阈值、不得自动扩大阈值，也不得从本次待验结果实时拟合阈值。

每个 hard-gate 样本都必须先满足规则声明的 exact metadata。mask/validity 必须
完全一致，NaN 和无穷一律拒绝。任何非有限数、单位不一致、状态不一致或 mask
不一致都不能被数值容差覆盖。

## 3. 数值 metric 的精确定义

### 3.1 Exact

- `numeric`：有限数按数值相等比较；规则可声明 `+0` 与 `-0` 相等；
- `bitwise`：比较 IEEE 位模式，用于同二进制确定性门禁。

### 3.2 ULP

将有限 binary64 映射为保持数值顺序的无符号整数，距离为两整数之差的绝对值。
NaN/无穷在 ULP 前已经由 `non_finite_policy=reject` 拒绝。CFSR q 的
Rust/ecCodes 规则冻结为最多 3 ULP；omega 独立冻结为最多 2 ULP。

### 3.3 Absolute-relative

每个有效标量必须满足：

```text
abs(a - b) <= absolute
              + relative * max(abs(a), abs(b), scale_floor)
```

所有样本逐点 hard gate，不允许 percentile、均方误差或平均误差掩盖局部失败。

### 3.4 Vector

U/V 必须同时满足：

1. 两个分量分别满足上面的 absolute-relative 公式；
2. 当两边风速都不低于 `speed_floor` 时，方向夹角不得超过
   `direction_degrees`；
3. 任一分量、风速或方向出现非有限值立即失败。

低于 `speed_floor` 时不比较方向，但仍比较两个分量。

## 4. 当前已认证的 backend 规则

### 4.1 ERA5 NetCDF

ERA5 pressure 和 ERA5 hybrid 的 Rust NetCDF/native netCDF-C 在冻结文件、全部
三个时次和全部源字段上数值完全一致，因此 v1 使用 numeric exact hard gate。
Hybrid 规则明确包含二维时变源场 `lnsp`，不能用 convenience `sp` 替代。

### 4.2 CFSR GRIB

CFSR 00/06/12 三文件实测结果：

| 字段 | 决策 |
|---|---|
| T、U、V、surface pressure、surface height | numeric exact |
| specific humidity | 最多 3 binary64 ULP |
| pressure vertical velocity | 最多 2 binary64 ULP |

q 在三个时次均测得最大 3 ULP；最坏点位于 5 hPa、GRIB2 template 40、
decimal scale 10 的同类 JPEG2000 消息，最坏绝对差约 `3.1e-25`。omega 仍为最大
2 ULP。两者因此使用独立规则；q 超过 3 ULP 或 omega 超过 2 ULP 时，都必须先
调查 decoder、打包参数、库版本和字段身份，不得自动更新 registry。

## 5. FLEXPART 分层裁决

### 5.1 共同语义 hard gate

FLEXPART commit 冻结为：

```text
dace3affa2ba71677f12f3858b04aaf59f8ee51e
```

该版本的主要气象数组和插值输出使用默认 Fortran `REAL`（binary32）。在看到
真实 oracle 数值前，A 按 binary32 存储、有限次插值/重构运算和单位转换预注册
以下 hard gate：

| 样本 | 字段 | 冻结门槛摘要 |
|---|---|---|
| native anchor | U/V | 分量 `2e-5 + 5e-7*scale`，非静风方向 `0.02°` |
| native anchor | T | `2e-4 + 5e-7*scale` K |
| native anchor | q | `2e-8 + 2e-6*scale` kg/kg |
| native anchor | p | `0.1 + 2e-6*scale` Pa |
| native anchor | geopotential height | `0.02 + 1e-6*scale` m |
| interpolated common | U/V | 分量 `0.02 + 5e-4*scale`，非静风方向 `0.2°` |
| interpolated common | T | `0.02 + 2e-5*scale` K |
| interpolated common | q | `2e-6 + 2e-4*scale` kg/kg |
| interpolated common | p | `2 + 2e-5*scale` Pa |
| interpolated common | geopotential height | `0.5 + 1e-5*scale` m |

表格是摘要，机器裁决只读取 registry。native anchor 指 exact frame、exact grid
node 和 exact native full level；它主要隔离读取、单位、层序和存储精度。
interpolated common 指 off-node/off-level/intermediate-time 的共同字段，覆盖实际
FLEXPART 插值路径。

`trajecta-vs-flexpart-v11.1` 只认证 `default_real_bits=32`。禁止使用
`-fdefault-real-8` 等精度提升选项后仍套用本 registry；不同默认精度必须使用新的
comparison variant，并在 A 注册前标为 `unvalidated`。

### 5.2 现代语义 report only

以下 Trajecta 结果不得以 FLEXPART 数值作为 hard gate：

- 完整运动学 geometric W；
- geopotential 到 geometric height 的现代转换结果；
- 从最终 p/T/q 重算的 moist-air density；
- `MoninObukhovBusingerDyer/v0` 近地层及其三维层桥接。

它们必须继续通过独立解析公式、物理不变量和真实资料边界 hard gate，同时生成
相对 FLEXPART 的版本化差异报告。`report_only` 不等于通过；超出诊断尺度不会
自动使 M3 失败，但必须进入 A2 报告，异常量级仍须修复或形成带编号的显式
allowlist。

共同的 `geopotential_height` 单独 hard-gate；最终
`geometric_terrain_height` 只 report-only，避免把两种高度语义混为一谈。

## 6. Oracle 查询矩阵

每套正式资料必须使用冻结 query JSON，并把其 SHA-256 写入 oracle。最小矩阵：

### 6.1 Native anchors

- 三个正式时次全部覆盖；
- sea/plain/mountain 三个确定性选择的 exact grid node；
- 三个 full level：上层、中层、近地层但不取模型边界；
- pressure 资料优先使用 250/500/850 hPa；hybrid 使用从顶向下约 20%、50%、80%
  分位的 active full level；
- 每套至少 `3 × 3 × 3 = 27` 条 native-anchor record。

位置选择规则不得由 B 临时挑选“容易通过”的点：sea 为地形绝对值不超过 10 m
的最低稳定线性索引；plain 为 50--300 m 的最低稳定线性索引；mountain 为域内
最高有效地形点，若不足 500 m 则 query 生成必须失败并交 A 裁决。

### 6.2 Interpolated common

- 两个相邻正式帧区间的中点时刻；
- 上述三类位置所在 cell 的几何中心；
- ASL、AGL、Pa 三种坐标；
- AGL 使用 1000 m，Pa 使用 50000 Pa，ASL 使用 5000 m；
- 地下或顶界状态也必须比较，不能只保留 `ok` 点；
- 每套至少 `2 × 3 × 3 = 18` 条 record。

### 6.3 Surface/modern difference

- 两个中点时刻、三类位置；
- AGL 10 m 和 50 m；
- 相同物理点另外生成 modern-difference 记录，用于 W/density/geometric terrain；
- 每套至少 12 条 surface-layer 和 18 条 modern-difference record。

每套资料最少 75 条记录，三套合计最少 225 条。增加点可以，但不得删除最小
矩阵或只保留成功点。query 文件、记录顺序和 point ID 必须稳定。

## 7. GPL harness 边界

FLEXPART oracle 必须由独立 GPL 工具生成。MIT crates 只能读取冻结 JSON，不得
链接、复制、内嵌或在发布构建中编译 FLEXPART/GPL 源码。

Harness 不需要生成传统 `COMMAND/AVAILABLE/RELEASES` 运行目录，也不需要执行完整
粒子模拟。允许在 GPL 侧实现最小独立 loader：用 ecCodes 或 netCDF-C 直接读取同一
冻结官方资料、按审计过的变量/层序/单位映射填充 FLEXPART 气象数组，再调用
FLEXPART 的真实垂直变换和点插值例程。该 loader 不得调用 Trajecta reader，也不得
读取 Trajecta 查询输出；reader/backend 正确性由独立 backend/format 门禁负责。

Harness 必须：

- 锁定上面的 40 位 FLEXPART commit；
- 对 pressure/CFSR 使用非 `ETA` 的 `pressure_meter` 构建，对 ERA5 hybrid 使用
  `ETA` 构建；
- 记录编译器、版本、flags、preprocessor definitions、`default_real_bits`、binary、
  loader 和 harness SHA-256；
- 原 FLEXPART reader 能直接、无歧义读取资料时可使用 `flexpart_native_reader`；
  否则使用 `oracle_adapter_eccodes` 或 `oracle_adapter_netcdf`；
- loader 完成后必须调用 FLEXPART 的垂直处理、`interpol_wind` 和
  `interpol_partoutput_val` 实际路径，不允许自行重写一个“看起来像 FLEXPART”的
  插值器；
- 记录 harness version/SHA、host、输入文件 size/SHA、manifest/Profile/query SHA；
- 按 `M3_FLEXPART_ORACLE.schema.json` 输出原始结果；
- 不在原始 oracle 内做容差裁决。

真实运行失败必须写 `status=partial|failed` 和结构化 failure。不得写零值、空成功
或把 stub 输出冒充 complete。`status=complete` 必须至少有一条记录且 failures
为空。

## 8. 比较报告与最终状态

MIT 侧比较器读取 subject、reference/oracle 和 registry，输出
`M3_COMPARISON_REPORT.schema.json`：

- `passed`：覆盖完整，所有 hard gate 通过，没有 unmatched/duplicate rule、
  oracle failure 或 blocker；
- `failed`：覆盖完整，但至少一条 hard gate 的 metadata/mask/non-finite/numeric
  判定失败；
- `incomplete`：缺文件、缺时次、缺 query、oracle failure、平台未运行或覆盖不足；
- `unvalidated`：已有数值，但 registry 零匹配/多匹配，或 comparison variant 未经
  A 注册。

report-only 结果使用 `reported`，既不能把总体失败变通过，也不能单独把总体变为
失败。最终 `passed` 必须至少包含一个 hard-gate result。

## 9. 变更控制

以下任一变化必须产生新 registry version，并重新进行 A 级评审：

- 科学算法、常量或运算顺序；
- 正式数据锚点、Profile 或 reader 科学语义；
- backend/library 主版本导致差分包络改变；
- FLEXPART commit、编译器精度选项、preprocessor 构建或 harness；
- query 集合；
- metric、阈值、selector、decision 或 allowlist。

任何新测量超过 hard gate 时，默认结论是失败并调查，不是扩大阈值。只有证明旧
阈值的数值模型错误、且有独立证据和 A 级书面裁决时，才允许升级 registry。
