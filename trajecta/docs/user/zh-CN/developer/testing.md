---
title: 测试与 fixture
description: Trajecta 的单元、合同、真实资料、运行时、软件包、性能和文档测试分层。
---

# 测试与 fixture

Trajecta 按测试跨越的边界组织测试。Parser invariant 通常可以在一个 crate 内检查；验证正式分发的
binary 则需要全新解压的软件包、配套原生库、真实气象资料和 production daemon-worker 路径。

Workspace test suite 是共同基线。专用矩阵会补充更广范围所需的资料和运行环境。

## 测试分层

| 层级 | 主要位置 | 回答的问题 |
| --- | --- | --- |
| Unit test | Module 内的 `#[cfg(test)]` block | 单个 type 或 algorithm 是否保持局部 invariant？ |
| Crate integration test | `crates/*/tests` | Public module 不借助 private test access 时能否组合？ |
| Schema 与 contract test | Crate test 和 `testdata` validator | 公开文件和机器消息是否维持声明的结构？ |
| Real-data reader test | `trajecta-met/tests/real_*` | Production reader 能否正确解释身份固定的气象源？ |
| Numerical replay | `trajecta-core/tests/m4_*` | 积分、边界、population、质量与输出能否在真实资料上协调？ |
| Runtime contract | `trajecta-cli/tests/m5_a2_runtime_contracts.rs` | Production daemon 与 worker 能否完成、取消并保留 attempt？ |
| Product package | `tools/m5_a4_package.py` 与 `tools/run_m5_a4_product_matrix.py` | 全新解压的 archive 能否使用随包 binary 和 native library 运行？ |
| Validation 与 performance | 冻结的 M4/M5 runner 和 publication asset | 科学指标、身份、资源门与计时规则是否继续满足？ |
| Documentation | M5.1 generator、validator、MkDocs 与 quickstart | Example、双语页面、参考页、链接和发布 metadata 是否与产品一致？ |

较宽的测试层通常会包含较窄层的前置条件，但两者各有用途。Unit failure 的定位速度远快于一个
失败的软件包 cell。

## Workspace test

从 `trajecta` 目录运行完整默认 feature suite：

```text
cargo test --offline --workspace
```

开发期间可选择目标 crate 或 integration target：

```text
cargo test --offline --package trajecta-case
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
cargo test --offline --package trajecta-core --test m4_a2_domain_fill
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli --test m5_a1_cli_contracts
```

使用 `-- --list` 查看编译出的 test name，或在末尾加入 filter 运行单项：

```text
cargo test --offline --package trajecta-core --test m4_a2_domain_fill -- --list
cargo test --offline --package trajecta-core --test m4_a2_domain_fill boundary_mass
```

Workspace lint 与文档门禁应和 test 一起运行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
git diff --check
```

## Contract fixture

`testdata` 保存跨越单个 module 的公共格式。主要分组如下：

| 前缀 | 内容 |
| --- | --- |
| `M3_*` | Reader comparison、tolerance 和科学 query contract |
| `M4_*` | 数值合同、run manifest、provenance bundle、SQLite schema 和 performance attribution |
| `M5_CLI_*` | 命令树、machine envelope 和 stream-item format |
| `M5_CONFIG`、`M5_PROJECT`、`M5_DATA_PLAN` | 本地配置与 project control-plane format |
| `M5_JOB_*`、`M5_PRUNE_*` | Job record、event、scheduler contract 和 dry-run cleanup plan |
| `M5_RESULT_*`、`M5_TRAJECTORY_*` | Result inspection 与 trajectory stream format |
| `M5_BUILD_*`、`M5_PRODUCT_*` | Product archive 与 package-matrix format |
| M5 comparison contract | 仅供 Validation 章节使用的 comparison format |
| `REAL_MET_MANIFEST.json` | 外部气象 fixture 的预期来源、路径、大小和 hash |

Schema file 与 example 会配对测试。修改 schema 时，应同时提供 valid example、明确的 invalid case，
并决定是否需要更新 schema identity。Alpha 文档在现有 schema 内增加字段，也需要在同一次修改中
更新全部 producer 与 consumer。

顶层 validator 会检查 JSON Schema 难以表达的关系：

```text
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
```

这些程序验证文件身份、命令覆盖、稳定常量和跨文档引用。它们属于源码检查，无需 provider
credential。

## 真实气象 fixture

体积较大或受 provider 管理的资料放在 Git 外。Manifest 记录身份，环境变量将 test 指向本地副本。

| 变量 | 资料家族 |
| --- | --- |
| `TRAJECTA_REAL_CFSR_DIR` | Official CFSR pressure-level GRIB file |
| `TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR` | 已准备的 official ERA5 pressure-level file |
| `TRAJECTA_REAL_ERA5_HYBRID137_DIR` | 已准备的 official ERA5 hybrid 137-level file |
| `TRAJECTA_REAL_CFSR_PRESSURE_NETCDF_DIR` | CFSR pressure-level NetCDF fixture |
| `TRAJECTA_REAL_ERA5_PRESSURE_NETCDF_DIR` | ERA5 pressure-level NetCDF fixture |
| `TRAJECTA_REAL_ERA5_HYBRID_NETCDF4_DIR` | ERA5 hybrid NetCDF4 fixture |
| `TRAJECTA_REAL_CFSR_DERIVED_MULTIFILE_DIR` | Multi-file CFSR-derived NetCDF fixture |
| `TRAJECTA_REAL_NOAA_PSL_NCEP_R1_DIR` | NOAA PSL NCEP/NCAR Reanalysis 1 fixture |

许多普通 real-data test 在 asset 缺失时会提前返回。Formal run 应设置
`TRAJECTA_REQUIRE_REAL_MET=1`，使缺少必需文件成为 test failure：

```text
TRAJECTA_REQUIRE_REAL_MET=1 \
TRAJECTA_REAL_CFSR_DIR=/data/cfsr \
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
```

PowerShell 写法为：

```text
$env:TRAJECTA_REQUIRE_REAL_MET = "1"
$env:TRAJECTA_REAL_CFSR_DIR = "D:\met\cfsr"
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
```

!!! tip "正式运行要启用 fixture 硬门"

    设置 `TRAJECTA_REQUIRE_REAL_MET=1`，本地资料缺失时 test 会立即失败。

Assertion 应同时绑定 source hash 和 Profile。文件名相同而 byte 不同的资料属于另一份 fixture。

## Pure 与 native reader test

Reader 修改通常需要三类比较：

1. 相同 source 上的 metadata 与 inventory selection。
2. Pure 和 native backend 之间的 field array、mask、grid、vertical coordinate 与 valid time。
3. Derivation 和 interpolation 完成后的 end-to-end query output。

启用目标 feature 构建 native test：

```text
cargo test --offline --package trajecta-met \
  --features native-eccodes --test real_cfsr_pipeline

cargo test --offline --package trajecta-met \
  --features native-netcdf --test real_netcdf_pipeline

cargo test --offline --package trajecta-met \
  --features native-eccodes,native-netcdf --test real_netcdf_pipeline
```

Differential tolerance 来自带版本的 tolerance contract。合同涉及 full-field interpretation 时，test
会比较全部 selected valid time 与 level。只采样一个方便的 point 适合 smoke，不足以判断 reader
equivalence。

## 数值与生命周期 replay

`trajecta-core/tests` 同时包含 synthetic engineering case 和 real-data replay，覆盖范围包括：

- 带符号的正反向 clock。
- 球面 RK2 convergence 与 batch behavior。
- Surface、top、horizontal-domain 与 periodic boundary。
- Release、air-mass domain fill 与 ozone domain fill。
- Dynamic boundary birth 与 stable particle ID。
- Mass-ledger closure 和 normal/abnormal termination classification。
- Output-event coverage、SQLite row identity、provenance 和 full verification。
- 不同 worker count 下的 normalized-output determinism。

耗时较长的 real-data 与 performance cell 标为 `#[ignore]`，使日常 workspace suite 保持适中。
配置好冻结 fixture 后，可直接运行 ignored target：

```text
cargo test --offline --release --package trajecta-core \
  --test m4_a4_real_perf -- --ignored --nocapture
```

Formal orchestration script 还会固定 cell selection、timeout、resource measurement、attempt retention
和 summary validation。Acceptance matrix 宜通过 orchestrator 执行；调查单个 cell 时，直接运行
ignored test 更方便。

## Runtime contract

M5 runtime test 会将已编译 CLI 分别作为 daemon 和独立 worker 启动。每次 test 使用隔离的 config、
catalog、IPC endpoint、project 与 result root。合同覆盖完整运行，以及保留 attempt 的 cancel 与
force-cancel 路径。

```text
cargo test --offline --package trajecta-cli --test m5_a2_runtime_contracts
```

只有在调查主机特有进程竞态时才需要显式串行运行。Test 自身已经使用独立路径。失败后应先查看
retained runtime root，再处理进程；识别 stale worker 时要同时匹配 executable identity 与 run ID。

## 产品软件包

Package test 从 `tools/m5_a4_package.py` 构建的 archive 开始。Verify 先检查相邻 checksum 和安全
解压路径，再核对 build manifest、payload digest 与 SBOM。随后检查 third-party license
inventory、executable identity 和 native component inventory。

Clean-package smoke 包含两个 CFSR cell：

- Pure Rust reader、release population、forward、1,000 particles、一个 worker。
- Native reader、release population、backward、1,000 particles、一个 worker。

每个平台的 formal matrix 包含 30 个 cell：

| 分组 | Cell 数 | 覆盖 |
| --- | ---: | --- |
| Rust 1k | 18 | 三种资料家族 × 三种 population × 两个方向 |
| Rust 10k | 6 | 跨资料家族选择 population 与方向组合，四个 worker |
| Native 1k | 6 | 三种资料家族 × release 的两个方向 |

Runner 为每个 cell 创建新的 attempt 目录，并在首个失败处停止。前半段检查 project finalize、
daemon lifecycle、manifest status 和 abnormal termination。科学产品检查覆盖 mass、quality、input
identity 与 SQLite。收尾阶段再核对 WAL、trajectory access、provenance、report idempotence 和
catalog state。

Package verify 与 matrix 应在已经准备好的 formal environment 中运行：

```text
python tools/m5_a4_package.py verify \
  --archive <archive> \
  --extract-root <empty-directory> \
  --result <verification.json>

python tools/run_m5_a4_product_matrix.py smoke \
  --package-root <verified-root> \
  --artifact-root <new-artifact-root> \
  --cfsr-dir <cfsr-directory>
```

Formal product matrix 还需要两个 ERA5 root，并将 `smoke` 替换为 `matrix`。

## Quickstart 与教程

`tools/run_m5_1_quickstart.py` 可以针对 source-tree binary 或解压后的正式软件包执行文档命令。它会
创建全新项目，配置资源池，生成 data plan，finalize，运行 deep doctor，再以前台模式执行任务。
随后进行 full verify、result inspect、单条 trajectory 读取和 run report 生成。

四个 example project 为：

| Example | 资料与 population |
| --- | --- |
| `domain-fill-cfsr` | CFSR domain-fill moisture workflow |
| `release-cfsr` | CFSR release workflow |
| `air-mass-era5-pressure` | ERA5 pressure-level air-mass workflow |
| `ozone-era5-hybrid` | ERA5 hybrid-level ozone workflow |

每周任务和 release workflow 会在 Windows 与 Ubuntu 软件包上运行两个 CFSR tutorial。两个 ERA5
tutorial 在 Ubuntu 上执行，provider credential 通过 CI secret 提供。

## 文档测试

文档门禁同时检查生成内容、手写内容、构建后的 HTML 和一条可执行 quickstart：

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_validation_assets.py
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
mkdocs build --strict
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli --site-dir site
```

Validator 先检查双语 path、navigation、front matter 和生成参考页。随后核对已记录命令、
diagnostic code、snippet 与 SEO metadata，并执行 style constraint。单独的 dev-site build 会检查
`noindex`。External link 由带重试的 CI job 验证，因此本地 Markdown build 不受网络波动影响。

Validation chart 还有一项重建门禁：

```text
python tools/validate_m5_1_validation_assets.py
```

它从冻结 JSON/CSV 重建五张 publication chart，比较 SHA-256，并确认两个语言 asset tree 的 byte
完全一致。

## 失败 artifact

Formal runner 使用 create-new attempt directory，不会覆盖失败 cell。有效的失败交接应包含：

- 精确 source 或 package identity；
- Command transcript 与 return code；
- First failing check；
- 已经存在的 manifest、SQLite/WAL state、log 和 forensic file；
- 显示 passed、failed 与 not run cell 的 matrix summary；
- Stop-on-failure 后没有启动后续 cell 的确认。

外层观察窗口中断与 product cell failure 是两种状态。前者应保留 partial attempt；只有 matrix
合同允许 retry 时，才创建新的 numbered attempt。

## 增加测试

新 test 应放在能够观察目标规则的最窄层级。每项 test 保持一个清晰 failure reason；如果检查的是
crate boundary，应通过 public API 执行。增加 fixture 时：

1. 记录来源，以及 license 或 data-use term；
2. 在对应 manifest 中保存 size 与 SHA-256；
3. Credential 和 provider configuration 留在 fixture 外；
4. 分别定义 local run 与 formal run 的 missing-data behavior；
5. 保留仍能覆盖目标路径的最小空间与时间子集；
6. Cleanup 只处理 temporary file，不删除失败的 formal attempt。

最后依次运行 focused test、owning crate、workspace gate，以及[开发架构](index.md)修改路由表中
对应的专用矩阵。
