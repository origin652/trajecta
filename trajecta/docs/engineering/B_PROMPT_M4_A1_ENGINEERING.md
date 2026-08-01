# 给 B：完成 M4-A1 普通 release 工程闭环

你是 B 模型。请在 A 已冻结并实现的 M4-A1 数值/生命周期核心上完成工程接线。开始前完整阅读：

- `docs/engineering/TRAJECTA_M4_PARTICLE_LOOP_PLAN.md`
- `docs/engineering/TRAJECTA_M4_MODEL_ASSIGNMENT.md`
- `docs/engineering/TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`
- `docs/engineering/TRAJECTA_M4_A1_A_DELIVERY_REPORT.md`

保持未提交交付；不得宣称 M4-A1 或 M4 完成；不得修改算法 ID、公式、容差、normal/abnormal 分类和 Runner 生命周期来“修绿”。

当前接手基线：A0 与 M4-A1 A 级核心均在工作区、尚未提交；本机基线门禁为 `282 passed, 2 ignored`，两条 ignored 是旧 M3 百万点长测。保留全部现有改动，不要回滚或重写 A 的实现；不要修改、暂存或提交仓库根目录的无关文件 `origo-validation-v1.json`。若发现必须变更 A 冻结合同才能继续，先记录最小复现与候选方案并停止该分支，交 A 裁决。

## 1. GeoJSON 与 deterministic sampling

实现 Case file/inline GeoJSON 的生产解析、规范化与 sampler：

- Point/MultiPoint、Line/MultiLine、Polygon/MultiPolygon；
- longitude 规范化、ring 方向/起点/排序、hole 归属；
- 大圆短弧；180° 歧义边硬失败；
- 跨日期线自动切分，切分前后球面面积相对差 `<=1e-12`；
- 点等权、线按球面长度、面按球面面积减洞采样；
- seed/population/event/ordinal 完全来自 `ReleaseSamplingRequest`；
- 随机维度只用公共常量：component=1、horizontal u=2、horizontal v=3；vertical sampler 使用 4，birth time 的 0 维不得复用；
- chunk、worker、storage permutation 不改变坐标；
- 外部 `.geojson` 保存 canonical path、size、SHA 和 canonical geometry SHA；
- 采样点证明位于原始球面区域内，不得重采/clamp/丢弃。

## 2. Vertical resolver

实现 `ReleaseVerticalResolver` 生产版本：

- ASL 可机械透传但仍检查 finite/地下/越顶/域外；
- AGL 在每粒子的 exact birth time + lon/lat 查询 terrain 后转 ASL；
- pressure 在 exact birth time 走正式 column geometry 转 ASL；
- 任一粒子失败则整个 release event 硬失败；
- 不得减少 particle count 或 mass；
- 不得把 surface、available top、physical model top 混用。

## 3. Production boundary sampler

实现 `BoundaryPathSamplerFactory` 的 MetEngine 版本：

- 位置沿 RK2 start/proposal 的球面路径插值，时间/height/offset/age 同步插值；
- terrain、physical model top、domain status 在对应物理时刻正式查询；
- `ordered_segments` 覆盖完整 `[0,1]`；
- 按穿越 grid cell 切分，并在 clearance/model-top 标量极值或 inside/outside 转换处继续切分，保证每段至多一个 down-crossing/出域转换；
- `retarget` 后只表示碰撞后的剩余路径，局部 fraction 重置到 `[0,1]`；
- 不得把 A 的 root solver 改成固定采样点扫描或 endpoint clamp。

补真实 factory 单测：平坦地面、斜坡、先降后升、多 cell、日期线、模式顶、有限域、四次反射上限。

## 4. RunnerBuilder 与 registry

完成 `RunnerBuilder::build` 和内置 registry：

- 精确解析 integrator/boundary/population/output/sink ID；
- unknown/duplicate/incompatible ID 构建期硬失败；
- 构建 Profile/catalog/lock、加载所需 frames 并在首步前填充 MetEngine cache；
- 编译 transport plan 和各 output generic query plan；
- 生成或读取 seed，并在首粒子前写入 running manifest；
- 创建唯一 run directory 和合法 UUID-v7 run ID；
- output schedules 按 direction 排序并与 start/end/birth 去重；
- builder 只接线，不复制或改写 A 的 Runner loop。

## 5. Manifest 与 SQLite

实现生产 `RunManifestStore`、`LifecycleClock`、`ParticleStateProduct` 和 `particle_state_sqlite/v1`：

- manifest 同目录 `.tmp` + flush/sync + atomic replace；
- 进程中断保留 running，不自动伪造 complete；
- SQLite 严格执行 `testdata/M4_SQLITE_SCHEMA.v1.sql`；
- WAL、synchronous=NORMAL、foreign_keys、user_version 全校验；
- 一个 logical output event 一个事务，不逐粒子 commit；
- birth/start、interval、termination/end 重合行规范去重；
- 核心数值用 typed columns，不用 JSON blob；
- sink 写入失败为 run fatal，最终 manifest=failed；
- 支持运行中独立只读连接读取已提交事件；
- `integrity_check`、row counts、indexes 和规范 SQL digest 自动化。

## 6. 合成端到端 hard gates

必须通过实际 `RunnerBuilder -> SimulationRunner -> SQLite`，不能只测 fake components：

- zero wind stationary；
- constant east/north/W；
- solid-body rotation；
- forward/backward；
- observed RK2 order `>=1.9`；
- 日期线/高纬；
- 地面连续反射、模式顶、有限域；
- instantaneous + continuous releases；
- ASL/AGL/pressure releases；
- exact count、exact event mass、stable IDs；
- interval output 气象在 output time 查询，证明不是 midpoint；
- 1 worker / 4 workers、不同 chunk 和输入逆排列的规范 SQL 摘要一致；
- 失败注入：坏 GeoJSON、无效 vertical、ID 冲突、manifest 原子写失败、SQLite 事务失败。

日常测试保持小型；不要把旧百万点或未来 10 万长测放进默认 workspace tests。

本轮只闭合 M4-A1，不提前进入 M4-A2/A3/A4；三套真实资料、native、WSL 和 10 万粒子在报告中只列为后续项，除非 A 另行授权。

## 7. 门禁与交付报告

至少实际运行：

~~~text
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python tools/validate_m4_a0_contracts.py
git diff --check
~~~

报告写入 `docs/engineering/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，逐项区分 implemented / executed / passed / blocked，并列出：

- 精确命令与 exit code；
- 新增/修改关键文件；
- SQLite artifact path/size/SHA、row counts、integrity；
- 正反向解析误差与收敛率；
- worker/chunk/permutation digest；
- 未运行的真实资料、native、WSL、10 万项目；
- “未 commit、不宣称 M4-A1/M4 完成”。

遇到以下情况停止相关分支并交 A 裁决：公式/容差不够、certified segment 无法从现有 grid API 构造、AGL/pressure 身份不清、output exact-time query 与 sink 合同冲突、任何需要改 normal/abnormal 分类的情况。
