# 给 B 模型：M3 Phase 2--4 A 评审返工 Prompt

你在 `E:\flexpart\trajecta` 工作。当前工作区已有大量未提交修改，全部视为用户资产：禁止 `git reset --hard`、`git checkout --`、删除或覆盖无关修改；本轮仍不 git commit，不宣称 M3 完成。

先完整阅读：

- `docs/TRAJECTA_M3_MET_QUERY_PLAN.md`
- `docs/TRAJECTA_M3_MODEL_ASSIGNMENT.md`
- `docs/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`
- `docs/TRAJECTA_M3_A_PHASE2_4_REVIEW.md`
- `docs/TRAJECTA_M3_B_PHASE2_4_HANDOFF_TO_A.md`

本轮目标不是增加“测试数量”，而是把 A 评审指出的 ERA5 和终审证据缺口真正闭合。

## 一、必须先修的阻断项

### 1. CF 单位

- 在公共单位系统中支持 SI 派生单位 `W = kg m2 s-3`；
- `W m**-2`、`W m-2`、`W/m2` 必须得到与 `kg/s3` 相同的维度和比例；
- 增加正常、等价、非法和复合单位测试；
- 不得删除 NetCDF `units`，不得用错误 Profile unit 绕过解析。

### 2. NetCDF source identity 升级

实现并冻结 `role + variable + expected layout` 选择：

- `OpenedSource` 保留 `FrameDescriptor.files` 中的逻辑 role；
- Profile identity 中 `role` 必须过滤逻辑成员；
- `variable` 交给具体 reader；
- `layout` 在 decode 后硬校验；
- reader/loader 不得静默忽略不认识的 identity selector；
- 对声明了 role 的多文件 Profile，缺 role、重复 role、多成员匹配、错误 layout 都写负例并硬失败；单文件 Profile 可以继续不声明 role。

ERA5 pressure Profile：

- pressure role 的 3-D `z` 映射为 extension geopotential，再按已有冻结公式派生 `geopotential_height`；
- surface role 的 2-D `z` 映射为 `surface_geopotential`；
- classic 转换恢复 3-D `z`，删除“为避免 AmbiguousSource 而省略 z”的做法；
- official NetCDF4 与 classic NetCDF3 都必须走同一逻辑字段合同。

### 3. ERA5 NearSurface 完整化

严格实现 A 已冻结的公式，不自行换常量或公式：

```text
H_up = -ishf_down
LE_up = -M3_CONSTANTS.latent_heat_vaporization_j_kg * ie_down

e(Td) = 611.21 * exp(17.502 * (Td - 273.16) / (Td - 32.19))
epsilon = M3_CONSTANTS.dry_air_gas_constant_j_kg_k
        / M3_CONSTANTS.water_vapour_gas_constant_j_kg_k
q2m = epsilon * e / (sp - (1 - epsilon) * e)
```

- 为 q2m 和 moisture-flux -> latent-heat-flux 增加明确的版本化 derivation op/algorithm ID；
- 输入有效性、mask 交集、单位维度、quality=`Derived`、provenance 都必须正确；
- `ishf` 必须显式 negation，不能仍作为 Source 直接发布 canonical sensible heat flux；
- pressure/hybrid Profile 都补齐 `d2m`、`ie` 和派生字段；
- 增加 capability 固定合同校验：NearSurfaceTransport 缺 q2m、latent 或其它强制输入时，Profile load 必须失败；friction velocity 与 stress 采用 A 文档规定的 one-of 组；
- 增加正/负热通量、凝结/蒸发、低温 dewpoint、非法 `sp <= e`、mask 传播测试。

### 4. hybrid `lnsp` provenance

- `sp = exp(lnsp)` 科学上已获 A 批准；
- Profile 必须从 Source `lnsp` 经 `surface_pressure_from_log` 生成 canonical `surface_pressure`；
- 输出 quality/provenance 必须为 `Derived` 并能追溯到 raw `lnsp`；
- prepared 文件若因 CF `formula_terms` 保留 convenience `sp`，Profile 不得把它作为 canonical Source；
- preparation 必须逐值校验 hybrid、lnsp、surface base/flux 的 time/lat/lon 坐标一致，不只检查 shape。

## 二、重做可复现资料链

pressure 和 hybrid 都采用：

```text
raw/       # 服务原始字节，只读、永不 stamp
ready/     # 可确定性重建的 normalized/prepared 文件
FETCH_MANIFEST.json
PREPARE_MANIFEST.json
```

要求：

- fetch 使用 `.part`/原子完成；已有文件先校验 size/SHA；
- FETCH 只记录 raw；PREPARE 只记录 ready；
- manifest 不自哈希；不记录 CDS key 或片段；
- preparation 不得改 raw；
- 合并重复的 hybrid 获取入口，保留一个正式可从空缓存运行的命令；其它脚本删除或明确标 deprecated，不能让复现依赖手工补步骤；
- 自动获取/生成 model-level、lnsp、surface base/flux、PV A/B，并冻结请求参数、job、size、SHA、来源、attribution；
- 修复中央 `testdata/REAL_MET_MANIFEST.json`，并增加 raw/ready/FETCH/PREPARE 全量自校验测试；
- clean fetch + prepare 后再次运行，所有 manifest 必须稳定一致。

## 三、补齐两套 ERA5 真正正向链

不得再把 `lock -> frame` 测试命名或报告成 M3 full chain。pressure 00/06/12 和 hybrid 00/03/06 分别完成：

```text
fetch/manifest
-> lock/inventory
-> frame loading
-> TransportPlan(allow_estimated=false)
-> prepare
-> prepare_batch
-> execute
-> CLI probe
-> CLI replay
```

每套至少覆盖：

- AGL、ASL、Pa；
- 内部整帧和两个帧间时刻；
- 海面、平原、山地、近地层、自由大气、高空；
- 八个 Transport 输出、validity、quality、provenance、bounds；
- BelowGround、AboveAvailableTop、端点 MissingSymmetricTimeSupport；
- 同一 index、PreparedWindow 和 CLI 重复查询；
- NearSurfaceTransport 完整字段；
- pressure 逐层 `z` 和 hybrid omega 完整运动学 W 的路径证据。

真实测试在 `TRAJECTA_REQUIRE_REAL_MET=1` 时不得软跳过。新增 ERA5 CLI 集成测试，不只调用 reader API。

## 四、native 差分：只测量，不自行定容差

- 在可用 Windows 环境运行 native-netcdf；可用时同时运行 native-eccodes；
- 比较所有时次、所有字段、metadata、layout、mask、unit、temporal、quality、provenance 和最终查询输出；
- 输出机器可读测量：exact mismatch count、max abs、max rel、最坏索引和对应值；
- 删除/停用把 `1e-4/1e-5` 当正式认证的硬编码说明；
- 未匹配 tolerance registry 时状态必须是 `unvalidated_measurement`，不能写“通过”；
- A 将根据测量另行冻结 registry，B 不得放宽阈值或创建 allowlist。

## 五、百万点终审补口

在现有 CFSR 百万点主路径基础上增加：

- 至少三种 chunk 大小；
- 原顺序与固定随机 permutation；
- 冷/热缓存；
- 1 与多线程 bitwise；
- execute 前后 reader/provider 调用计数，execute 内必须为 0；
- engine accounted bytes 与进程 peak RSS/allocator peak；
- N 与 2N 的耗时和临时内存报告，证明近似线性；
- 结果用稳定 hash 或全列比较，不能只抽样有限值。

吞吐只记录，不冻结跨机器绝对门槛。若 Windows 无可靠 RSS API，明确采用的替代测量并在报告中说明局限。

## 六、oracle 与平台边界

- 保留现有 FLEXPART stub 和 MIT/GPL 进程隔离；
- 没有真实 FLEXPART binary/case harness 时继续明确 `oracle_failure`，不得伪造 samples；
- 若环境已具备，则生成固定 commit/compiler/flags/input hash 的真实 JSON；仍不得链接 GPL 到 MIT crate；
- Linux 未实际运行就写“未运行”，CI 配置存在不等于平台通过。

## 七、门禁与交付报告

至少运行并逐条报告：

```text
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace -- --skip cfsr_pgbl_million_point
cargo doc --offline --workspace --no-deps
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met --tests
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-cli --test met_cli_contracts
# 百万点单独完整运行
# native feature 命令与实际环境结果
```

交付报告必须分成：

1. 已实现且实际运行；
2. 已实现但未运行；
3. 外部阻断；
4. A 仍需裁决；
5. 明确未宣称。

报告中必须附：两套 ERA5 CLI 成功输出摘要、manifest 自校验结果、pressure 3-D/2-D `z` 选择负例、q2m/热通量 provenance、native 测量 JSON、百万点内存/I/O/确定性数据。

本轮完成条件：上述阻断项闭合且真实 ERA5 查询链实际成功；仍不得自行宣称 M3 完成、A2 通过或容差认证完成，也不要 git commit。
