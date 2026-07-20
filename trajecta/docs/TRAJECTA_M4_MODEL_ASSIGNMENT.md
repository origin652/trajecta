# Trajecta M4：A/B/C 模型职责与交付边界

状态：设计已冻结，实施时按本文分派；本文不代表 M4 已完成。
日期：2026-07-20
权威实施计划：`TRAJECTA_M4_PARTICLE_LOOP_PLAN.md`。

## 1. 分级原则

| 等级 | 能力定位 | M4 分派原则 |
|---|---|---|
| A | SOTA 科学与架构模型 | 冻结并实现会决定科学正确性、公共合同、生命周期和最终验收的核心 |
| B | 高性能工程模型 | 在 A 已冻结合同下完成机械工程、真实资料、平台、性能和输出实现 |
| C | 通用性价比模型 | 在稳定接口下完成 serde、文档、报告、golden 和低判断风险测试 |

基本规则：

- A 先冻结公式、类型、失败语义、算法 ID 和验收夹具；
- B/C 不得自行解释未冻结的科学含义；
- B/C 发现合同矛盾时停止相关分支，提交最小复现给 A；
- B/C 不得选择“最容易让测试通过”的公式、容差或 fallback；
- A 负责最终合并前科学审计，不能只看测试数量；
- 每轮报告必须区分 implemented / executed / passed / blocked；
- 未实际运行的平台、native、真实资料或长测不得写成通过；
- B/C 阶段默认不提交，等待 A 验收；A 审阅通过后再形成阶段 commit。

## 2. A 模型职责

A 对 M4 的总体目标是：完成并签署最小粒子闭环的数学、公共合同和高风险生命周期，不把关键科学判断下放给 B/C。

### 2.1 A 必须冻结的合同

| 子系统 | A 必须冻结的内容 |
|---|---|
| Particle | 状态、身份、origin、年龄/偏移、carrier mass、正常/异常终止分类 |
| Clock | signed dt、事件边界、正反向排序和时间溢出语义 |
| RNG | Philox 版本、key 组成、字符串 ID 摘要和 f64 映射 |
| Release | event、质量等分、连续出生时刻、垂直无效硬失败 |
| Geometry | 球面长度/面积、日期线切分、极区拒绝和确定性采样合同 |
| Integrator | 三维单位球 RK2、W、运算顺序和收敛标准 |
| Boundary | 地面求交反射、模式顶/有限域交点、正常与异常分类 |
| Population | lifecycle 顺序、dynamic births、outflow 和 stable IDs |
| Air mass | 网格面积、pressure interface、dry-air mass、边界通量和残余 |
| Ozone | Ertel PV、2 PVU/3 km/60 ppbv 规则、carrier/ozone mass 语义 |
| Output | exact-time met sampling、typed sink、失败与 finalize 顺序 |
| Manifest | 生命周期状态、身份哈希、seed、算法和统计字段 |
| Tolerance | 守恒容差、解析容差、FLEXPART report-only 分类 |
| Acceptance | Windows/WSL/native/真实资料/性能矩阵和最终完成定义 |

### 2.2 A 必须亲自实现或逐行科学验收

- `SimulationRunner` 主循环和生命周期顺序；
- `StepPlanner` signed event splitting；
- `Rk2Spherical` 三维单位球中点法；
- 地面连续碰撞定位与反射；
- 正常/异常粒子终止状态机；
- `PopulationStrategy` 公共接口和 ReleaseDriven lifecycle；
- `AirMassDeriver`；
- hybrid/pressure 干空气质量；
- boundary mass flux 与 residual accumulator；
- 每步和最终质量守恒；
- `PotentialVorticityDeriver` 的 Ertel PV；
- `OzoneAssignmentRule` 公共接口；
- PV60 臭氧公式；
- 输出时刻气象查询语义；
- RunOutcome 与 manifest 最终状态；
- FLEXPART oracle 的科学分类；
- M4 最终完成裁决。

### 2.3 A 的必交付物

- 权威实施计划和模型分工；
- 冻结后的 public API 与 schema；
- 算法 ID、常量表和 tolerance registry；
- 独立解析参考公式和不变量测试；
- A 亲自完成的高风险数值核心；
- 给 B/C 的阶段 prompt；
- 每轮 B/C 交付审计报告；
- 最终科学验收报告；
- 经验证的阶段 commit。

### 2.4 A 不得偷懒

- 不得复制 FLEXPART 运算顺序后把结果称为现代独立算法；
- 不得用经纬度平面近似替代球面推进或面积；
- 不得把 midpoint 风冒充输出时刻风；
- 不得把地下、模式顶、域外或缺柱静默 clamp；
- 不得因真实差分失败自动放宽容差；
- 不得把正常出流和 numerical failure 混成同一状态；
- 不得只测单粒子、单步、单方向或单线程；
- 不得在 B/C 尚未实跑平台和长测时签署完成。

## 3. B 模型职责

B 对 M4 的总体目标是：在 A 已冻结的科学与接口合同下，把 release、GeoJSON、SQLite、真实资料、native、平台和性能工程链完整接通，不重新设计科学核心。

### 3.1 B 可以独立实现的工作

| 子系统 | B 的交付目标 |
|---|---|
| Case schema | 按 A 冻结类型完成 serde、validation、resolver 和 round-trip |
| ReleaseSchedule | 显式 event 解析、排序、duplicate 检查和时间边界 |
| RNG mechanics | 按冻结 Philox 常量和 key 编码实现，无公式裁决权 |
| GeoJSON | parser、外部文件哈希、日期线切分、洞归属和规范化 |
| Geometry sampling | 按 A 冻结球面长度/面积算法实现批量采样 |
| Stable IDs | event/domain boundary spawn 的确定性 ID 分配 |
| SQLite crate | rusqlite、schema v1、prepared statements、WAL 和事务 |
| Manifest I/O | 原子写、路径、SHA、状态落盘和 schema 自校验 |
| Runner plumbing | registry、builder、typed diagnostics 和资源配置接线 |
| Real data tests | 三套资料的 release/air-mass/ozone 正反向集成测试 |
| Native diff | Rust/native 轨迹和逻辑 SQLite 比较 |
| Performance | 1 万/10 万驱动、RSS、I/O、SQLite size、digest 和 baseline |
| Platform | Windows 与 WSL 脚本、环境探测和 artifact 汇总 |
| Oracle tooling | 独立 GPL harness 编译/运行、query 生成和 JSON 汇总 |

### 3.2 B 需要 A 先提供的输入

- 所有 public type 和字段含义；
- Philox 算法版本、常量和 key 编码；
- 球面长度、面积和日期线规范化规则；
- RK2、boundary 和 population trait；
- dry-air mass、boundary flux 和 PV 公式；
- ozone rule；
- 正常/异常状态分类；
- manifest 与 SQLite schema；
- tolerance 和 oracle report schema；
- 真实资料固定日期、区域、时次、粒子数和方向矩阵。

### 3.3 B 的必交付物

- 可编译的 release/GeoJSON/SQLite 工程实现；
- 所有新增 schema 的正例、负例和 round-trip；
- 外部 GeoJSON path/size/SHA 自校验；
- 日期线自动切分面积守恒和采样不越界证据；
- SQLite live-read、事务原子性、integrity 和 schema v1 测试；
- 三套真实资料正向/反向运行 artifact；
- Rust/native 小规模差分；
- Windows/WSL 实际运行记录；
- WSL 10 万 hybrid 端到端性能 JSON；
- I/O counters、RSS、SQLite size、scaling ratio 和逻辑 digest；
- 一份诚实交付报告，逐项写 implemented / executed / passed / blocked；
- 未提交状态下交 A 审阅。

### 3.4 B 不得自行决定

- 球面 RK2 公式或运算顺序；
- 地面反射求交和安全偏移；
- pressure interface 构造；
- dry-air mass、boundary flux 或 residual 公式；
- Ertel PV；
- 2 PVU、3 km、60 ppbv、48/29 等臭氧规则常量；
- 正常与异常终止分类；
- 守恒/解析/native/FLEXPART 容差；
- 缺字段、缺柱、地下、模式顶是否可 fallback；
- 哪些 FLEXPART 差异可接受；
- MIT/GPL 边界。

### 3.5 B 不得偷懒

- 不得使用全局顺序 RNG；
- 不得让线程或 chunk 改变粒子 ID 和样本；
- 不得把 GeoJSON 经纬度当平面面积；
- 不得要求用户手工切日期线后假装自动支持；
- 不得丢弃无效 release 粒子后继续；
- 不得逐粒子 SQLite commit；
- 不得用 JSON blob 保存核心数值列；
- 不得在 WAL 失败时静默切换日志模式；
- 不得用 midpoint 气象值填充输出；
- 不得让 native feature 静默回退 pure-Rust；
- 不得只跑第一套资料、第一方向或第一后端；
- 不得把未跑的 WSL/10 万测试写成 passed；
- 不得自行 commit 或宣称 M4 完成。

## 4. C 模型职责

C 对 M4 的总体目标是：在 A/B 已冻结并实现的接口上完成机械、可核验、低判断风险的工作，不接触公式、容差和科学裁决。

### 4.1 C 可以承担的工作

| 子系统 | C 的交付目标 |
|---|---|
| Serde | 简单枚举、显示、只读访问器和 round-trip |
| Error text | 稳定错误码、人类可读说明和 hint 文本 |
| JSON Schema | manifest、performance、oracle 和 report schema 的机械实现 |
| SQLite docs | 表结构说明、SQL 查询示例和 schema version 文档 |
| Examples | endpoints/interval、release、domain-fill 和 output_root 示例 |
| Golden tests | manifest、resolved Case、错误文本和规范 SQL 导出 |
| Report rendering | 从机器 JSON 生成 Markdown 表格，不判断通过与否 |
| Link checks | 文档链接、命令、环境变量和 artifact 路径检查 |
| Small fixtures | 按 A 已给参数生成固定小夹具，不选择科学场景 |
| Test helpers | 临时目录、SQLite 查询助手和重复机械断言 |

### 4.2 C 需要 A/B 先提供的输入

- 已冻结的 type/schema/error code；
- 已实现的 SQLite 核心表；
- 已生成的机器 JSON artifact；
- 已确认的命令、环境变量和示例 Case；
- A 对状态、容差和结果用语的最终定义。

### 4.3 C 的必交付物

- 所有新增公开类型的 rustdoc；
- manifest/SQLite/schema 使用文档；
- 可复制的 SQL 轨迹查询示例；
- endpoints 与 interval 示例；
- 正向/反向 age/offset 说明；
- normal vs abnormal termination 表；
- stable golden files；
- 文档链接和命令检查；
- 不含科学判断的测试覆盖报告；
- 明确声明依赖的 A/B 合同版本。

### 4.4 C 不得承担

- RK2、球面几何、边界、干空气质量、PV 或臭氧公式；
- RNG 算法或采样分布选择；
- lifecycle、并发、缓存和内存预算设计；
- SQLite 事务/恢复策略裁决；
- tolerance、oracle、allowlist 和科学验收；
- Profile 变量身份和单位裁决；
- 修改 A 冻结接口以方便文档或 serde；
- 宣称 Windows/WSL/native/性能通过。

## 5. 阶段分派与交接

### M4-A0：A 冻结合同

A：

- 写入实施计划和模型分工；
- 冻结 Case、RunProfile、Particle、Runner、manifest 和 SQLite schema；
- 冻结公式、算法 ID、容差和解析夹具。

B：

- 只做依赖、许可、rusqlite/GeoJSON crate 和平台环境探测；
- 不实现会锁定公式的逻辑。

C：

- 建文档、schema 和测试目录骨架；
- 不填未冻结内容。

退出条件：A0 合同无待 B/C 自行裁决的空白。

### M4-A1：普通 release 闭环

A：

- Particle/Clock/Integrator/Boundary/Runner；
- ReleaseDriven lifecycle；
- exact-time output sampling；
- 解析 hard gate。

B：

- ReleaseSchedule；
- Philox mechanics；
- GeoJSON、日期线自动切分和球面采样；
- SQLite crate 与 manifest I/O；
- 合成集成测试。

C：

- serde/golden/error/doc；
- SQL 示例。

退出条件：合成场普通 release 正反向可生成完整 SQLite interval 轨迹。

### M4-A2：Air-mass domain-fill

A：

- AirMassDeriver；
- 初始化、边界通量、residual 和守恒；
- 正反向科学审计。

B：

- 三套真实资料编排；
- 1 万粒子矩阵；
- 计数器、性能和 artifact。

C：

- 质量账本文档和报告表格；
- 正常 outflow 示例。

退出条件：三套资料 air-mass 1 万正反向在 Windows/WSL 通过，守恒 hard gate 通过。

### M4-A3：Ozone domain-fill

A：

- Ertel PV；
- OzoneAssignmentRule；
- PV60 规则；
- 公开公式和 GPL oracle 裁决。

B：

- 三套资料 ozone 真实链；
- Rust/native 差分；
- GPL harness 和 JSON artifact。

C：

- 规则说明、provenance 和报告渲染。

退出条件：三套资料 1,000 粒子正反向通过，标量 hard gate 与完整差异报告齐全。

### M4-A4：终审

A：

- 重新检查所有公式、状态、输入身份和报告；
- 裁决所有 FLEXPART 差异；
- 确认 abnormal termination=0；
- 签署或拒绝 M4。

B：

- WSL hybrid 10 万正反向；
- Windows/WSL/native 最终矩阵；
- 性能 baseline、RSS、SQLite size、I/O 和 digest；
- 完整交付报告。

C：

- 最终文档、链接、复现命令和 artifact index。

退出条件：A 给出书面完成裁决；此前任何模型不得宣称 M4 完成。

## 6. 报告格式

B/C 每轮报告必须包含：

| 字段 | 含义 |
|---|---|
| implemented | 代码或文档是否已经落地 |
| executed | 对应测试/命令是否实际运行 |
| passed | 冻结验收标准是否通过 |
| blocked | 外部环境或 A 裁决是否阻断 |

另附：

- 精确命令；
- exit code；
- artifact 路径、size、SHA；
- OS、Rust、compiler、SQLite、netCDF、HDF5、ecCodes 和 libclang 版本；
- 未运行项；
- 未提交声明；
- 不宣称 M4 完成声明。

报告不得只写“测试数量”，也不得把代码存在写成 executed/passed。

## 7. Commit 规则

- A 自己完成的合同/数值核心可在 A 门禁通过后形成阶段 commit；
- B/C 默认在未提交状态交付；
- A 必须检查 diff、真实 artifact 和未运行项；
- A 验收通过后再提交对应阶段；
- scientific formula、tolerance、algorithm ID、schema version 和 GPL/MIT 边界的修改必须由 A 明确写入审计；
- 最终 M4 标记只能由 A 在完整终审后提交。

## 8. 各等级完成定义

### A 完成

- 所有公式、公共接口和 hard gate 已冻结并实现；
- 没有待 B/C 自行裁决的科学空白；
- 真实资料、native、平台、性能和 oracle 证据已审阅；
- 所有异常有书面裁决；
- 最终 M4 状态已签署。

### B 完成

- A 冻结后的工程链全部实现；
- 三资料、双方向、Windows/WSL、native 和 10 万长测均有实际记录；
- 没有静默 fallback、伪确定性、平面几何、逐粒子 commit 或假执行；
- 未完成项诚实列出；
- 交付保持未提交等待 A 验收。

### C 完成

- serde、schema、文档、golden、错误文本和报告与冻结合同一致；
- 示例可由普通开发者复现；
- 没有修改科学算法、容差或高层接口；
- 所有链接、SQL 和命令与实际产物一致。
