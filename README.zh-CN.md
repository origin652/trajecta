# Trajecta

**开源的拉格朗日水汽追踪与大气轨迹框架**

[English](README.md)

[![Release](https://img.shields.io/github/v/release/origin652/trajecta?include_prereleases&sort=semver)](https://github.com/origin652/trajecta/releases)
[![中文文档](https://img.shields.io/badge/docs-简体中文-4051b5)](https://origin652.github.io/trajecta/latest/zh-CN/)
[![Docs CI](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml/badge.svg)](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Trajecta 面向水汽来源分析和大气输送研究。它在气象资料中生成并推进拉格朗日粒子，支持
正向与反向积分，并把一次模拟所用的科学设置、输入资料和轨迹结果完整保存在运行目录中。
同一套项目文件和命令行流程可以在 Windows 与 Ubuntu 上使用。

区域填充水汽追踪是 Trajecta 的主要应用方向。框架也支持定时释放、等质量气团和
平流层臭氧等粒子群，可用于来源区识别、输送路径分析以及不同时间和空间范围的轨迹研究。

> **当前版本：** `0.1.0-alpha.1`。Alpha 版本已经提供完整的命令行、项目配置、公开
> JSON Schema 和磁盘结果格式。Rust crate API 仍在随内部实现调整，主要供仓库开发使用。

## Trajecta 可以完成什么

| 研究或运行需求 | Trajecta 中的做法 |
| --- | --- |
| 追踪研究区域内的水汽来源 | 按空气质量在区域内生成粒子，随气流积分，并记录水汽收支与粒子轨迹 |
| 研究特定时刻或位置释放的粒子 | 配置定时释放，选择正向或反向积分 |
| 表示有限区域内的气团 | 按等干空气质量生成粒子群，持续记录输送和边界事件 |
| 研究平流层臭氧输送 | 根据臭氧相关条件初始化粒子，并沿三维气象场推进 |
| 在资料下载前准备研究项目 | 先编写案例和运行配置，再生成资料计划；资料到齐后完成项目定稿 |
| 同时管理多个模拟任务 | 使用本地守护进程、持久任务队列和资源池进行调度 |
| 从异常退出后继续工作 | 保留每次执行的独立目录；已经完成的任务不会因队列恢复而重新运行 |
| 读取和复核结果 | 使用命令行检查运行摘要、完整性和单粒子轨迹；高级分析可只读访问 SQLite |

当前可用的气象资料路径包括：

| 资料系列 | 垂直坐标 | 可用读取方式 |
| --- | --- | --- |
| CFSR | 气压层 | Rust 读取器或 ecCodes |
| ERA5 | 气压层 | Rust 读取器或 ecCodes |
| ERA5 | 混合坐标层 | Rust 读取器或 netCDF-C/HDF5 |

每个项目由三个部分组成：

- **案例文档（Case）**描述模拟时段、研究区域、粒子群、积分方向和数值设置。
- **运行配置（RunProfile）**保存本机资料路径、结果路径以及 CPU 和内存需求。
- **资料锁（DatasetLock）**记录实际参与运行的资料文件、覆盖范围和内容散列。

项目完成定稿后，Trajecta 将任务交给本地守护进程。工作进程读取已经锁定的气象资料，
执行数值积分，并把轨迹、终止原因和质量统计写入 SQLite。运行结束时还会生成运行清单、
解析后的项目文档和溯源信息。前台运行适合单个案例，后台模式和任务队列适合连续提交多个案例。

```text
案例 + 运行配置 + 气象资料
            │
       项目校验与定稿
            │
       本地任务队列
            │
    工作进程执行数值积分
            │
 SQLite 轨迹 + 运行清单 + 溯源信息
```

当前版本尚未提供插件加载和通用结果导出功能，因此配置文件和命令行中也没有对应入口。
内部模块已经按读取器、数值过程和输出端划分，今后的插件设计会沿这些位置继续展开。

## 快速开始

第一次使用可以选择发布包，也可以从源码构建。发布包已经包含可执行文件和所需的原生运行库；
源码构建便于查看实现、修改代码或使用本机工具链。

### 使用发布包

1. 在 [GitHub Releases](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1)
   下载 Windows x86_64 或 Ubuntu 24.04 x86_64 归档。
2. 下载同名 SHA-256 文件，校验归档后再解压。
3. 按[十五分钟快速入门](https://origin652.github.io/trajecta/latest/zh-CN/getting-started/quickstart/)
   下载约 22 MiB 的四时次 CFSR 演示资料。
4. 完成环境检查、项目定稿和区域填充示例，随后读取已经验证的轨迹结果。

发布包中的主程序名如下：

| 平台 | 主程序 |
| --- | --- |
| Windows x86_64 | `trajecta.exe` |
| Ubuntu 24.04 x86_64 | `trajecta` |

先确认程序能够启动：

```text
trajecta --version
trajecta --help
```

### 从源码构建

基础构建使用 Rust 读取器，可以直接运行 CFSR 演示案例。需要 ecCodes 或 netCDF-C 读取器时，
再安装对应的原生开发库并启用构建特性。

| 组件 | 版本或用途 |
| --- | --- |
| Rust | `1.85.0`；工作区最低版本为 `1.85`，使用 Rust 2024 edition |
| Cargo | 随 Rust 工具链安装；按 `Cargo.lock` 构建依赖 |
| Python | `3.11` 或更高版本；用于资料助手、校验工具和文档 |
| C/C++ 工具链 | 仅在构建原生读取器时需要，ABI 必须与 Rust 目标一致 |
| ecCodes | GRIB 原生读取器；基础 Rust 构建无需安装 |
| netCDF-C 与 HDF5 | NetCDF 和混合坐标层原生读取器；基础 Rust 构建无需安装 |

Windows PowerShell：

```powershell
git clone https://github.com/origin652/trajecta.git
Set-Location .\trajecta\trajecta

rustup toolchain install 1.85.0 --profile minimal
rustup override set 1.85.0
cargo build --locked --release --package trajecta-cli

.\target\release\trajecta-cli.exe --version
.\target\release\trajecta-cli.exe --help
```

Ubuntu 24.04：

```bash
git clone https://github.com/origin652/trajecta.git
cd trajecta/trajecta

rustup toolchain install 1.85.0 --profile minimal
rustup override set 1.85.0
cargo build --locked --release --package trajecta-cli

./target/release/trajecta-cli --version
./target/release/trajecta-cli --help
```

构建完成后，Windows 可执行文件位于 `target/release/trajecta-cli.exe`，Ubuntu 可执行文件
位于 `target/release/trajecta-cli`。正式发布包会把它重命名为更简短的 `trajecta`。

需要与正式软件包相同的原生读取器时，Ubuntu 可以先安装开发库：

```bash
sudo apt-get update
sudo apt-get install pkg-config libeccodes-dev libnetcdf-dev libhdf5-dev libclang-dev
```

随后启用两个原生读取特性：

```bash
cargo build --locked --release --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

Windows 原生构建使用 MSYS2 UCRT64 与 `x86_64-pc-windows-gnu` 工具链，ecCodes、netCDF-C
和 HDF5 需要来自同一 ABI 环境。完整的环境变量和安装步骤见
[构建环境](https://origin652.github.io/trajecta/latest/zh-CN/developer/build/)。

## 运行第一个项目

仓库与发布包都带有 `examples/domain-fill-cfsr` 示例。准备好演示资料后，流程依次为：

```text
trajecta --config quickstart.toml config init
trajecta --config quickstart.toml config set resources.cpu_slots 1
trajecta --config quickstart.toml config set resources.memory_reserve_mib 256
trajecta --config quickstart.toml config set resources.memory_pool_mib 1536
trajecta --config quickstart.toml config validate

trajecta --project examples/domain-fill-cfsr project validate
trajecta --format json --project examples/domain-fill-cfsr project data-plan --output data-plan.json
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json

trajecta --config quickstart.toml --project examples/domain-fill-cfsr project finalize
trajecta --config quickstart.toml --project examples/domain-fill-cfsr doctor --deep
trajecta --config quickstart.toml --project examples/domain-fill-cfsr run --profile product
```

资料助手默认只列出所需文件和目标路径。加入 `--execute` 后才会下载资料。下载完成后仍需
显式运行 `project finalize`，由 Trajecta 检查文件并生成资料锁。

`run` 默认在前台等待，并持续显示任务状态。加入 `--detach` 后，命令会在任务进入队列后返回；
可以使用 `job events JOB_ID --follow` 查看后续事件，或用 `job wait JOB_ID` 等待运行结束。

## 结果目录

一次成功运行会在项目的 `runs/` 目录下创建独立的执行目录。主要文件包括：

| 文件 | 内容 |
| --- | --- |
| `run-manifest.json` | 运行状态、输入文件、数值设置、统计信息和产物清单 |
| `particles.sqlite` | 粒子属性、轨迹状态、事件、终止原因和质量收支 |
| `resolved-case.json` | 本次运行实际采用的案例文档 |
| `resolved-run-profile.json` | 本次运行实际采用的运行配置 |
| `provenance-bundle.json` | 输出记录对应的气象来源和资料处理过程 |
| `run-report.md` | 由 `run report` 生成的可读摘要 |

日常读取优先使用以下命令：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id PARTICLE_ID
trajecta run report --result RESULT
```

`result inspect` 适合快速查看任务状态和记录数量，`result trajectory` 可以按粒子编号读取轨迹。
需要批量处理时，可选择 JSON 或 JSONL 输出。SQLite 的只读表结构和字段含义收录在
[结果与 SQLite 参考](https://origin652.github.io/trajecta/latest/zh-CN/reference/results-sqlite/)。

## 文档

中文与英文手册使用 MkDocs Material 构建。英文页面位于默认地址，中文页面位于
`/zh-CN/`。两套页面保持相同的章节结构。

| 内容 | 简体中文 | English |
| --- | --- | --- |
| 安装、配置和第一次运行 | [入门](https://origin652.github.io/trajecta/latest/zh-CN/getting-started/) | [Getting Started](https://origin652.github.io/trajecta/latest/getting-started/) |
| 完整科研案例 | [教程](https://origin652.github.io/trajecta/latest/zh-CN/tutorials/) | [Tutorials](https://origin652.github.io/trajecta/latest/tutorials/) |
| 按任务查找操作方法 | [操作指南](https://origin652.github.io/trajecta/latest/zh-CN/how-to/) | [How-to Guides](https://origin652.github.io/trajecta/latest/how-to/) |
| 模型和项目概念 | [概念](https://origin652.github.io/trajecta/latest/zh-CN/concepts/) | [Concepts](https://origin652.github.io/trajecta/latest/concepts/) |
| 队列维护与异常恢复 | [运行维护](https://origin652.github.io/trajecta/latest/zh-CN/operations/) | [Operations](https://origin652.github.io/trajecta/latest/operations/) |
| 科学与性能比较方法 | [验证](https://origin652.github.io/trajecta/latest/zh-CN/validation/) | [Validation](https://origin652.github.io/trajecta/latest/validation/) |
| 命令、配置和文件格式 | [参考](https://origin652.github.io/trajecta/latest/zh-CN/reference/) | [Reference](https://origin652.github.io/trajecta/latest/reference/) |
| 构建、测试和内部实现 | [开发手册](https://origin652.github.io/trajecta/latest/zh-CN/developer/) | [Developer Guide](https://origin652.github.io/trajecta/latest/developer/) |

## 支持平台

| 平台 | 架构 | 发布状态 |
| --- | --- | --- |
| Windows | x86_64 | 提供 `0.1.0-alpha.1` 预发布包 |
| Ubuntu 24.04 | x86_64 | 提供 `0.1.0-alpha.1` 预发布包 |

平台归档中包含主程序、原生运行库、四个示例项目、资料助手和精简的双语离线指南。
气象演示资料单独发布，可以在两个平台间共用。

## 验证与性能资料

[验证手册](https://origin652.github.io/trajecta/latest/zh-CN/validation/)介绍科学比较的案例设置、
统计量和可比较范围，也记录性能测试所用的平台、任务规模、读取器及计时范围。图表由随仓库保存的
CSV 或 JSON 数据生成，便于在修改计算或输出路径后重新运行同一套比较。

## 仓库结构

```text
.
├── trajecta/
│   ├── crates/           Rust 工作区及各功能 crate
│   ├── docs/user/        中英文用户手册源文件
│   ├── docs/engineering/ 计划、接口约定和执行记录
│   ├── examples/         教程使用的完整示例项目
│   ├── packaging/        软件包内的离线资料和发布支持文件
│   ├── testdata/         JSON Schema、示例和固定测试资料
│   └── tools/            校验、打包和资料下载工具
└── .github/workflows/    测试、软件包和文档发布工作流
```

工程记录存放在 `docs/engineering/`，不会进入用户手册导航、站内搜索或 sitemap。

## 开发检查

依赖已经进入 Cargo 缓存后，常用检查可以离线执行：

```text
cd trajecta
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
```

文档站使用 MkDocs Material：

```text
cd trajecta
python -m pip install -r requirements-docs.txt
mkdocs build --strict
```

构建产物位于 `site/`，也可以用 `mkdocs serve` 在本机预览。文档中的命令参考、Schema、
示例片段和中英文页面结构会在持续集成中检查。

## 参与项目

欢迎通过问题（Issue）报告运行故障、提出文档修订或讨论功能设计。范围明确的代码修改可以提交
拉取请求（Pull Request）。运行问题通常需要平台、Trajecta 版本、执行命令和诊断码；若运行目录
仍在，也可以附上经过检查且不含敏感信息的清单或日志片段。

- [提交问题](https://github.com/origin652/trajecta/issues/new)
- [查看现有问题](https://github.com/origin652/trajecta/issues)

## 许可证

Trajecta 使用 [MIT License](LICENSE)。
