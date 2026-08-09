---
title: Project、Case、RunProfile 与 DatasetLock 参考
description: 查找 Trajecta 项目关系、文档字段、路径规则、draft 状态、data plan、lock 与 finalize 行为。
---

# Project、Case、RunProfile 与 DatasetLock

Trajecta 将 portable scientific intent、machine-local execution 与 run 使用的准确气象文件分开
保存。Project 通过名称连接这些部分，同时让每份 source document 保持独立可读。

## 文档职责

| 文档 | 格式 | 主要职责 | 创建或读取它的命令 |
| --- | --- | --- | --- |
| Project index | YAML，`trajecta-project.yaml` | 命名 Case 与 Profile，选择 dataset profile，记录 optional default Profile | `project init`、`project get/set`、`project validate`、`project finalize` |
| Case | YAML 或 JSON | 定义 scientific time、meteorology requirement、population、numerics、physics 与 output schedule | `case validate`、`case resolve`、project validation、worker |
| RunProfile | YAML 或 JSON | 选择一个 Case，并提供 local output、dataset binding、reader 与 resource | Project validation、finalization、scheduler、worker |
| Data plan | JSON，`trajecta.data-plan/v1` | 列出确定性 file 与 capability requirement，以及 current local status | `project data-plan` |
| DatasetLock | JSON | 将一个 logical dataset 绑定到 immutable local file、hash、coverage 与 capability | `data lock`、`project finalize`、`doctor --deep`、worker |
| Resolved Case | JSON result artifact | 完整展开 Case component 并记录 source digest | `case resolve`、run admission、worker |
| Resolved RunProfile | JSON result artifact | Canonicalize local path，并记录准确 execution binding | Run admission 与 worker |

## Project index

Project index 使用 schema identity `trajecta.project-index/v1`。

```yaml
schema_version: trajecta.project-index/v1
name: moisture-study
cases:
  wet-season: cases/wet-season.yaml
  dry-season: cases/dry-season.yaml
profiles:
  wet-local:
    path: profiles/wet-local.yaml
    dataset_profiles:
      met: era5-hybrid
  dry-local:
    path: profiles/dry-local.yaml
    dataset_profiles:
      met: era5-hybrid
default_profile: wet-local
```

### 字段

| Field | Constraint | 含义 |
| --- | --- | --- |
| `schema_version` | 准确等于 `trajecta.project-index/v1` | Index format identity |
| `name` | Non-empty string | Human project name |
| `cases` | Mapping，name 与 path 非空且唯一 | Case name 到 project-relative document path |
| `profiles` | Mapping，name 非空且唯一 | Runnable Profile entry |
| `profiles.<name>.path` | Non-empty project-relative path | Entry 选择的 RunProfile document |
| `profiles.<name>.dataset_profiles` | Logical dataset ID 到 named profile 的 mapping | 为每个 Case dataset 选择 preparation 与 lock behavior |
| `profiles.<name>.template` | Optional non-empty name | 准备期间使用的 machine configuration Profile template |
| `profiles.<name>.template_sha256` | 与 `template` 同时出现；64 个 hexadecimal character | Selected template content identity |
| `default_profile` | Optional existing Profile name | Project-aware command 允许省略 selection 时采用的 Profile |

同一 mapping level 中的 key 保持唯一。Typed index conversion 前会拒绝 duplicate YAML mapping
key，其中也包括 `dataset_profiles` 内的重复项。

## Case 与 Profile 的关系

一个 project 可以包含多个 Case 和多个 Profile。每个 Profile entry 选择一份 RunProfile
document，其中的 `case_path` 再选择一个 indexed Case。多个 Profile 可以引用同一个 Case，
从而表达不同 reader、thread、memory 或 output choice。

一个 Profile 不会展开到多个 Case。需要以两种机器配置运行四个 Case 时，可以为需要的组合
建立单独 named Profile entry。每次 run receipt 由此对应一个 resolved Case 和一个 resolved
RunProfile。

Selected Case 使用的 logical dataset identifier 应出现在 Profile entry 的
`dataset_profiles` mapping 中。缺少 mapping 或出现额外 mapping 时，project validation 与
finalize preflight 会返回相应 diagnostic。

## Case document

Case 使用当前 numeric Case schema version 和 `kind: case`。Top-level object 会拒绝 unknown
field。

| Field | 职责 |
| --- | --- |
| `schema_version` | Numeric Case document contract version |
| `kind` | `case` |
| `metadata` | Metadata contract 支持的 name、description、authorship 或 label |
| `time` | Direction、start/end instant、transport step 与 output timing requirement |
| `meteorology` | Domain、logical dataset reference、required field、coverage 与 vertical interpretation |
| `particle_population` | Release、air-mass、ozone 或 domain-fill initialization 与 lifecycle setting |
| `substances` | Tracked substance definition 与 initial mass relationship |
| `numerics` | Integration 与 boundary-control setting |
| `physics` | Selected physical module 及其 parameter |
| `outputs` | Product 与 output-event specification |

Component 可以 inline，也可以引用 local component document。`case resolve` 会展开 reference，
normalize value，并记录每个 source digest。Worker 接收 resolved form，因此 source component
在 admission 之后的编辑不会改变该 attempt 的内容。

Validation intent 改变 minimum complete shape：

```text
trajecta case validate cases/study.yaml --intent simulation
trajecta case validate cases/study.yaml --intent met-probe
trajecta case validate cases/study.yaml --intent migration
```

Simulation intent 要求 numerical run 所需 component。Meteorological probe 可以使用更小的
Case，集中描述 query coverage。Migration intent 用于 document conversion check。

## RunProfile document

RunProfile 使用当前 numeric Case schema version 和 `kind: run_profile`。

| Field | 职责 |
| --- | --- |
| `schema_version` | Numeric RunProfile contract version |
| `kind` | `run_profile` |
| `metadata` | Descriptive Profile metadata |
| `case_path` | 指向一个 selected Case 的 local path |
| `output_root` | 创建 unique run 与 attempt directory 的 root |
| `datasets` | Logical dataset binding，其中包含 lockfile、root、optional cache root 与 reader override |
| `profile_sources` | Meteorological reader 使用的 explicit Profile file 或 non-recursive directory source |
| `execution.worker_threads` | Positive scheduler CPU request |
| `execution.memory_budget_bytes` | 以 byte 表示的 positive scheduler memory request |
| `execution.executor` | Non-empty executor identity |
| `execution.meteorology_reader` | Binding 未 override 时采用的 default `rust` 或 `native` reader |

Resolved RunProfile 会 canonicalize local path，并记录 source digest。Memory budget 向上取整到
MiB 后用于 queue admission。Dataset binding 可以为一个 logical dataset override Profile
reader。

## Project 路径规则

Project index 中的路径遵守 lexical 与 canonical project jail：

- 路径相对于 project root；
- Index 使用 `/` 作为 portable separator；
- Empty component、repeated separator、`.` segment 与 `..` segment 会被拒绝；
- Absolute path 和 normalize 后越过 root 的路径会被拒绝；
- 已存在 filesystem object 的 canonicalization 也不能越过 project root。

`project set` 在写入前校验 resulting complete index。路径被拒绝后，index byte 保持不变，
project 外也不会创建文件。

RunProfile data root 与 output root 是 resolved Profile contract 中的 machine-local path。Public
project format 要求 project-relative path 时，project preparation 会让 generated plan 和 lock
path 保持相对形式。

## Draft、configured 与 finalized

| 状态 | 含义 | 可进行的工作 |
| --- | --- | --- |
| `draft` | 一个或多个 required Case / Profile value 仍缺失 | 继续 incremental `project set`，查看 partial status |
| `configured` | Document 能解析和 resolve，data、lock 或 output preparation 仍未完成 | 生成 data plan，准备文件并运行 validation |
| `finalized` | Required document、lock、capability、coverage 与 output root 已就绪 | 运行 doctor 并提交 selected Profile |

增量设置期间，缺少 required RunProfile value 可以保持 draft。Unknown field、wrong type、
duplicate key 和 semantic Case error 会报告为 document error，不会当成 draft field。

## Data plan

```text
trajecta --project PROJECT project data-plan
trajecta --format json --project PROJECT project data-plan --output data-plan.json
```

Plan 从 selected Case 派生 temporal 与 spatial coverage，再按 dataset 与 capability 列出
requirement。它会报告 current local state 为 missing、partial 或 ready。Requirement、root 与
capability 使用确定性排序，因此相同 project state 会产生相同 plan byte。

Project 可以在资料到达前完成配置。Plan 描述准备内容，不创建 DatasetLock。文件就绪后，
explicit finalization 会读取真实 metadata 与 content。

## DatasetLock 与 finalization

DatasetLock 记录：

- Schema 与 dataset-profile identity；
- Coverage interval 与 spatial capability set；
- 每个 selected file 的 relative path、size 与 SHA-256；
- 解析 locked path 使用的 named local root；
- 校验 binding 所需的 reader 与 metadata。

`data lock` 根据 root、profile 与 Case 直接构建一个 lock：

```text
trajecta data lock --root DATA --profile PROFILE --case CASE --output LOCKFILE
```

`--replace` 允许在完整 build 成功后替换已有 output。未使用该 option 且 lock 已存在时，命令
返回 `data.lock_exists`。

Project finalization 会一起处理全部 selected mapping：

```text
trajecta --project PROJECT project finalize
```

它会校验 index 与 referenced document，检查 Profile-to-Case selection，读取 data root，派生
required coverage 与 capability，构建 candidate lock，并检查 output root。Complete preflight
成功后才替换已有 lockfile。Preflight 失败会返回 nested diagnostic，同时保留 previous lock
byte。

修改 Case、Profile、mapping、template identity 或 locked input file 会使旧 binding 过期。
提交该 Profile 前，可以生成新 plan，准备变化的文件，再次运行 finalize。
