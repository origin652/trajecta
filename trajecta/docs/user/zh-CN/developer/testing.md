---
title: 测试与测试资料
description: Trajecta 的单元测试、格式约定、真实资料、运行时、软件包、性能和文档测试分层。
---

# 测试与测试资料

Trajecta 按测试所跨越的边界组织测试。解析器的局部约束通常可以在一个 crate 内检查；验证正式
分发的可执行文件，则需要全新解压的软件包、配套原生库、真实气象资料，以及生产守护进程和
工作进程组成的完整路径。

工作区测试套件是共同基线。专用矩阵会补充更广范围所需的资料和运行环境。

## 测试分层

| 层级 | 主要位置 | 回答的问题 |
| --- | --- | --- |
| 单元测试 | 模块内的 `#[cfg(test)]` 代码块 | 单个类型或算法是否满足局部约束？ |
| Crate 集成测试 | `crates/*/tests` | 公开模块在不借助私有测试入口时能否正确组合？ |
| Schema 与约定测试 | Crate 测试和 `testdata` 校验器 | 公开文件和机器消息是否维持声明的结构？ |
| 真实资料读取器测试 | `trajecta-met/tests/real_*` | 生产读取器能否正确解释由 SHA-256 固定内容的气象来源？ |
| 数值回放 | `trajecta-core/tests/m4_*` | 积分、边界、粒子群、质量与输出能否在真实资料上协调？ |
| 运行时约定 | `trajecta-cli/tests/m5_a2_runtime_contracts.rs` | 生产守护进程与工作进程能否完成、取消并保留执行轮次？ |
| 发布包 | `tools/m5_a4_package.py` 与 `tools/run_m5_a4_product_matrix.py` | 全新解压的归档包能否使用随包可执行文件和原生库运行？ |
| 验证与性能 | 固定版本的 M4/M5 运行器和发布资源文件 | 科学指标、内容散列、资源上限与计时规则是否继续满足？ |
| 文档 | 参考页生成器、文档校验器、MkDocs 与快速入门 | 示例、双语页面、参考页、链接和发布元数据是否与产品一致？ |

较宽的测试层通常会包含较窄层的前置条件，但两者各有用途。单位失败的定位速度远快于一个
失败的软件包测试单元。

## 工作区测试

从 `trajecta` 目录运行使用默认特性的完整测试套件：

```text
cargo test --offline --workspace
```

开发期间可以只运行目标 crate 或某个集成测试：

```text
cargo test --offline --package trajecta-case
cargo test --offline --package trajecta-met --test real_cfsr_pipeline
cargo test --offline --package trajecta-core --test m4_a2_domain_fill
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli --test m5_a1_cli_contracts
```

使用 `-- --list` 查看编译出的测试名称，或在末尾加入名称过滤条件运行单项：

```text
cargo test --offline --package trajecta-core --test m4_a2_domain_fill -- --list
cargo test --offline --package trajecta-core --test m4_a2_domain_fill boundary_mass
```

工作区静态检查与文档检查应和测试一起运行：

```text
cargo fmt --all -- --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
git diff --check
```

## 带版本的约定与测试资料

`testdata` 保存跨越单个模块的公共格式。主要分组如下：

| 前缀 | 内容 |
| --- | --- |
| `M3_*` | 读取器对比、容差和科学查询约定 |
| `M4_*` | 数值约定、运行清单、溯源信息、SQLite schema 和性能归因 |
| `M5_CLI_*` | 命令树、JSON 响应和流记录格式 |
| `M5_CONFIG`、`M5_PROJECT`、`M5_DATA_PLAN` | 本地配置与项目控制面格式 |
| `M5_JOB_*`、`M5_PRUNE_*` | 任务记录、事件、调度器约定和只读清理计划 |
| `M5_RESULT_*`、`M5_TRAJECTORY_*` | 结果检查与轨迹流格式 |
| `M5_BUILD_*`、`M5_PRODUCT_*` | 发布归档包与软件包矩阵格式 |
| M5 对比约定 | 仅供验证章节使用的对比格式 |
| `REAL_MET_MANIFEST.json` | 外部气象固定测试资料的预期来源、路径、大小和散列 |

schema 文件与示例会配对测试。修改 schema 时，应同时提供有效示例和明确的无效案例，并判断
是否需要更新 schema 标识符。即使 Alpha 阶段继续沿用原有 schema 标识符，增加字段时也需要在
同一次修改中更新所有写入端与读取端。

顶层校验器会检查 JSON 结构定义难以表达的关系：

```text
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
```

这些程序验证文件散列、命令覆盖、稳定常量和跨文档引用。它们属于源码检查，无需资料服务方的
凭据。

## 真实气象固定测试资料

体积较大或受提供方管理的资料放在 Git 外。运行清单记录文件大小和 SHA-256，环境变量将测试指向本地副本。

| 变量 | 资料系列 |
| --- | --- |
| `TRAJECTA_REAL_CFSR_DIR` | 官方 CFSR 气压层 GRIB 文件 |
| `TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR` | 已准备的官方 ERA5 气压层文件 |
| `TRAJECTA_REAL_ERA5_HYBRID137_DIR` | 已准备的官方 ERA5 137 层混合坐标文件 |
| `TRAJECTA_REAL_CFSR_PRESSURE_NETCDF_DIR` | CFSR 气压层 NetCDF 固定测试资料 |
| `TRAJECTA_REAL_ERA5_PRESSURE_NETCDF_DIR` | ERA5 气压层 NetCDF 固定测试资料 |
| `TRAJECTA_REAL_ERA5_HYBRID_NETCDF4_DIR` | ERA5 混合坐标 NetCDF4 固定测试资料 |
| `TRAJECTA_REAL_CFSR_DERIVED_MULTIFILE_DIR` | 由多个 CFSR 文件派生的 NetCDF 固定测试资料 |
| `TRAJECTA_REAL_NOAA_PSL_NCEP_R1_DIR` | NOAA PSL NCEP/NCAR Reanalysis 1 固定测试资料 |

许多日常真实资料测试在资源文件缺失时会提前返回。正式运行应设置
`TRAJECTA_REQUIRE_REAL_MET=1`，使缺少必需文件成为测试失败：

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

!!! tip "正式测试要求真实资料"

    设置 `TRAJECTA_REQUIRE_REAL_MET=1`，本地资料缺失时测试会立即失败。

断言应同时绑定源文件散列和运行配置。文件名相同而字节不同的资料属于另一份固定测试资料。

## 纯 Rust 与原生读取器测试

读取器修改通常需要三类比较：

1. 相同来源上的元数据与清单选择。
2. 纯 Rust 与原生读取器之间的场数组、掩码、网格、垂直坐标和有效时次。
3. 派生和插值完成后的端到端查询输出。

启用目标特性构建原生测试：

```text
cargo test --offline --package trajecta-met \
  --features native-eccodes --test real_cfsr_pipeline

cargo test --offline --package trajecta-met \
  --features native-netcdf --test real_netcdf_pipeline

cargo test --offline --package trajecta-met \
  --features native-eccodes,native-netcdf --test real_netcdf_pipeline
```

差异测试的容差来自带版本的容差约定。约定涉及完整场解释时，测试会比较所有选定的有效时次
和垂直层。单点采样适合冒烟检查，无法替代完整读取器等价性比较。

## 数值与生命周期回放

`trajecta-core/tests` 同时包含合成工程案例和真实资料回放，覆盖范围包括：

- 带符号的正反向时钟。
- 球面 RK2 收敛性与批次行为。
- 地表、模式顶、水平区域与周期边界。
- 定时释放、气团区域填充与臭氧区域填充。
- 动态边界生成与稳定粒子 ID。
- 质量账本闭合和正常、异常终止分类。
- 输出事件覆盖范围、SQLite 行主键、溯源信息和完整验证。
- 不同工作线程数下规范化输出的确定性。

耗时较长的真实资料与性能测试单元标为 `#[ignore]`，使日常工作区测试套件保持适中。配置好
固定测试资料准备完成后，可以显式运行这些目标：

```text
cargo test --offline --release --package trajecta-core \
  --test m4_a4_real_perf -- --ignored --nocapture
```

完整编排脚本还会统一测试单元选择、超时、资源测量、执行轮次保留方式和摘要验证。完整矩阵由
编排器执行；调查单个测试单元时，直接运行被忽略的测试更方便。

## 运行时约定

运行时集成测试会将已编译 CLI 分别作为守护进程和独立工作进程启动。每次测试使用隔离的配置、
任务数据库、IPC 端点、项目与结果根目录。约定覆盖完整运行，也覆盖保留执行轮次的安全取消与
强制取消路径。

```text
cargo test --offline --package trajecta-cli --test m5_a2_runtime_contracts
```

只有在调查主机特有进程竞态时才需要显式串行运行。测试自身已经使用独立路径。失败后应先查看
保留下来的运行时根目录，再处理进程；识别失去联系的工作进程时要同时匹配可执行文件路径、进程标识与运行 ID。

## 发布包

软件包测试从 `tools/m5_a4_package.py` 构建的归档包开始。检查程序先核对相邻校验和和安全
解压路径，再核对构建清单、载荷摘要与 SBOM。随后检查第三方许可证
清单、可执行文件散列和原生组件清单。

全新软件包快速运行测试包含两个 CFSR 测试单元：

- 纯 Rust 读取器、释放型粒子群、正向、1,000 粒子、一个工作进程。
- 原生读取器、释放型粒子群、反向、1,000 粒子、一个工作进程。

每个平台的正式矩阵包含 30 个测试单元：

| 分组 | 测试单元数 | 覆盖 |
| --- | ---: | --- |
| Rust 1k | 18 | 三种资料系列 × 三种粒子群 × 两个方向 |
| Rust 10k | 6 | 跨资料系列选择粒子群与方向组合，四个工作线程 |
| 原生 1k | 6 | 三种资料系列 × 定时释放 × 两个方向 |

运行器为每个测试单元创建新的执行轮次目录，并在首个失败处停止。前半段检查项目定稿、
守护进程生命周期、运行清单状态和异常终止。科学结果检查覆盖质量账本、气象质量、输入散列与
SQLite。收尾阶段再核对 WAL、轨迹访问、溯源信息、报告幂等性和
任务数据库状态。

软件包验证与矩阵应在已经准备好的正式环境中运行：

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

正式发布包矩阵还需要两个 ERA5 根目录，并将 `smoke` 替换为 `matrix`。

## 快速入门与教程

`tools/run_m5_1_quickstart.py` 可以针对源码树中的可执行文件或解压后的正式软件包执行文档命令。它会
创建全新项目，配置资源池，生成资料计划，完成项目定稿，运行深度环境检查，再以前台模式执行任务。
随后进行完整验证、结果检查、单条轨迹读取和运行报告生成。

四个示例项目为：

| 示例 | 资料与粒子群 |
| --- | --- |
| `domain-fill-cfsr` | CFSR 区域填充水汽追踪工作流 |
| `release-cfsr` | CFSR 定时释放工作流 |
| `air-mass-era5-pressure` | ERA5 气压层气团工作流 |
| `ozone-era5-hybrid` | ERA5 混合坐标层臭氧工作流 |

每周任务和发布工作流会在 Windows 与 Ubuntu 软件包上运行两个 CFSR 教程。两个 ERA5 教程
在 Ubuntu 上执行，资料提供方凭据通过 CI 机密变量提供。

## 文档测试

文档检查同时覆盖生成内容、手写内容、构建后的 HTML 和一条可执行快速入门：

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_validation_assets.py
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
mkdocs build --strict
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli --site-dir site
```

校验器先检查双语路径、导航、YAML 元数据和自动生成的参考页。随后核对文档中的命令与诊断码，
检查示例片段、SEO 元数据和写作风格。单独的开发站点构建会检查 `noindex`。外部链接
由带重试机制的 CI 任务验证，因此本地 Markdown 构建不受网络波动影响。

验证图表还有一项重建检查：

```text
python tools/validate_m5_1_validation_assets.py
```

它从冻结 JSON/CSV 重建五张发布图表，比较 SHA-256，并确认两个语言资源文件树的字节
完全一致。

## 保留失败现场

正式运行器只会创建新的执行轮次目录，不会覆盖失败测试单元。排查失败时，通常需要保留：

- 所用资料或软件包的版本与摘要；
- 命令记录与返回码；
- 最先失败的检查；
- 已经存在的运行清单、SQLite/WAL 状态、日志和中断诊断文件；
- 显示通过、失败与未运行测试单元的矩阵摘要；
- 首次失败后是否启动过后续测试单元。

外层观察窗口中断与软件包测试单元失败是两种状态。前者会留下部分执行轮次；只有矩阵约定允许
重试时，才创建下一个带编号的执行轮次。

## 增加测试

新测试应放在能够观察目标规则的最窄层级。每项测试保持一个清晰的失败原因；如果检查的是
crate 边界，应通过公开 API 执行。增加固定测试资料时：

1. 记录来源，以及许可证或资料使用条件；
2. 在对应运行清单中保存大小与 SHA-256；
3. 凭据和提供方配置留在固定测试资料外；
4. 分别定义本地运行与正式运行在资料缺失时的行为；
5. 保留仍能覆盖目标路径的最小空间与时间子集；
6. 清理步骤只处理临时文件，不删除失败的正式执行轮次。

最后依次运行范围明确的测试、负责该规则的 crate、工作区检查，以及[开发架构](index.md)修改路由表中
对应的专用矩阵。
