# M3 A2：B 本轮 A 级复审与 CFSR 裁决

日期：2026-07-18  
结论：**本轮不通过 A2 验收；不宣称 M3 完成；不提交。**

本轮已经形成了有价值的工程骨架，但现有 `passed` 还不能作为认证结论。主要原因
不是 CFSR 多出的 1 ULP，而是比较器尚未真正证明 exact metadata、完整覆盖和
vector 配对。

## 1. CFSR 3 ULP 的 A 级裁决

### 1.1 旧 2 ULP 证据存在缺陷

冻结校准文件引用的旧测量程序只计算了：

- exact mismatch；
- 最大绝对差；
- 最大相对差。

它没有逐点计算 ordered binary64 ULP，却在校准 JSON 中记录了
`maximum_ulps=2`。因此旧阈值的数值证据不充分；这是 A 侧旧校准缺陷，不应归责
为 B 擅自破坏门禁。

### 1.2 独立逐点复核

对冻结的 CFSR 00/06/12 文件、`grib:0.1.0:isobaric` 全场重新逐点扫描，Rust
`grib-reader 0.6.0` 与 ecCodes 2.47.0 的结果为：

| 文件 | max ULP | 超过 2 ULP 的点数 | 首个最坏点 |
|---|---:|---:|---|
| 00 | 3 | 5,170 | level index 3、y=1、x=0 |
| 06 | 3 | 5,157 | level index 3、y=1、x=53 |
| 12 | 3 | 7,427 | level index 3、y=0、x=0 |

三个最坏点均位于 5 hPa 附近的极小 q 值：

```text
exact decimal source value: 9e-10
Rust:    8.99999999999999994e-10  bits=0x3e0eec7bd512b572
ecCodes: 9.00000000000000304e-10  bits=0x3e0eec7bd512b575
distance: 3 ULP
absolute difference: 3.10192729707385378e-25
```

负值点具有对称结果。GRIB packing 为 template 40、binary scale 0、decimal scale
10；Rust 值是十进制 `9e-10` 的最近 binary64 表示，ecCodes 的运算顺序产生 3 ULP
偏移。该差异没有气象学意义，也不是字段错位、层序错位或大尺度数值错误。

### 1.3 书面决定

- 不要求修改 Rust decoder 去模仿 ecCodes 的非最近舍入；
- 不把当前 3 ULP 判为 Trajecta 科学错误；
- 原 `2 ULP` 数值模型应由 A 发布新 registry version 修正为 `3 ULP`；
- **在比较器补齐完整覆盖、metadata 和可复现最坏点 artifact 前，当前 registry
  暂不修改**；B 不得自行改阈值或 rule id。

这属于合同第 9 节允许的“旧阈值数值模型错误”修正，不是根据失败结果任意放宽。

## 2. P0：统一比较器尚不能认证

### 2.1 exact metadata 实际只检查了少数项

registry 要求 backend 比较精确检查：

- valid times；
- grid；
- vertical topology；
- layout；
- mask；
- unit；
- temporal support。

当前数据结构只携带 unit、status 和单点 validity，无法表达或检查其余项目。因此
ERA5 的 `passed` 只证明当前传入数组的数值/掩码/单位相符，不能证明完整 backend
合同。

### 2.2 覆盖是从被测输出反推的

`pairs_from_arrays` 对四个数组取最短长度，长度不一致会被静默截断；随后
`expected_case_ids` 又从已经生成的 pairs 反推。结果是：

- 少一个字段、文件或时次可能不会被发现；
- 数组尾部被截断仍可显示 coverage complete；
- 每个网格点共用同一个 case id，不能定位缺点或重复点；
- backend 脚本只要找到任意一套 report，就可能把子集写成总体 passed。

完整覆盖必须来自冻结 manifest/spec，而不是来自 observed 数据本身。

### 2.3 vector 缺分量时可静默通过

当前 vector rule 在没有 V 数组时，scalar fallback 返回 `true`；V 长度不足、case id
错位或 V 的 unit/status 不一致也没有完整拒绝。这会使未来 FLEXPART 风场 hard gate
出现假通过。

### 2.4 诊断统计没有指向真正失败点

ULP row 的 `worst_case_id/value` 当前跟踪最大绝对差，不是最大 ULP 点；因此报告中
显示的数值只有 2 ULP，而 `maximum_ulps` 是 3。ULP metric 必须记录其自身最坏点、
bits 和线性/层/y/x 索引。

此外，ULP row 的 `exact_mismatch_count` 当前为 0，和真实的数十万非 exact 点不符。

### 2.5 Schema 只碰巧通过

现有三个 report 经 A 外部 `Draft202012Validator` 检查均合法，但生成入口没有执行
Schema 验证。后续结构变化可能写出非法 artifact 却仍返回成功。

## 3. P0：backend/oracle 驱动仍可误报

### 3.1 backend 总入口没有要求三套齐全

缺文件时脚本直接跳过该族；总结只判断“已有 report 是否通过”，没有断言
ERA5 pressure、ERA5 hybrid、CFSR 三套全部本轮新生成。因此子集可以误报总体
`passed`。

### 3.2 query SHA 在 Windows 上错误

query 生成器按 LF 文本计算 hash，再用文本模式写文件；Windows 写盘变为 CRLF。
当前三个 `INDEX.json.sha256` 均不等于磁盘文件 SHA-256。oracle 合同要求冻结的是
实际 query 文件，必须按 bytes 写出并在写后重算/自校验。

### 3.3 failed oracle 模板不符合 v1 Schema

`run_oracle.sh` 在缺 gfortran 时生成的 JSON 使用了错误的 oracle/input 字段、空
files 和 `point_id:null`，不符合 `M3_FLEXPART_ORACLE.schema.json`。旧
`generate_oracle_stub.py` 仍输出 v0 且返回 0，也不能作为 v1 环境探针。

### 3.4 Linux 脚本不是可签字门禁

- native 环境 helper 仍是 Windows 专用，并只识别 `libclang.dll`；
- 只探测 netCDF，不保证 ecCodes；
- oracle 失败被 `|| true` 忽略；
- 百万点默认跳过；
- 缺真实资料会跳过而不是输出整体 incomplete/external_blocked；
- 最终仍可能写 `overall=passed_core_gates`。

该脚本可以保留为 core smoke gate，但不能充当 A2 完整 Linux 认证入口。

## 4. P1：IoCallCounters 只完成了局部链

认可的部分：Arc 原子计数、reader decorator 和 execute 前后 snapshot 断言方向正确。

仍缺：

- `inspect` wrapper 没有接到 lock/metadata inspection 生产链；
- `load_with_io` 只有百万点测试调用，普通生产入口仍传 `None`；
- provider frame load 在成功发布后才计数，失败尝试不计；
- filesystem counter 只代表 format detection 前的一次逻辑记录，不是所有真实 open；
- 百万点长测尚未重新执行，因此新断言尚无长测证据。

所以当前可以说“测试中的预加载 FrameLoader 路径被计数”，不能说“所有生产
reader/provider I/O 已被完整计数”。

## 5. 本轮可接受的阶段成果

- IoCallCounters 的无全局、Arc/atomic 基础形态；
- registry selector 的基本唯一匹配；
- scalar exact/ULP/abs-rel 的基本公式；
- native 全场数值扫描入口；
- 三套结构性 75 点 query 数量和稳定 point id；
- Bash/Python 文件语法可解析；
- B 对未完成项和 CFSR 失败保持了诚实状态。

这些成果可继续返工，不需要推倒重来。

## 6. 下一步

按 `docs/engineering/B_PROMPT_M3_A2_FIXUP5.md` 修复。完成前：

- ERA5 backend report 不得称“认证通过”，只能称“数值数组初步通过”；
- CFSR 保持现 registry 下 failed；
- A 不发布 registry v1.0.1；
- M3/A2 保持 incomplete；
- 不 commit。
