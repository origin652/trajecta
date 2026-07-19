# Trajecta M3 A/B/C 模型任务目标

## 1. 目的与等级

本文规定 M3 期间 A、B、C 三类模型允许承担的工作、必须交付的结果和验收边界。等级表示完成任务所需的模型能力，不表示代码量或人工工期。

| 等级 | 能力定义 | M3 分派原则 |
|---|---|---|
| A | 只能由 SOTA 模型可靠完成 | 冻结数学和公共合同，完成高风险数值核心、生命周期、缓存确定性和科学终审 |
| B | 次一级高性能模型可以完成 | 在 A 已冻结的合同下实现资料接入、执行编排、CLI、真实测试和后端差分 |
| C | 一般性价比模型可以完成 | 在稳定接口下完成机械类型、序列化、文档、报告渲染和常规测试 |

基本规则：

- A 先写清公式、类型、失败语义和验收夹具，B/C 才能开始实现；
- B/C 不得自行修改科学公式、字段含义、容差、状态优先级和许可边界；
- 发现合同矛盾时必须停止相关实现并提交给 A 裁决，不得选择“最容易通过测试”的解释；
- A 负责最终合并前的科学审计，不能只看门禁数量；
- 每个模型必须明确报告未运行的 feature、外部资料和平台测试。

## 2. A 模型目标

A 模型对 M3 的总体目标是：冻结并实现能够决定科学正确性、运行时确定性和未来 M4 接口的核心，不把关键判断下放给 B/C。

### 2.1 A 必须冻结的合同

| 子系统 | 主要类型或模块 | A 的交付目标 |
|---|---|---|
| Canonical fields | `field`、`Capability` | 冻结新增字段、单位、shape、stagger、quality、插值类别和 `NearSurfaceTransport` |
| Query API | `QueryPlan`、`TransportPlan`、`QueryOutput`、`TransportOutput` | 冻结单时刻/单垂直坐标批次、有效位、quality、provenance、bounds 和错误边界 |
| Grid | `RegularLatLonGrid`、球面 basis、vector policy | 冻结日期线、极点、周期经度、cell/triangle 和三维切向风算法 |
| Vertical | `HybridColumnBuilder`、`PressureColumnBuilder`、`VerticalBounds` | 冻结四角柱、hybrid 压力、静力高度、掩膜、单调性和部分层规则 |
| Derivation | pressure、height、thermo、vertical velocity | 冻结常量、公式、运算顺序、W 链式变换和 density 重算语义 |
| Surface layer | `MoninObukhovBusingerDyer` | 冻结输入质量、Businger-Dyer 公式、有效区间、平滑衔接和贴地拒绝 |
| Time/window | `WindowManager`、`PreparedWindow` | 冻结严格包围、整帧三帧斜率、端点覆盖内缩和方向独立性 |
| Cache/memory | Frame/Column stencil、Pin、ChunkPlan | 冻结预算、生命周期、并发、bitwise 确定性和无执行期 I/O |
| Oracle/tolerance | GPL harness contract、tolerance schema | 冻结 MIT/GPL 边界、分层判定、异常审计和禁止自动放宽政策 |

### 2.2 A 必须亲自完成或逐行科学验收

- `RegularLatLonGrid` 的全球定位和球面切向矢量插值；
- hybrid 半层/全层压力、ECMWF hydrostatic/alpha 位势递推和几何高度换算；
- 等压三角有效区与局地柱单调性；
- ASL/AGL/Pa 垂直定位和压力指数插值；
- omega/eta-dot 到几何 W 的完整链式变换；
- terrain 梯度和地面无穿透 W；
- Monin-Obukhov/Businger-Dyer、u*/L 的物理推导、近地有效边界和三维层衔接；
- PreparedWindow、PreparedBatch、缓存 Pin、自动分块和内存冻结的所有权模型；
- 同平台 bitwise 确定性的运算顺序和并行策略；
- GPL oracle 的科学含义、容差分类和最终差异审计；
- M3 最终完成裁决。

### 2.3 A 的必交付物

- 冻结后的公开 API 和合同测试；
- 解析场参考实现或独立参考公式；
- 核心数值实现及不变量测试；
- 算法版本 ID、常量表和 tolerance schema；
- A 级 code review 清单；
- 最终科学验收结论，逐项说明通过、失败或仍未运行。

### 2.4 A 禁止的偷懒方式

- 用 FLEXPART 输出反向拟合现代公式；
- 用 `-omega/(rho*g)` 代替完整几何 W；
- 把位势高度命名为几何 ASL；
- 对地下、模式顶、缺层或贴地查询进行静默 clamp；
- 依赖 NaN、99999 或其它哨兵；
- 只在单线程、小批次或人工常量场上宣称缓存和执行器完成；
- 因真实差分失败而自动放宽容差。

## 3. B 模型目标

B 模型对 M3 的总体目标是：在 A 已冻结的接口和数学合同下，把真实资料、运行编排、CLI、后端差分和大规模测试完整接通，不重新设计科学核心。

### 3.1 B 可以独立实现的工作

| 子系统 | 主要文件或类型 | B 的交付目标 |
|---|---|---|
| ERA5/CFSR 获取 | `tools/fetch_*`、manifest | 可续传下载、固定请求、size/SHA、原子完成、许可和 attribution |
| Profile 扩展 | dataset profiles | 按 A 冻结的 canonical 字段精确映射三套资料及负例，不发明数据集含义 |
| Reader 接入 | GRIB/NetCDF reader、FrameLoader | 接通新增变量、时间语义、mask、source precision 和 provenance |
| 多文件装配 | inventory/assembly | 对 pressure/surface/near-surface 成员做严格网格、时间和角色校验 |
| Query layout | grouping、permutation、workspace | 在 A 冻结的数据结构下实现稳定分组、原顺序恢复和复用缓冲 |
| CLI | `met probe`、`met replay` | JSONL streaming、summary、bounds、explain 和稳定诊断 |
| 后端差分 | Rust/native tests | 对所有时次和字段比较 metadata、mask、layout、quality、provenance 和数值 |
| 真实测试 | integration tests | 三套锚点完整正向链、8 层截顶、NOAA R1 拒绝和重复查询生命周期 |
| 性能工具 | benchmark、metrics | 百万点驱动、I/O 计数、缓存指标、内存记账和结果哈希 |
| CI 编排 | Windows/Linux jobs | 默认、真实资料、native、nightly 和终审门禁 |

### 3.2 B 需要 A 先提供的输入

- canonical 字段和 capability 列表；
- Profile 中每个来源变量的物理含义和单位；
- QueryPlan、TransportOutput、VerticalBounds 和状态合同；
- W、surface layer、height 和 density 的冻结接口；
- 缓存 key、预算、Pin 和 chunk 不变量；
- 比较字段、容差文件格式和 oracle JSON schema；
- 真实资料的固定日期、时次、区域和变量清单。

### 3.3 B 的必交付物

- 可从空缓存复现的三套正式资料获取流程；
- 更新后的 manifest、Profile 和哈希自校验；
- 三套资料 `fetch -> lock -> frame -> query -> CLI` 集成测试；
- GRIB/NetCDF3/NetCDF4 与 Rust/native 差分矩阵；
- 负例和错误码测试，而不只是成功路径；
- 100 万点测试驱动、内存/I/O/确定性指标；
- Windows/Linux 实际运行记录；
- 一份诚实的交付报告，区分“已实现”“已运行”“因环境未运行”。

### 3.4 B 不得自行决定

- full-level pressure、hydrostatic height、gravity、Earth radius 和 virtual temperature 公式；
- omega/eta-dot 到 W 的公式和运算顺序；
- Businger-Dyer 稳定度函数、surface layer 有效范围和估算政策；
- 哪些字段属于 Source、Derived 或 Estimated；
- SampleStatus、VerticalBounds 和结构错误的含义；
- 缺字段、缺帧、地下和顶界是否回退；
- 容差值、allowlist 和 FLEXPART 差异是否可接受；
- MIT/GPL 边界。

### 3.5 B 禁止的偷懒方式

- native feature 静默走 pure-Rust 路径；
- 只测第一个时次、第一个字段或第一个网格点；
- 把派生 NetCDF 冒充官方科学锚点；
- 为使多文件装配成功而忽略网格、时间、层数或 role 不一致；
- 整变量读取后伪装成 hyperslab；
- 用固定高度、最近层、零 W 或 NaN 填补未实现科学逻辑；
- 仅报告测试数量，不报告大型资料、native 和平台门禁是否真正运行。

## 4. C 模型目标

C 模型对 M3 的总体目标是：在 A/B 已冻结并实现的接口上完成机械、可核验、低判断风险的工作，不接触科学裁决。

### 4.1 C 可以承担的工作

| 子系统 | C 的交付目标 |
|---|---|
| 简单类型 | 机械实现枚举显示、serde、错误文本、row view 和只读访问器 |
| JSONL | 按冻结 schema 实现序列化/反序列化和 golden tests |
| Explain 渲染 | 把已有结构稳定渲染为 JSON 或文本，不重新计算权重 |
| 文档 | CLI 示例、字段表、状态说明、环境变量和运行手册 |
| Manifest 检查 | 路径、size、SHA、必填键和重复项的常规验证 |
| 合成夹具 | 按 A 给出的公式参数生成固定小数组，不自行选择科学场景 |
| 常规测试 | 长度不一致、空批、排序恢复、序列化 round-trip、错误消息 |
| 报告渲染 | 由机器 JSON 生成 Markdown 表格和摘要，不判断差异是否可接受 |

### 4.2 C 的必交付物

- 所有新增公开类型的文档和示例；
- JSONL、explain 和报告的稳定 golden files；
- CLI `--help`、错误文本和退出码测试；
- manifest 和文档链接检查；
- 不含科学判断的常规覆盖补齐；
- 明确声明依赖的 A/B 接口版本。

### 4.3 C 不得承担

- 任何气象公式、插值、球面几何、垂直柱或 surface layer 实现；
- 缓存生命周期、Pin、并发和内存预算算法；
- Profile 物理变量识别和单位裁决；
- tolerance、oracle、allowlist 和科学验收；
- 修改 A 冻结的公共接口以方便序列化或测试。

## 5. 推荐实施顺序

### 阶段 A0：A 冻结合同

A 完成 canonical fields、capability、公共 API、状态、VerticalBounds、算法常量、公式接口和验收夹具。此阶段结束前，B/C 不得实现会锁定科学语义的代码。

### 阶段 B0/C0：低风险并行准备

- B：资料下载清单、manifest 框架、现有资料 inventory、CI 环境探测；
- C：文档骨架、JSON schema 骨架、CLI help 和机械测试骨架。

不得提前填入未冻结的字段和容差。

### 阶段 A1：数值核心

A 完成 grid、vertical、time、W、surface layer、查询计划和缓存所有权。先通过解析场和小夹具，再交给 B 接真实资料。

当前记录（2026-07-16）：A0/A1 已完成到可交接状态，并补齐共享 SI 复合维度、canonical registry、CFSR 热通量符号、FRICV/stress 替代依赖和 opt-in 完整 Explain。B1 Phase 1 的真实 CFSR 00/06 与 CLI 已接通；A2 仍依赖三时次/三锚点、后端差分、平台、性能和 oracle 结果，不能提前签署。

当前记录（2026-07-18）：A 已进一步冻结 tolerance/oracle v1。Windows backend 规则已有真实校准证据；FLEXPART common-semantics 门槛在观察 oracle 数值前按 binary32 误差预算预注册，现代 W/height/density/surface-layer 只作版本化差异报告。B 下一步只能按 `B_PROMPT_M3_A2_ORACLE_COUNTERS_LINUX.md` 实现计数器、统一比较器、GPL harness 和 Linux 工程链，不得修改 A 冻结阈值。

### 阶段 B1：真实资料与编排

B 完成三套资料、Profile、reader 接入、CLI、真实差分、百万点测试驱动和 Windows/Linux CI。发现科学差异时提交最小复现给 A，不自行改公式。

### 阶段 C1：文档与报告

C 在稳定输出之上完成文档、golden files、错误信息、报告渲染和使用示例。

### 阶段 A2：最终审计

A 必须重新检查：

- 真实资料是否与描述一致；
- 三套锚点是否无 Estimated 通过；
- 解析场是否独立于生产实现；
- W、height、surface layer 和 density 是否满足冻结公式；
- 执行期是否真正无 I/O；
- 百万点是否受预算约束且结果确定；
- native 是否真实运行而非回退；
- oracle 差异和容差是否有科学依据；
- Windows/Linux 门禁是否实际完成。

只有 A2 给出通过结论，M3 才能标记完成。

## 6. 各等级完成定义

### A 完成

- 所有 A 级公式、接口和不变量有独立测试；
- 没有待 B/C 自行裁决的科学空白；
- 对真实资料和 oracle 差异给出书面结论；
- 公共合同与 M4 模拟消费链通过；
- A2 最终审计通过。

### B 完成

- 所有 A 已冻结的真实接入和运行编排均已实现；
- 三套资料、格式、后端、平台和百万点门禁有实际记录；
- 没有静默回退、伪官方资料、假 hyperslab 或只测首样本；
- 未运行项被明确列出，不能写成“代码已就位所以通过”。

### C 完成

- 机械类型、序列化、文档、golden tests 和报告渲染与冻结 schema 一致；
- 没有修改科学算法或高层接口；
- 示例能够由普通开发者按文档复现；
- 所有链接、命令和错误文本与实际程序一致。
