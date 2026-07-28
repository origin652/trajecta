# Trajecta M5 A/B 分工合同

## 1. 目的

本文件冻结 M5 期间 A 与 B 的职责、交接方式和停止边界，避免高风险合同、并发恢复或科学验收被机械执行模型擅自裁决。

M5 只使用 A、B 两个角色：

- A 是主责模型和最终技术裁决者；
- B 是在冻结合同下执行中等及以下工程任务的模型；
- 不设置 C；
- A 不调用子代理；
- B 只能由 A 写成详细 Prompt 后，由用户在外部启动。

## 2. 风险分级

### A 独占的高风险工作

以下工作不得直接交给 B：

- M5 权威计划、完成定义和公开合同；
- manifest、状态机、退出码和身份语义；
- daemon/worker 生命周期、IPC 和重新关联；
- 持久队列的一致性、恢复、取消和并发状态转换；
- Runner 宏步边界取消接口；
- SQLite/provenance 的 cancelled、failed、interrupted 终态语义；
- job series、run ID、attempt、rerun 和清理安全性；
- OOM、断电、daemon/worker 崩溃后的 forensic 裁决；
- 任何可能改变数值结果、气象查询、边界、population 或输出语义的修改；
- FLEXPART 科学可比较性与性能差距裁决；
- schema、版本号、容差、异常分类和最终发布签署。

### A 主导、B 可在冻结后协助的工作

- result quick/full validator；
- doctor deep 检查；
- job event schema 和 cursor 行为；
- project finalize 与 DatasetLock 交互；
- native reader 打包和 clean-extraction 验证；
- 故障注入 harness；
- 性能与科学比较脚本。

A 必须先冻结接口、反例和验收条件，再决定是否把其中的机械部分交给 B。

### 可优先交给 B 的工作

- CLI 参数解析和 help；
- human、JSON、JSONL 渲染；
- config/project 的 get/set/unset 和原子文件写入；
- 项目目录初始化和示例文件；
- data-plan JSON 生成和下载助手适配；
- 已冻结状态机的列表、显示和格式化代码；
- CI、打包脚本、SHA、SBOM 和 build manifest；
- 固定命令的 Windows/WSL 矩阵执行；
- artifact 收集、hash、表格和机械汇总；
- py_compile、fmt、clippy、doc、diff-check 和已冻结测试命令；
- A 已写明算法和负例后的局部工程修复。

## 3. 阶段分工

### M5-A0：合同冻结

A：

- 建立 M4 干净基线；
- 冻结命令面、状态机、退出码和机器合同；
- 冻结 manifest v1 修订；
- 写 validator、反例和 B Prompt；
- 裁决是否允许进入 A1。

B：

- A0 完成前不修改 M5 实现；
- 只可执行 A 指定的只读盘点或机械检查。

### M5-A1：CLI、配置与项目

A：

- 冻结配置优先级、项目状态和 finalize 行为；
- 处理 Case、Profile、DatasetLock 的合同冲突；
- 复验 B 的实现不存在科学规则复制。

B：

- 实现 CLI、输出格式和 help；
- 实现 config/project 渐进编辑；
- 实现项目目录、data-plan、模板快照和基础 doctor；
- 增加表驱动解析、原子写和错误信息测试。

### M5-A2：daemon、队列与恢复

A：

- 设计并实现 daemon、worker、IPC、lease 和持久队列核心；
- 实现资源调度、安全取消、重新关联和 interrupted forensic；
- 实现 Runner 控制接口及所有状态转换；
- 完成并发和崩溃反例。

B：

- 在接口冻结后实现 job list/status/wait/events 的显示和解析；
- 编写固定故障执行脚本与 artifact 采集；
- 不修改调度算法、恢复决策或 Runner 生命周期。

### M5-A2E：A2 代码精简与结构收口

A：

- 在 A2 功能冻结后审计新增代码，拆分过大的 runtime/catalog 模块；
- 删除重复实现、宽泛兜底、死代码和不必要抽象；
- 保证 CLI、状态机、schema、产物和数值行为逐字节/逐合同不变。

B：

- 执行冻结的规模统计、重复项清单和全量回归命令；
- 对比精简前后公开 API、测试结果与真实进程 evidence；
- 不自行重构生命周期核心，也不以新增平行版本规避收口。

### M5-A3：结果、验证与重跑

A：

- 冻结 quick/full verify、取消产物和 run_success 语义；
- 实现或审阅 rerun、prune、forget 的安全核心；
- 裁决所有 digest、身份和 forensic 问题。

B：

- 实现冻结后的 inspect/trajectory 流式接口；
- 实现 run-report 渲染；
- 扩充表驱动 validator 测试和固定故障矩阵；
- 执行 A 指定的跨 attempt 审计。

### M5-A4：产品矩阵与打包

A：

- 冻结正式 cell、硬门、source identity 和停止策略；
- 审查首次失败并决定修复路径；
- 审计最终安装包和 native 证据。

B：

- 实现或维护冻结后的编排脚本；
- 严格执行 Windows/WSL 矩阵；
- 首个 hard fail 后停止，不重试、不改参数、不修绿；
- 生成报告、hash 和 artifact 索引；
- 构建 ZIP、tar、SBOM、许可证和 build manifest。

### M5-A5：FLEXPART 对比与发布

A：

- 冻结可比场景、计时边界和科学指标；
- 归因超过 1.5 倍的性能差距；
- 裁决 comparability matrix；
- 与用户确认例外、版本和发布；
- 最终签署 M5。

B：

- 按冻结命令执行预热和三次正式重复；
- 收集 JSON/CSV、wall time、CPU、RSS 和输出质量；
- 生成固定图表与原始证据；
- 不擅自判断科学等价或接受性能例外。

## 4. B Prompt 必备内容

A 给 B 的每份 Prompt 必须写明：

- 唯一目标和允许修改的文件/模块；
- 明确禁止修改的合同、数值核心、容差、schema 和版本；
- 输入资料、命令、平台和 source identity；
- 必须实现或执行的步骤顺序；
- 首个 hard fail 的停止条件；
- 不允许自动重试、缩小规模、改变参数或删除失败 artifact；
- 必须运行的本地门禁和正式矩阵；
- 报告路径、hash、计数和诚实限制；
- `未 commit / 未 push / 不宣称阶段或 M5 完成`；
- 完成后交 A 复验，不自行裁决。

如果 Prompt 在关键合同处截断、含糊或互相矛盾，B 必须停止并请求 A 补全，不得自行猜测。

## 5. 交接和复验规则

B 每次交付至少包含：

- 实际修改清单；
- 实际执行命令；
- 测试通过、ignored 和失败计数；
- 首个失败的完整 artifact 路径；
- source identity 和关键 SHA；
- 未运行项目；
- external_blocked 与产品失败的明确区分；
- 未解决限制和未完成条件。

A 复验时必须：

- 阅读实际 diff，而不是只看 B 报告；
- 复核状态机、身份、异常分类和退出码；
- 对高风险修改补充独立反例；
- 在必要时亲自运行最小冻结复现；
- 明确给出 passed、partial、failed 或 external_blocked；
- 只有全部阶段退出条件满足后才允许 commit/push 或宣布完成。

## 6. Dry-run 删除边界

M5 的 rerun 清理和 `job prune` 仅提供 dry-run：

- 只计算未来可清理的旧 attempt；
- 输出候选路径、大小、原因、保护条件和关联的新 attempt；
- 不提供执行删除的参数；
- 不调用文件或目录删除；
- `job forget` 也不得删除任何运行产物；
- 测试必须在 dry-run 前后比较文件清单、大小和 digest，证明磁盘零删除。

A 不得在 M5 验收中把真实删除作为完成条件。B 不得自行增加 `--apply`、自动清理、保留期限或后台垃圾回收。真实删除能力只能在未来由用户与 A 重新冻结合同后实现。

## 7. Git 与发布权限

- B 默认不 commit、不 push、不打 tag、不创建 release；
- A 在复验通过后负责整理提交边界；
- M4 基线、M5 各阶段和最终 release 使用可审计的独立提交；
- 开发期版本保持 `0.0.0`，最终验收时一次改为 `0.1.0-alpha.1`；
- 发布、tag 和 GitHub Prerelease 必须获得用户明确确认；
- 不提交运行产生的大型 target、临时数据、失败数据库或本机绝对路径资料。

## 8. 文档职责

M5 内必要 help、示例、许可证和预发布说明可以由 B 在冻结结构下起草，由 A 复核。

M5.1 的完整中英文教程、性能解释、配置指南和 troubleshooting 由 A 主写，并与用户讨论不同受众和章节内容后定稿；不得把这部分完全交给 B 自动生成后直接发布。
