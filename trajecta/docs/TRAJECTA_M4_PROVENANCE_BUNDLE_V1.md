# Trajecta M4 provenance bundle v1

状态：A 冻结合同；供 B 接线、C 独立验证。它不代表 M4-A1 或 M4 已完成。

机器 schema：`testdata/M4_PROVENANCE_BUNDLE.schema.json`
规范示例：`testdata/M4_PROVENANCE_BUNDLE.example.json`

## 1. 为什么不修改 SQLite v1

`M4_SQLITE_SCHEMA.v1.sql` 的 `particle_state.provenance_id` 只有一个整数，无法无损表达同一行中 eastward wind、northward wind、vertical velocity、pressure、temperature 五个字段各自不同的来源链。取最小 ID、首个 ID 或合并成虚构记录都会丢信息。

因此 v1 采用正式、版本化的 `provenance-bundle.json`：

- 不修改 SQLite `user_version=1`；
- bundle 用 `(particle_id, sample_sequence)` 与 SQLite 主键对应；
- 每个样本引用一个五字段 `field_set`；
- `field_set` 再引用完整 `ProvenanceRecord`；
- records 与 field sets 均内容寻址并去重。

旧 `provenance-table.json` 是非原子、缺 transforms、没有样本到五字段映射的工程 dump，不是本合同的一部分，不能作为验收证据。

## 2. 运行目录与 manifest

终态运行目录必须包含：

```text
run-manifest.json
resolved-case.json
resolved-run-profile.json
particles.sqlite
provenance-bundle.json
```

`running` manifest 不得声称已有 provenance bundle。`complete` 或 `completed_with_particle_errors` manifest 必须包含：

```json
{
  "provenance": {
    "schema_version": "trajecta.provenance-bundle/v1",
    "relative_path": "provenance-bundle.json",
    "sha256": "<exact artifact bytes>",
    "sqlite_sha256": "<final particles.sqlite bytes>",
    "content_sha256": "<trajecta.provenance-content/v1>",
    "sqlite_sql_sha256": "<canonical ordered SQL digest>",
    "canonical_output_sha256": "<trajecta.canonical-output/v1>",
    "record_count": 0,
    "field_set_count": 0,
    "sample_count": 0
  }
}
```

写入顺序固定：

1. 完成并 checkpoint SQLite，关闭 writer；
2. 计算最终 `particles.sqlite` 精确文件 SHA-256 与 canonical ordered SQL digest；
3. 生成 bundle 到同目录临时文件，flush/sync 后原子替换；
4. 计算 bundle 精确文件字节 SHA-256 与 normalized content digest；
5. 写终态 manifest，纳入路径、exact SHA、content/SQL/canonical-output digests 和三个 count；
6. exact SHA 仅服务单 run 审计；跨 run 的 1/4 worker、chunk、排列差分必须比较 content 与 canonical-output digests，不能只 hash SQLite 的单个 `provenance_id`，也不能直接比较包含随机 run UUID 的 bundle SHA。

任一步失败均为 run-level `output.*`/`manifest.*` fatal；不得留下 `complete` manifest 指向缺失或旧 bundle。

## 3. Record 的无损内容

每个 record 必须保存 `trajecta_met::provenance::ProvenanceRecord` 的全部语义：

- `field`：canonical snake_case 字段，或 `{namespace,name}` 扩展字段；禁止 Rust `Debug` 文本；
- `quality`：`source | derived | estimated`；
- `sources`：冻结、可解析的 locked source identities，顺序保留；
- `transforms`：完整、按执行顺序排列的 transform chain；
- transform `parameters`：完整规范化 name/value，不得遗漏；
- `fallback_reason`：存在时原样保存；
- `profile_sha256`：精确 lowercase SHA-256。

不得为了去重而跨字段合并 record；`field` 是 record 内容和哈希的一部分。

## 4. 内容寻址

`record_hash_algorithm` 固定为 `sha256-rfc8785`。

- record SHA = SHA-256(RFC 8785 canonical JSON of the `record` object)；
- field-set SHA = SHA-256(RFC 8785 canonical JSON of the `fields` object)；
- UTF-8；对象键按 RFC 8785 排序；数组保持科学顺序；无无意义空白；
- v1 record/field-set 哈希输入不含非有限数值；所有 transform 参数以字符串保存；
- 哈希使用 lowercase 64 hex。

同一 SHA 若对应不同 canonical bytes，必须 hard fail；不得覆盖、保留首条或按插入顺序裁决。

输出数组排序固定：

- `records` 按 `sha256` 升序；
- `field_sets` 按 `sha256` 升序；
- `samples` 按 `(particle_id, sample_sequence)` 升序；
- transform chain 与 sources 保留原科学顺序；
- 每个 transform 的 parameters 按 `(name,value)` 升序，重复参数拒绝。

同一冻结 `run_id` 与同一 SQLite identity 下，worker 数、chunk 划分、查询返回排列不得改变 artifact bytes。独立 production run 会生成不同 UUID，SQLite 也保存 run UUID，因此 exact bundle/SQLite bytes 与 SHA 按定义可以不同；跨 run 确定性门禁必须比较 normalized content，而不是错误要求随机 UUID run 的 exact artifact SHA 相等。

normalized provenance content digest 固定为下列 UTF-8 行流的 SHA-256；records、field sets、samples 均使用本节已冻结的排序：

```text
trajecta.provenance-content/v1\n
record_hash_algorithm=sha256-rfc8785\n
record=<record_sha256>\n
field_set=<field_set_sha256>\n
sample=<particle_id>,<sample_sequence>,<field_set_sha256>\n
```

`record=` 行对每条 record 重复，`field_set=` 行对每个 field set 重复，`sample=` 行对每个 sample 重复。record/field-set SHA 已提交各自完整 canonical body，因此该行流无需重复展开 body。它明确排除 `run_id`、SQLite raw-file SHA 和 bundle raw-file SHA。

规范输出摘要固定为：

```text
SHA256(
  "trajecta.canonical-output/v1\n" +
  "sqlite_sql_sha256=<canonical ordered SQL digest>\n" +
  "provenance_content_sha256=<normalized provenance content digest>\n"
)
```

manifest 中的 exact SHA 用于单次运行审计；normalized digest 用于 1/4 worker、chunk 和排列差分，两者不得混为一谈。

## 5. 五字段映射

每个 field set 必须精确包含以下五个键：

```text
eastward_wind
northward_wind
geometric_vertical_velocity
air_pressure
air_temperature
```

值是 record SHA 或 JSON `null`。非 null 引用必须存在，且 record 的 `field` 必须和 slot 完全相同。

SQLite 每个 `particle_state` 行必须恰有一个 sample assignment；bundle 不得有 SQLite 中不存在的行，也不得遗漏 SQLite 行。对应规则：

- SQL 数值非 NULL：slot 必须为非 null record SHA；
- SQL 数值 NULL 但正式 QueryOutput 仍携带该字段 provenance：slot 必须保留非 null record SHA；validity/status 继续表达数值无效原因；
- 只有该输出行没有该字段或没有可解析 ProvenanceRecord 时，slot 才为 null；
- field validity/quality 必须和引用 record、查询结果一致；
- 一个 `(particle_id, sample_sequence)` 只能出现一次；
- assignment 引用的 field set 必须存在。

SQLite v1 的单个 `provenance_id` 不再是验收来源。B 接线时可以为兼容读取保留该列，但 canonical digest、解释与审计必须只信正式 bundle；不得继续把“最小 provenance ID”描述成五字段 provenance。

## 6. 原子性与资源上限

实现必须流式/有界构建 samples；不得为了 10 万粒子把所有逐行完整 record 展开在内存中。允许：

- 内存中保存去重 record/field-set 字典；
- sample assignments 先写有界 run，再按主键外排归并；
- 最终单 writer 顺序生成 JSON。

临时文件必须位于目标目录，成功校验 schema、counts、引用和 SHA 后才能 atomic replace。失败/取消时清理本轮临时文件，不得复用无 stamp/无 SHA 的旧 bundle。

## 7. B 接线门禁

B 至少补齐：

1. 正式 Rust 类型和 deterministic RFC 8785 hash；
2. 五字段逐项提取，不再用 `format!("{FieldKey:?}")` 分类；
3. 完整 transforms/parameters/fallback/profile/source 序列化；
4. sample↔SQLite 全覆盖交叉校验；
5. 原子 bundle writer；
6. terminal manifest identity；
7. 实现 normalized provenance content digest，并按冻结公式纳入 canonical output digest；
8. collision、漏字段、错字段引用、重复 sample、残留旧 artifact 等负例；
9. 固定 run identity 的 fixture 要求 artifact SHA 相同；独立 UUID runs 则要求 normalized provenance/canonical output digest 相同；
10. 真实 CFSR E2E 中五字段非空行均可解析到完整 record。

C 独立验证时必须重新计算 record、field-set、SQLite、bundle 四类 SHA，并从 SQLite 主键重建全覆盖集合；只做 JSON Schema 校验不够。
