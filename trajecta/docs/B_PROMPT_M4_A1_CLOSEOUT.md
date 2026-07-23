# 给 B：M4-A1 工程闭环收尾

你是 B 模型。本轮只修工程合同与接线，不修改 A 已接管并冻结的数值核心。开始前完整阅读：

- `docs/TRAJECTA_M4_A0_SCIENCE_CONTRACT.md`
- `docs/TRAJECTA_M4_A1_A_DELIVERY_REPORT.md`
- `docs/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`
- `testdata/M4_PROVENANCE_BUNDLE.schema.json`
- `testdata/M4_PROVENANCE_BUNDLE.example.json`
- `testdata/M4_RUN_MANIFEST.schema.json`
- `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`

保留全部现有未提交改动；不要修改、暂存或提交仓库外层的 `origo-validation-v1.json`。本轮不 commit，不宣称 M4-A1/M4 完成。不得修改 `boundary/met_path.rs` 的 interval arithmetic、`Jet2`、root certificate、容差、预算或 64 次二分来修绿；若出现反例，保留最小复现并交 A。

## P0-1：warmup 做完整依赖闭包和逐域选择

当前 `max_warmup_frames_for_capabilities` 只匹配 capability 直接输出，且所有域共用一个全局最大值。修为：

1. 以 required capability 的 `FieldReference` 为根；
2. 将目标映射到 direct/derived field id；
3. 对 derived expression 解析 identifier，递归遍历 direct/derived 依赖；
4. 对闭包中每个 direct/derived field 统计 `temporal.warmup_frames` 最大值；
5. unknown identifier、重复 target、无法解析依赖或不一致 profile 必须 hard fail，不得按 0 继续；
6. warmup 按实际 `(DomainId, ProfileName)` 计算，`select_bracketing_frames` 对每个域使用自己的值，禁止全局最大值污染无关域；
7. 保留 exact-on-frame previous/next 和“guard 左侧仍需满足 warmup”的语义。

至少增加：

- capability output -> derived A -> derived B -> accumulated direct source 的传递测试；
- 无关 derived 高 warmup 不计入；
- 两个 domain/profile 分别 warmup=0/4，前者不得因后者缺历史而失败；
- 实际需要 warmup 的域缺历史 hard fail；
- forward/backward 选择相同时间集合；
- 当前 CFSR 03 UTC 仍只选 00/06。

优先复用 `trajecta-met` 已验证的 expression parser/compiled profile 信息；不要写字符串切割解析器。

## P0-2：正式 provenance bundle v1

严格按冻结文档接线，不再把 `provenance-table.json` 当正式证据。

### 数据与哈希

- 为 bundle、record entry、field set、sample assignment、manifest summary 建立正式 Rust 类型；
- `FieldKey` 使用稳定 snake_case canonical 或 `{namespace,name}`，禁止 `Debug` 文本；
- 每个 SQLite sample 分别提取 eastward wind、northward wind、geometric vertical velocity、air pressure、air temperature 的完整 `ProvenanceRecord`；
- 保留 sources 顺序、transform 顺序、完整 parameters、fallback_reason、quality、profile SHA；
- transform parameters 输出前按 `(name,value)` 排序，重复参数 hard fail；
- record SHA 和 field-set SHA 必须是真正 RFC 8785 canonical JSON 的 SHA-256，不得假定普通 `serde_json::to_vec` 等价；
- 同 SHA 不同 canonical bytes 立即 hard fail。

### 全覆盖和资源边界

- bundle 中每个 `(particle_id, sample_sequence)` 与 SQLite `particle_state` 一一对应；不得遗漏、多出或重复；
- non-null SQL 数值必须有对应 record；SQL NULL 但 QueryOutput 有 provenance 时仍保留 record；
- field-set slot 引用的 record.field 必须一致；
- records、field_sets、samples 按合同固定排序；
- 逐行 sample 必须有界 spool/外排归并，不能把 10 万粒子的完整 sample 列表全部常驻内存；去重字典可留内存，但需有显式上限和 hard fail。

### 终态顺序与原子性

1. 完成 SQLite WAL checkpoint 并关闭 writer；
2. 计算最终 `particles.sqlite` SHA；
3. 同目录临时文件生成 bundle，flush/sync、校验 schema/count/reference/hash 后 atomic replace；
4. 计算 bundle 精确字节 SHA；
5. terminal manifest 写入 provenance identity 与 counts；
6. canonical run digest 纳入 bundle identity，不再依赖 SQLite 单个 `provenance_id`；
7. 每次运行开始清理或隔离本 run 的旧临时/旧 bundle，失败时不得留下 `complete` manifest 指向残留文件。

至少增加：hash collision、record hash 错、field-set hash 错、错 slot、缺引用、重复/遗漏/多余 sample、SQLite SHA 错、旧 artifact、atomic replace 失败、manifest 写失败等负例。

确定性 hard gate：1 worker/4 workers、不同 chunk、输入逆排列产生完全相同的 bundle bytes/SHA 和 canonical run digest。真实 CFSR E2E 中每个 U/V/W/P/T 非空输出都必须可解析到正确完整 record。

## P0-3：一般凹 Polygon 与多 hole

闭合当前只覆盖部分反例的 mesh：

- 一般合法凹 exterior；
- 多个 hole，包含凹 hole；
- 日期线 shell/hole；
- MultiPolygon；
- canonicalization 后无 double-cover、无 hole 泄漏、面积守恒；
- 按球面面积采样，不能重采、clamp、质心回退或静默丢三角；
- 自交、相切/交叉 ring、180° 歧义、极点歧义等不支持输入必须稳定 hard fail。

可采用可靠的约束三角化/earcut 工程实现，但球面投影、日期线规范化、orientation、面积权重和逆映射必须有明确合同与冻结反例。若现有离线依赖无法提供通用保证，不得假装完成；提交最小失败 fixture 给 A 裁决。

## P1：补小型文件级矩阵

保持默认测试小型，不运行旧百万点。补齐能在日常门禁运行的：

- solid-body 与 RK2 order；
- AGL/pressure exact-birth release；
- surface/model-top/limited-domain 与失败注入；
- forward/backward；
- instantaneous/continuous release；
- output exact-time query 不复用 midpoint；
- 1/4 worker、chunk、逆排列 digest。

10 万粒子、WSL、native 与三套真实资料完整轨迹矩阵留到 A 再授权的长测轮次。

## 门禁与报告

实际运行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/validate_m4_a0_contracts.py
python tools/validate_m4_a0_contracts.py
git diff --check
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-core --test m4_a1_cfsr_e2e -- --nocapture
```

更新 `docs/TRAJECTA_M4_A1_B_DELIVERY_REPORT.md`，逐项写 implemented/executed/passed/blocked，并列出 bundle/SQLite 路径、size、SHA、counts、worker/chunk/permutation digest。明确：未 commit、不宣称 M4-A1/M4 完成、未修改 A interval segmentation、未碰 `origo-validation-v1.json`。
