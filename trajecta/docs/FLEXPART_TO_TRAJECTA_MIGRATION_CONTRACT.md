# FLEXPART 到 Trajecta v0 迁移合同

## 1. 状态与权威性

- 合同状态：M1 实现合同。
- 目标工具：GPL `flexpart-to-case`。
- 目标格式：MIT `trajecta-case` schema version `0`。
- 实现难度：B。
- 语义与最终验收：A。

本文定义 `flexpart-to-case --target trajecta` 的行为、字段映射、迁移报告和验收标准。
实现不得自行扩大 Trajecta schema，也不得用占位值伪装成可运行配置。

如本文与旧 flexctl Case schema 冲突，`--target flexctl` 保持原行为；本文只约束
`--target trajecta`。

## 2. 目标与非目标

### 2.1 目标

一次读取传统 FLEXPART `pathnames` 和 option 文件，生成：

1. 一个符合 `trajecta-case` v0 schema 的 JSON 或 YAML Case；
2. 一个固定为 JSON 的机器可读迁移报告；
3. 稳定诊断，明确哪些语义已映射、规范化、默认、延期、无法映射或被丢弃。

生成的 Case 可以是部分 Case。部分 Case 必须真实表达已经确认的科学语义，并通过
`ValidationIntent::Migration`；不得为了通过 `Simulation` 校验而制造虚假字段。

### 2.2 非目标

- 不生成传统 FLEXPART 配置文件；
- 不运行 FLEXPART 或 Trajecta；
- 不下载气象资料；
- 不读取或复制大气象场载荷；
- 不生成 RunProfile 或 DatasetLock；
- 不把 `legacy`、`raw_value`、传统文件全文或任意 JSON 塞进 Trajecta Case；
- 不在 MIT workspace 中链接 GPL legacy parser；
- 不承诺生成的 Case 当前可执行。

## 3. CLI 合同

### 3.1 命令

~~~text
flexpart-to-case \
  --target trajecta \
  --pathnames <path> \
  --out <case.yaml|case.json> \
  [--migration-report <report.json>] \
  [--format yaml|json] \
  [--options-dir <dir>] \
  [--json]
~~~

### 3.2 参数规则

- `--target` 接受 `flexctl`、`trajecta`；为兼容现有调用，省略时默认 `flexctl`。
- `--target flexctl` 的输出、快照和退出码不得改变。
- `--target trajecta` 总是生成迁移报告。
- 未指定 `--migration-report` 时，默认路径为 Case 同目录下的
  `<case-stem>.migration.json`。
- 迁移报告始终为规范 JSON，不跟随 `--format`。
- `--copy-raw-files` 与 `--target trajecta` 冲突，必须作为参数错误拒绝；Trajecta
  没有 raw escape hatch。
- `--options-dir` 保持现有覆盖语义。
- `--json` 继续使用现有命令 envelope；`data` 至少包含 `case`、`report` 和
  `migration_status`。

### 3.3 退出码

- `0`：Case 和报告均成功生成；报告状态可以是 `complete` 或 `partial`。
- `1`：读取、解析、映射、目标 schema 校验或写文件发生致命失败。
- `2`：命令行参数错误。

`partial` 是成功完成迁移审计，不等于可运行。自动化必须读取报告状态，不得只根据退出码
判断 Simulation 是否就绪。

### 3.4 写文件规则

- 先在内存中构造并校验 Case，再生成报告；
- Case 序列化后计算 SHA-256，写入报告；
- 使用临时文件后替换最终文件，致命失败不得留下半写入的最终文件；
- 输出必须确定性：相同输入、工具版本和参数产生逐字节相同的 Case 与报告；
- 报告不得包含当前时间等非确定性字段。

## 4. GPL 与 MIT 边界

依赖方向只允许：

~~~text
GPL flexpart-to-case -> MIT trajecta-case
~~~

禁止：

~~~text
MIT trajecta-* -> GPL flexpart-to-case / flexctl-core / FLEXPART source
~~~

GPL 转换器可以构造并序列化 `trajecta_case::document::CaseDocument`，也可以调用 MIT
crate 的 schema 和 intent 校验。这不会改变 MIT crate 的许可证。MIT 代码不得复制 legacy
parser 的实现、注释或 FLEXPART 模块组织。

Case 和迁移报告是工具生成的数据，不形成 MIT crate 对 GPL 代码的链接依赖。报告必须记录
转换工具名称和版本，便于审计。

## 5. 映射处理类别

每个被识别的 legacy 语义必须产生一个迁移条目，`disposition` 只能是：

| 值 | 含义 | 是否进入 Case |
|---|---|---|
| `mapped` | 语义和值无变化地映射 | 是 |
| `normalized` | 语义相同，但单位、标识符或表示形式改变 | 是 |
| `defaulted` | legacy 无对应字段，使用本合同规定的目标默认值 | 是 |
| `deferred` | 已有中立 Case 合同，但当前执行器尚不可用 | 是 |
| `unmapped` | 当前目标 schema 无法无损表达或源值有歧义 | 否 |
| `discarded` | 重复 legacy 镜像、raw 值或纯文件边界，没有独立科学语义 | 否 |

只有 `unmapped` 和会阻止当前运行能力的 `deferred` 会使报告成为 `partial`。
丢弃重复 legacy 镜像本身不使报告变为 `partial`。

## 6. 总体映射原则

1. 只映射可以说明物理含义和单位的字段。
2. 不用 `LSYNCTIME`、`CTL` 等近似替代 Trajecta `time_step`。
3. 不用字符串占位符伪造 release schedule、dataset lock、输出空间目标或粒子初始场。
4. Case 只保存可移植科学意图；机器路径只进入迁移报告。
5. 传统数字代码只用于解析，不作为新 Case 的核心身份。
6. 原字段名可以出现在报告的 `source.pointer`，不得泄漏到 Trajecta 公共字段名。
7. 源值非法时不得静默默认；应产生稳定诊断并省略无法构造的目标组件。
8. 如果一个目标结构要求的字段不能完整获得，应省略整个结构，而不是生成半真半假的对象。

## 7. 字段映射

### 7.1 文档头与 metadata

| 来源 | Trajecta 目标 | 处理 |
|---|---|---|
| 固定目标 | `schema_version: 0` | `defaulted` |
| 固定目标 | `kind: case` | `defaulted` |
| 输出文件 stem | `metadata.name` | `normalized`；空 stem 时使用 `migrated-flexpart-case` |
| 固定说明 | `metadata.description` | `defaulted`；说明这是从传统 FLEXPART 配置迁移的部分 Case |
| legacy 作者 | `metadata.authors` | legacy 无可靠来源，保持空 |

源路径、源哈希、转换工具、旧 options 目录不得写入 Case metadata；它们只进入迁移报告。

### 7.2 时间

| legacy 语义 | Trajecta 目标 | 处理 |
|---|---|---|
| `COMMAND IBDATE/IBTIME` | `time.start` | `normalized` 为 UTC Unix 秒和纳秒 |
| `COMMAND IEDATE/IETIME` | `time.end` | `normalized` |
| `LDIRECT=1` | `time.direction=forward` | `mapped` |
| `LDIRECT=-1` | `time.direction=backward` | `mapped` |

如果方向不是 `1/-1`，或者日期无法构造成合法 UTC 时间，必须省略整个 `time` 组件并产生
阻塞 `met_probe`、`simulation` 的 `unmapped` 条目。不得猜测方向或时区。

### 7.3 气象域

传统路径不能进入 Case。路径和 AVAILABLE 内容进入报告；Case 只生成逻辑域：

| 来源 | `DomainSpec` |
|---|---|
| mother | `id=mother`、`dataset=legacy-mother`、`priority=0`、无 parent |
| 第 n 个 nest | `id=nest-NN`、`dataset=legacy-nest-NN`、`priority=n`、`parent=mother` |

其中 `NN` 从 `01` 开始，两位十进制。所有迁移域的
`horizontal_halo_cells` 在 v0 固定为 `1`，记录为 `defaulted`；这是规则经纬网格线性查询的
最低目标合同，不是从 legacy 输入推断的值。

报告必须为每个 dataset 生成 `suggested_dataset_bindings`，但不得虚构 lockfile 路径：

~~~json
{
  "dataset": "legacy-mother",
  "source_role": "meteorology.mother",
  "requires_dataset_lock": true
}
~~~

`input_dir`、`AVAILABLE` 路径、时次文件名和状态只进入报告的外部引用。转换器不打开气象载荷，
不计算载荷哈希；这属于后续 `trajecta data lock`。

### 7.4 粒子来源、RELEASES 与 domain-fill

当前 Trajecta v0 只有 release schedule 标识符，没有 release schedule 文档；domain-fill 又要求
明确的目标粒子质量或数量。因此以下内容都不能无损构造 `particle_population`：

| legacy 状态 | 处理 |
|---|---|
| `IPIN=0, MDOMAINFILL=0` + RELEASES | 识别为 release-driven，但省略整个组件；报告 `release_schedule_contract_missing` |
| `MDOMAINFILL=1` | 识别为 air-mass domain fill，但不得从 RELEASES 质量或粒子数猜测 target；省略组件 |
| `MDOMAINFILL=2` | 识别为 stratospheric ozone domain fill，但 target 和具名 ozone rule 尚未冻结；省略组件 |
| `IPIN=1/2/3/4` | restart、previous partoutput、part_ic 当前没有对应 population 合同；省略组件 |

RELEASES 中的时间、经纬度范围、高度范围、垂直参考、质量、粒子数、注释和物种顺序必须逐项
进入迁移报告，`disposition=unmapped`，并阻塞 `simulation`。

禁止生成诸如 `schedule: legacy-releases` 的占位符，因为它会让一个实际不完整的 Case 看起来
已经具备 release schedule。

### 7.5 substances

只有成功解析且具有非空 `PSPECIES` 的物种才能生成 `SubstanceSpec`。

#### 7.5.1 身份

- `display_name`：原始 `PSPECIES`，去除首尾空白；
- `id`：由 `PSPECIES` 生成小写 kebab-case；
- 只保留 ASCII 字母、数字和单个 `-`；
- 不得使用 `SPECIES_024` 或数字编号作为正常核心 ID；
- ID 冲突时追加 `-` 和该 SPECIES 文件 SHA-256 的前 8 位；
- 如果名称不能产生非空 ID，省略该 substance，并报告阻塞项；
- legacy species number 和文件路径只进入报告。

#### 7.5.2 可映射 properties

| legacy 字段 | property key | unit | 处理 |
|---|---|---|---|
| `PWEIGHTMOLAR` | `molar_mass` | `g/mol` | `normalized` |
| 正的 `PDECAY` | `half_life` | `s` | `mapped` |
| `PDENSITY` | `particle_density` | `kg/m^3` | `mapped` |
| `PDIA` 或 `PDQUER` | `particle_diameter` | `m` | `mapped` |
| `PDSIGMA` | `particle_diameter_sigma` | `1` | `mapped` |
| `PDRYVEL` | `dry_deposition_velocity` | `m/s` | `normalized`；沿用已验证的 cm/s 到 m/s 换算 |
| `PRELDIFF` | `relative_diffusivity` | `1` | `mapped` |
| `PF0` | `reactivity_fraction` | `1` | `mapped` |

所有值必须有限。`PDECAY<=0` 表示不声明 half-life，不产生错误，但报告为已识别的禁用语义。

以下组在当前合同中只进入报告，不进入 properties：

- `PHENRY`：单位和使用模型尚未冻结；
- 气体和气溶胶湿清除参数；
- 化学反应物与 C/D/N 常数；
- emissions 文件、日变化和小时变化；
- 粒子形状代码、方向和轴长。

这些字段不得以 `legacy.*` property key 逃避建模。

缺失被引用的 SPECIES 文件时，转换仍可生成其他部分 Case，但对应 substance 省略，报告
`source_missing` 并阻塞 `simulation`。

### 7.6 numerics

当前 legacy 输入没有可无歧义映射到 Trajecta `NumericsSpec.time_step` 的字段：

- `LSYNCTIME` 是同步间隔，不是积分步长；
- `CTL` 是时间步长控制系数，不是固定时间；
- `IFINE` 是垂直细化设置；
- legacy integrator 和边界策略没有冻结为 Trajecta model ID。

因此 v0 renderer 必须省略整个 `numerics` 组件，并把相关字段记为 `unmapped`，阻塞
`simulation`。不得使用 `LSYNCTIME` 代替 `time_step`，不得生成
`integrator.model=flexpart` 占位值。

### 7.7 physics 开关

以下布尔开关可以生成中立 `PhysicsModuleSpec`。源字段存在时，无论 true/false 都输出，以保留
显式选择。

| legacy 字段 | `model` |
|---|---|
| `LSUBGRID` | `subgrid_topography` |
| `LCONVECTION` | `convection` |
| `LTURBULENCE` | `turbulence` |
| `LTURBULENCE_MESO` | `mesoscale_turbulence` |
| `CBLFLAG` | `convective_boundary_layer` |

处置规则：

- 值为合法布尔且 `enabled=false`：写入 Case，报告记为 `mapped`，**不**阻塞任何 intent；
- 值为合法布尔且 `enabled=true`：写入 Case，报告记为 `deferred`
  （`migration.physics_executor_unavailable`），并阻塞 `simulation`，因为当前没有执行器；
- 值存在但无法解析为布尔：不写入该模块，报告记为 `unmapped`
  （`migration.field_value_invalid`），并阻塞 `simulation`。

`parameters` 在 M1 保持空。下列字段不属于上述模块的稳定参数合同，只进入报告：

- `LAGESPECTRA`；
- `LOGVERTINTERP`；
- `D_TROP`、`D_STRAT`；
- `NXSHIFT`；
- `MAXTHREADGRID`；
- 准拉格朗日开关及其它运行模式耦合字段。

物种沉降、湿清除、衰变、化学和排放模块尚未冻结 property/parameter 合同，M1 不自动生成
对应 physics module；报告必须说明相关源数据已解析但执行语义延期。

### 7.8 outputs、OUTGRID 与 AGECLASSES

当前 `OutputProductSpec` 只有产品、时间表和编码器，没有输出空间目标。传统 OUTGRID、
OUTGRID_NEST、surface-only、per-release、flux、inversion 和 age classes 因此不能完整表达。

M1 必须省略整个 `outputs` 组件，并把以下内容记为 `unmapped`：

- `IOUT` 产品组合；
- `LOUTSTEP`、`LOUTAVER`、`LOUTSAMPLE`；
- NetCDF/legacy 编码选择；
- particle output 模式；
- OUTGRID 经纬网格和垂直层；
- OUTGRID_NEST；
- AGECLASSES；
- receptor output；
- flux、per-release、surface-only、initial-condition、inversion、LCM 输出。

不得只映射时间表而丢掉空间目标后仍宣称产品完整。

### 7.9 机器路径、support data、raw 与 legacy 镜像

以下内容不得进入 Trajecta Case：

- output directory；
- options directory；
- pathnames 原始行；
- `legacy` 对象；
- `raw_value` 和旧日期/时间数字副本；
- `raw_files`；
- support-data 路径；
- PARTOPTIONS、RECEPTORS、INITCONC、REAGENTS、SATELLITES；
- 土地利用、OH、化学场、排放场、restart 和粒子初始文件路径；
- plugin 配置。

纯重复镜像记为 `discarded`。具有尚未建模运行语义的 support data 记为 `unmapped`，并按照
实际启用状态决定是否阻塞 `simulation`。所有路径和已读取小型配置文件哈希进入报告。

## 8. 迁移报告合同

### 8.1 顶层结构

~~~json
{
  "schema_version": 0,
  "kind": "flexpart_to_trajecta_migration_report",
  "tool": {
    "name": "flexpart-to-case",
    "version": "0.1.0",
    "target": "trajecta"
  },
  "target": {
    "case_schema_version": 0,
    "contract": "FLEXPART_TO_TRAJECTA_MIGRATION_CONTRACT"
  },
  "source": {
    "pathnames": {},
    "options_dir": {},
    "documents": [],
    "external_references": []
  },
  "output": {
    "case_path": "case.yaml",
    "case_format": "yaml",
    "case_size_bytes": 0,
    "case_sha256": "",
    "case_intents": {},
    "suggested_dataset_bindings": []
  },
  "summary": {
    "status": "partial",
    "manual_action_required": true,
    "counts": {}
  },
  "items": [],
  "diagnostics": []
}
~~~

所有对象字段使用 `deny_unknown_fields` 等价语义。schema version 0 仍处于开发期，但字段改变
必须同步更新合同、快照和解析测试。

### 8.2 SourceIdentity

每个实际读取的小型配置文档必须记录：

~~~json
{
  "role": "COMMAND",
  "path": "C:/case/options/COMMAND",
  "size_bytes": 1234,
  "sha256": "64-lowercase-hex"
}
~~~

- `path` 是实际读取文件的规范绝对路径；
- `documents` 按 `(role, path)` 排序；
- pathnames、COMMAND、RELEASES、OUTGRID、AGECLASSES、已读取 SPECIES 和 support 文件均需记录；
- 不把 GRIB/NetCDF 气象载荷加入 `documents`。

### 8.3 ExternalReference

大文件、目录或后续 DatasetLock 输入使用：

~~~json
{
  "role": "meteorology.mother",
  "declared_path": "../met",
  "resolved_path": "D:/met",
  "exists": true,
  "content_hashed": false,
  "next_action": "create_dataset_lock"
}
~~~

转换器不得为了填写报告而扫描或哈希整个气象目录。

### 8.4 MappingItem

~~~json
{
  "id": "command.ldirect",
  "source": {
    "document_role": "COMMAND",
    "pointer": "LDIRECT"
  },
  "target_pointer": "time.direction",
  "disposition": "mapped",
  "reason_code": "exact_semantic_match",
  "message": "Forward direction mapped exactly",
  "blocks": [],
  "hint": null
}
~~~

规则：

- `id` 和 `reason_code` 是稳定机器代码；
- `target_pointer` 在未进入 Case 时为 `null`；
- `blocks` 只使用 `met_probe`、`met_replay`、`simulation`；
- `hint` 只给出下一步，不隐藏必要错误；
- 不把大型数组或文件全文复制到 item；值由 Case 和源文件哈希审计；
- items 按 `(source.document_role, source.pointer, target_pointer)` 确定性排序。

### 8.5 Summary

`counts` 必须包含六个 disposition 的数量。`status`：

- `complete`：没有阻塞 intent 的 `unmapped/deferred` 项；
- `partial`：Case 已生成，但有一个或多个 intent 被阻塞；
- `failed`：没有生成有效 Case；通常通过命令错误 envelope 返回，不留下最终 Case。

`manual_action_required` 在 status 为 `partial` 时必须为 true。

### 8.6 Case intent 状态

`output.case_intents` 必须实际调用 Trajecta intent 校验，结构为：

~~~json
{
  "migration": { "ready": true, "diagnostic_codes": [] },
  "met_probe": { "ready": true, "diagnostic_codes": [] },
  "met_replay": { "ready": true, "diagnostic_codes": [] },
  "simulation": {
    "ready": false,
    "diagnostic_codes": ["case.missing_particle_population", "case.missing_numerics"]
  }
}
~~~

Case intent ready 不等于本地数据和执行器 ready。报告还必须指出 RunProfile、DatasetLock 和
延期 physics executor 等外部前置条件。

## 9. 稳定诊断代码

M1 至少定义：

| code | 含义 |
|---|---|
| `migration.release_schedule_contract_missing` | RELEASES 无目标 schedule 文档 |
| `migration.population_target_missing` | domain-fill 无明确 target mass/count |
| `migration.initial_particle_source_unsupported` | IPIN 模式无目标 population 合同 |
| `migration.numerics_not_lossless` | 无法构造 NumericsSpec |
| `migration.output_target_contract_missing` | 输出空间目标未建模 |
| `migration.species_source_missing` | 被引用 SPECIES 文件缺失 |
| `migration.species_id_invalid` | PSPECIES 无法形成稳定 ID |
| `migration.species_property_deferred` | 模型专用 property 合同未冻结 |
| `migration.physics_executor_unavailable` | Case 可声明但当前执行器不存在 |
| `migration.field_value_invalid` | 源字段存在但无法解析为合法有限值 |
| `migration.machine_path_report_only` | 路径移入报告，不进入 Case |
| `migration.legacy_mirror_discarded` | raw/legacy 重复镜像被丢弃 |
| `migration.dataset_lock_required` | 逻辑 dataset 仍需 RunProfile/lock |
| `migration.unmapped_legacy_field` | 未识别/未合同化的 legacy 字段 |
| `migration.output_source_missing` | 期望的输出相关源文件缺失 |
| `migration.release_field_invalid` | RELEASE 日期/时间等字段非法 |

诊断 code 不得包含具体文件名、物种编号或数组索引；具体位置放在 path/source pointer。

## 10. 验收测试

### 10.1 向后兼容

- 不带 `--target` 的现有 flexctl 输出快照逐字节不变；
- `--target flexctl` 与旧默认完全等价；
- 现有参数错误和 JSON envelope 测试继续通过。

### 10.2 Trajecta 目标快速测试

至少覆盖：

1. COMMAND 正向/反向时间精确转换为 Timestamp；
2. mother + 两个 nests 生成稳定 domain/dataset ID、priority 和 parent；
3. Case 中不出现输入目录、AVAILABLE、output_dir、legacy、raw_value；
4. 报告记录所有已读取配置文件的大小和 SHA-256；
5. 报告不扫描或哈希气象载荷；
6. PSPECIES ID 规范化和哈希冲突后缀；
7. 已批准 substance properties 的数值和单位；
8. PDRYVEL 单位换算；
9. 缺失 SPECIES 文件产生 partial 报告且 Case 仍可解析；
10. RELEASES、domain-fill、IPIN、numerics、outputs 按合同省略并报告阻塞；
11. 五个 physics 开关生成稳定 model ID；
12. `--copy-raw-files --target trajecta` 被拒绝；
13. JSON 与 YAML Case 均能被 `trajecta-case` 重新解析；
14. 生成 Case 通过 Migration intent，并记录其它 intent 的实际结果；
15. 相同输入连续转换两次，Case 和报告 SHA-256 完全一致；
16. 报告 items 和 source documents 顺序确定；
17. 输出写入失败不留下半写入最终文件；
18. GPL workspace 可以依赖 MIT `trajecta-case`，MIT workspace 依赖图不出现 GPL crate。

### 10.3 真实案例

使用 FLEXPART 仓库的 `tests/default_options` 和一个含嵌套域的真实 pathnames：

- 生成 Trajecta Case 和 migration report 快照；
- Case 至少可用于 `Migration`；
- 时间和逻辑气象域完整；
- 物种身份及已批准 properties 完整；
- 报告清楚说明为何当前不能 Simulation；
- 不把传统文件内容或绝对路径写入 Case。

## 11. 完成定义

`--target trajecta` 只有同时满足以下条件才算完成：

- 旧 parser 仍只有一份，两个 renderer 共享解析结果；
- 没有修改 Trajecta schema 来迁就 legacy raw 字段；
- Case 和报告均有严格类型或等价 schema；
- 所有映射都有 disposition 和稳定 reason code；
- 所有省略均可在报告中审计；
- partial Case 不伪装为 runnable；
- 快速测试、真实案例快照、fmt、clippy 和文档门禁通过；
- GPL/MIT 单向依赖经检查；
- 使用文档包含完整命令示例和 partial 结果说明。
