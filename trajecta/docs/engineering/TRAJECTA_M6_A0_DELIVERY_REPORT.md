# Trajecta M6-A0 交付报告

日期：2026-08-10

结论：**M6-A0 passed；科学、配置、资料、验证、过程产物和恢复合同已经冻结。当前停在 A0，不进入 A1。**

M6-A6 的 OH 分支存在一项已登记的外部阻塞。该阻塞不影响 A0 合同闭合，也不限制 A1–A5 的顺序实施；A6 在补齐 OH 资料来源链之前不能签署。

## 1. 本轮范围

本轮只完成 M6-A0：

- 冻结强类型 `physics` 配置、物种和预设；
- 冻结 12 个物理模块的顺序、依赖、参数单位、允许覆盖字段和科学来源；
- 冻结共同子步、Strang 分裂、中点速度合成和随机键；
- 冻结辅助资料选择、获取方式、使用条件、再分发判定和全局锁边界；
- 冻结解析、守恒、伴随、随机统计、外部实验及性能容差；
- 冻结 SQLite v2、过程查询 v1 和检查点 v1；
- 冻结自动恢复原因、人工处理原因、谱系和已完成任务保护；
- 建立独立的 M6-A0 校验器并执行现有 M4/M5 回归门禁。

未实现 Case 类型、物理管线、CLI 命令、数据库写入、检查点或恢复代码。生产核心仍使用 M4 SQLite v1。

## 2. 交付文件

### 2.1 权威合同

| 文件 | SHA-256 |
|---|---|
| `docs/engineering/TRAJECTA_M6_A0_SCIENCE_CONTRACT.md` | `f80cf57a149d22dd6c4af41e98e9cf94515d4d5c616ebd68cfbedc3ead84cabf` |
| `testdata/M6_PHYSICS_CONTRACT.schema.json` | `07f6d3010e746209fdc42db0aad4299de5dce0f6a954d496c3e3c0d12bde2f01` |
| `testdata/M6_PHYSICS_CONTRACT.v1.json` | `5e79df600de6b318b59bb9ae1d6025a45f35e490f877b8a348aaea20d1d6e8c2` |
| `testdata/M6_TOLERANCES.schema.json` | `64cfbf084d2ec1458e7254303ac13861875cc2efbef246bd9a64ac74f37f8976` |
| `testdata/M6_TOLERANCES.v1.json` | `98239ff9b48355229497b975db235b77854ad236db3e1fe8bf236cd40cc417cc` |
| `testdata/M6_VALIDATION_ASSETS.schema.json` | `1207f38729c34979ef3f298740111be22fba32b31b3671763f05936af4844282` |
| `testdata/M6_VALIDATION_ASSETS.v1.json` | `18c0ea9d0dc20939e4fffcee261469c8cb3b848473628f49896a7d403472c26e` |

### 2.2 产物合同

| 文件 | SHA-256 |
|---|---|
| `testdata/M6_CHECKPOINT_MANIFEST.schema.json` | `ff6f040056696f6d39027c4ac7901135de1122e0ea38b2d8e31fad1f215893d6` |
| `testdata/M6_CHECKPOINT_MANIFEST.example.json` | `504e957a2cf5b1bd687619afbed1f13e1ff3822adf273c4848f8735e6a0af88e` |
| `testdata/M6_PROCESS_QUERY.schema.json` | `d24d92619c46dadda26122f2d76cbdbd9eec3a858d4353e4ad2baa3825cce32d` |
| `testdata/M6_PROCESS_QUERY.example.json` | `f558ade3263d7d90ad54de517e7a233dcc34ba6c53b26cb7afbbc694bc6438d7` |
| `testdata/M6_SQLITE_SCHEMA.v2.sql` | `1600d2235a9e03167e08f48a8cccb6db56e24dbec9e62658f7cda209a5782c92` |

### 2.3 校验器

| 文件 | SHA-256 |
|---|---|
| `tools/validate_m6_a0_contracts.py` | `51e026fcb51f233852a7ca340c2b2f8997bc2a0bb9e1d8fa2e7d68bef53050e4` |

## 3. 冻结结果

### 3.1 配置和执行

- 模块数：12；
- 预设数：4；
- `physics` 合并顺序：preset → remove → overrides → modules → 验证；
- 缺少 `physics`：纯平流；
- 旧 `enabled` 和字符串 `parameters`：带字段路径的配置错误；
- 显式 `order`：all-or-none、从 0 连续、唯一且满足依赖；
- 随机生成器：`Philox4x32-10`；
- w1/wN：规范化科学字节一致；
- 生产执行路径：共同子步、对称二阶分裂、单一中点运动积分。

### 3.2 资料与验证

- 辅助资料：4 类；
- 验证资产合同：9 组；
- 正式基础矩阵：96 cells；
- 平台：Windows x86_64、Ubuntu 24.04 x86_64；
- 气象资料：ERA5 hybrid、ERA5 pressure、CFSR pressure；
- 任务：water vapor、gas、aerosol、buoyant release；
- 方向：forward、backward；
- worker：w1、w4。

GMTED2010 采用 USGS 30 arc-second mean/std 产品。MCD12C1.061 和 MCD15A3H.061 采用 NASA LP DAAC 正式集合与 Earthdata 获取流程。三者的使用条款链接和固定引用文本已经写入注册表；软件包不内置这些全球资料。

Kincaid SF6 正式归档已经冻结：

```text
size:    3,129,058 bytes
SHA-256: 8b645ac7c4dbbc126a315124d4bf7bb8863077b25f06155bca17bc00d2445470
```

### 3.3 OH 资料裁决

Spivakovsky OH 论文的正确 DOI 为：

```text
10.1029/1999JD901006
```

候选 `OH_7lev_agl.dat` 已记录精确身份：

```text
commit:  3d7eebf7c4909f59db5ec32c524f88fb846e9fe5
size:    1,161,216 bytes
SHA-256: c8c362df48a525e240c86fbead7b9e78709a26a330ac050ac7676fb53f7110d7
```

候选文件缺少足以建立科学来源链的网格、单位、生成过程和再分发说明。注册表将其标为：

```text
local_use:     blocked_pending_provenance
redistribution: unverified
M6-A6:         external_blocked
```

未选择常数 OH、其它气候场或未经审定的替代文件。

### 3.4 产物和恢复

SQLite v2 共冻结 15 张公共表，其中新增 forward/backward 分离状态、固定粒径、过程汇总、公共事件和五类明细。Schema 已在真实内存 SQLite 中执行，并通过以下反例：

- 同一粒子、模块、物种和宏步的重复事件被拒绝；
- backward 事件写入质量字段被拒绝；
- 超出 `0.01–100 µm` 的粒径被拒绝；
- 外键检查和 integrity check 通过。

检查点默认墙钟间隔为 1800 s，只在完整宏步边界发布，保留最近两代。自动恢复最多执行 3 次；每次创建新 attempt。完整任务、OOM、磁盘不足、损坏检查点和科学错误不会自动恢复。

## 4. M6-A0 校验结果

```json
{"a6_oh_status":"external_blocked","auxiliary_datasets":4,"base_matrix_cells":96,"modules":12,"presets":4,"sqlite_tables":15,"sqlite_user_version":2,"status":"passed","validation_assets":9}
```

校验器同时执行：

- 5 对 Draft 2020-12 Schema/example；
- 版本和 A0 阶段边界；
- 模块、预设、依赖和参数覆盖一致性；
- 随机维度无碰撞；
- IGBP 1–17 到 Wesely 1–13 映射完整性；
- 官方资料使用条款与引用字段；
- OH 与 Kincaid 文件身份；
- 检查点路径 containment、计数、高水位和 payload identity；
- forward/backward 过程查询语义；
- SQLite v2 结构和约束反例；
- 科学合同完整度。

## 5. 回归门禁

```text
python tools/validate_m6_a0_contracts.py
passed

python tools/validate_m5_a0_contracts.py
passed: 35 commands, 9 states, 0 placeholders, 60 product cells

python tools/validate_m4_a0_contracts.py
passed: SQLite v1 / WAL / provenance v1 unchanged

python -m py_compile tools/validate_m6_a0_contracts.py
passed

cargo test --offline --workspace
552 passed / 12 ignored (564 discovered)

cargo clippy --offline --workspace --all-targets -- -D warnings
passed

cargo doc --offline --workspace --no-deps
passed

cargo fmt --all -- --check
passed

git diff --check
passed
```

12 个 ignored 均为仓库已有的显式长矩阵或性能门，本轮没有更改 ignore 属性。

## 6. 边界遵守

- 未修改 `trajecta-core`、`trajecta-met`、`trajecta-case`、`trajecta-job` 或 CLI 实现；
- 未把 SQLite v2 接入生产 Runner；生产常量仍为 1；
- 未添加兼容解析、快/准双路径或旧算法回退；
- 未运行 M6 模拟、性能矩阵或 WSL 正式任务；
- 未修改软件版本、Case schema 或现有 M4/M5 合同；
- 未读取、修改、删除或提交 `E:\flexpart\origo-validation-v1.json`；
- 未使用子代理；
- 未 commit、未 push。

## 7. 停止点

M6-A0 已完成并停在此处。M6-A1 尚未开始。进入 A1 前由用户审阅本报告、科学合同和 OH 外部阻塞裁决。
