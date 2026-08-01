# 给 B 模型的实现 Prompt

你负责实现 `flexpart-to-case --target trajecta` 和机器可读迁移报告。

开始前必须完整阅读：

1. `trajecta/docs/engineering/FLEXPART_TO_TRAJECTA_MIGRATION_CONTRACT.md`
2. `trajecta/crates/trajecta-case/API.md`
3. `trajecta/docs/engineering/TRAJECTA_V0_PLAN.md` 的 3.5、M1 和许可边界
4. `tools/flexctl/crates/flexpart-to-case` 当前实现和测试

合同优先级：迁移合同 > 当前 Trajecta v0 类型 > 旧 flexctl 输出兼容性。不要自行改变映射
语义；遇到合同未定义的 legacy 字段，加入报告并标为 `unmapped`，不要猜测。

## 任务

- 在现有 GPL `flexpart-to-case` 中增加 `--target flexctl|trajecta`，默认仍为 `flexctl`。
- 保持 legacy parser 只有一份；在解析后的中立结果上增加 `TrajectaTargetRenderer` 和
  `MigrationReportBuilder`。
- `--target trajecta` 生成一个 Trajecta v0 Case 和一个 JSON migration report。
- 可从 GPL 工具单向依赖 MIT `trajecta-case`，优先构造其公开类型并调用其解析、shape、expand
  或 intent API，避免手写一个会漂移的平行 schema。
- 不得让 MIT workspace 依赖 GPL crate。

## 必须遵守

- 不生成传统 FLEXPART 文件。
- 不实现运行器、气象查询或粒子算法。
- 不下载、扫描或哈希气象载荷。
- 不把 `legacy`、`raw_value`、原始 namelist、raw files 或机器路径写进 Trajecta Case。
- 不用占位 schedule、integrator、output target 或 particle source 伪装成完整配置。
- 当前无法无损映射的 RELEASES、domain-fill、IPIN、numerics 和 outputs 必须按合同省略并写入
  报告。
- `--copy-raw-files` 与 target trajecta 必须冲突报错。
- 输出必须确定性，不写当前时间、随机 ID 或机器相关临时路径。
- 不修改现有 `--target flexctl` 输出和快照。

## 实现顺序

1. 为 CLI 增加 target 和 migration-report 参数及冲突校验。
2. 抽出或确认现有 parser 的目标中立结果，避免从已渲染 flexctl JSON 反向解析。
3. 实现合同规定的 metadata、time、meteorology、substances 和 physics 映射。
4. 实现 disposition、稳定 reason code、source identity、external reference、summary 和 intent
   结果。
5. 使用临时文件完成 Case/报告的安全写入与 Case SHA-256 回填。
6. 增加单元测试、转换快照和真实 default_options 验收。
7. 更新 flexpart-to-case 使用文档，但不要改写 Trajecta 的科学合同。

## 明确禁止的偷懒方式

- 把整个旧 Case 放进 `metadata.labels`、`properties` 或某个 JSON 字符串；
- 用 `legacy-releases`、`flexpart-integrator` 等占位符让 Simulation 看似完整；
- 因字段难映射就静默丢弃；
- 只写人类 Markdown 报告而没有稳定 JSON 报告；
- 测试只判断文件存在，不验证 Case 可被 `trajecta-case` 解析、报告 disposition 和确定性；
- 复制 FLEXPART Fortran 或 GPL validator 代码到 MIT crate。

## 验收命令

至少运行并报告：

~~~text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked --offline --workspace --all-targets
~~~

另外运行 Trajecta workspace 的 fmt、clippy、test 和 rustdoc 门禁，证明单向依赖没有破坏 MIT
workspace。

## 交付说明

完成后列出：

- 修改文件；
- 新 CLI 示例；
- complete/partial 报告示例；
- 各 disposition 数量；
- 尚未映射的字段清单；
- 测试结果；
- GPL/MIT 依赖方向检查结果。

不要声称生成的 Case 可以 Simulation，除非报告中的实际 Trajecta Simulation intent 校验通过，
且没有合同列出的外部前置条件。
