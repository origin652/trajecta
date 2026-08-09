---
title: 构建环境
description: 在 Windows 或 Ubuntu 上构建 Trajecta 工作区和带原生读取器的完整 CLI，并运行本地开发检查。
---

# 构建环境

Trajecta 有两种常用构建形式。Cargo 默认构建适合日常 Rust 开发，完整构建会启用正式软件包使用的
ecCodes 与 netCDF-C 读取器。

本页命令都从仓库内的 `trajecta` 目录执行。

## 工具链

| 组件 | 项目基线 | 用途 |
| --- | --- | --- |
| Rust | `1.85.0` 或更高版本；使用 2024 edition | 五个工作区 crate 和 CLI 可执行文件 |
| Cargo | 随 Rust 工具链安装 | 锁定依赖、构建、测试、Clippy 检查和 rustdoc |
| Python | 3.11 或更高版本；CI 使用 3.12 | 校验器、打包、资料助手和文档 |
| Git | 当前受支持版本 | 源码修订、软件包输入清单和发布工作树 |
| C 工具链 | ABI 与 Rust 目标一致 | 链接 ecCodes、netCDF-C 和 HDF5 |
| MkDocs 依赖 | `requirements-docs.txt` 中的固定版本 | 本地预览和严格文档构建 |

工作区声明了 `rust-version = "1.85"`。CI 会明确安装 1.85.0。日常使用较新工具链时，涉及
语言特性或标准库的修改仍需回到该基线检查。

```text
rustup toolchain install 1.85.0 --profile minimal
rustup override set 1.85.0
rustc --version
cargo --version
```

Windows 上的软件包测试与运行时测试路径较深，建议先启用 Git 长路径：

```text
git config --global core.longpaths true
```

## 构建 Rust CLI

有网络时先获取锁文件指定的依赖，随后构建 CLI：

```text
cargo fetch --locked
cargo build --locked --package trajecta-cli
```

开发可执行文件位于：

| 主机 | 可执行文件 |
| --- | --- |
| Windows | `target/debug/trajecta-cli.exe` |
| Linux | `target/debug/trajecta-cli` |

确认可执行文件可以启动，并检查 Cargo 选中的版本：

```text
target/debug/trajecta-cli --version
target/debug/trajecta-cli --help
```

PowerShell 使用 Windows 路径写法：

```text
.\target\debug\trajecta-cli.exe --version
.\target\debug\trajecta-cli.exe --help
```

优化后的纯 Rust 可执行文件可用以下命令生成：

```text
cargo build --offline --locked --release --package trajecta-cli
```

Cargo 产物名为 `trajecta-cli`。确定性软件包构建器会将正式产品可执行文件重命名为 Windows
上的 `trajecta.exe`，Linux 软件包中则使用 `trajecta`。

## 原生读取器构建

正式产品启用以下特性组合：

```text
trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

两个特性都需要显式启用。构建环境缺少原生库或 ABI 不匹配时，编译会直接失败；已经选择
的读取器不会静默切换到另一种实现。

| 平台 | 原生开发环境 | 运行时条件 |
| --- | --- | --- |
| Ubuntu 24.04 x86_64 | `pkg-config`、ecCodes、netCDF-C 与 HDF5 的头部和库；重新生成绑定时需要 libclang | 动态加载器能够找到链接的共享库；ecCodes 能找到匹配的定义 |
| Windows x86_64 GNU | MSYS2 UCRT64 GCC、`pkgconf`、UCRT64 netCDF-C/HDF5、相同 GNU ABI 的 ecCodes；重新生成绑定时需要 libclang | UCRT64 与 ecCodes DLL 目录保留在 `PATH`；`ECCODES_DEFINITION_PATH` 指向匹配定义 |

Ubuntu 可先安装发行版提供的开发包：

```text
sudo apt-get update
sudo apt-get install pkg-config libeccodes-dev libnetcdf-dev libhdf5-dev libclang-dev
```

经过验证的 Windows GNU 环境使用 MSYS2 UCRT64 提供 netCDF-C、HDF5 和 `pkgconf`：

```text
pacman -S --needed mingw-w64-ucrt-x86_64-netcdf mingw-w64-ucrt-x86_64-pkgconf
```

Windows 环境中的路径应来自同一个 ABI 系列。启动 Cargo 前，典型的 UCRT64 配置包含：

```text
PATH=<ucrt64-bin>;<eccodes-bin>;%PATH%
NETCDF_DIR=<ucrt64-root>
PKG_CONFIG_PATH=<ucrt64-lib-pkgconfig>;<eccodes-lib-pkgconfig>
ECCODES_DEFINITION_PATH=<eccodes-share-definitions>
LIBCLANG_PATH=<directory-containing-libclang>
```

!!! warning "Windows 使用一套 ABI"

    `x86_64-pc-windows-gnu` Rust 应与 UCRT64 原生库配套。MSVC 导入库使用另一套
    工具链，无法混合链接。

开始构建前，可同时确认 Rust 主机和 C 库：

```text
rustc -vV
pkg-config --modversion netcdf
pkg-config --modversion eccodes
```

使用与正式软件包相同的特性构建：

```text
cargo build --offline --locked --release --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

命令会在 Windows 生成 `target/release/trajecta-cli.exe`，在 Ubuntu 生成
`target/release/trajecta-cli`。正式资料测试开始前，可以在同一环境执行 `--help`，先排除运行时
共享库缺失。

## 开发期间如何选择构建

| 工作范围 | 快速循环 | 需要补充的检查 |
| --- | --- | --- |
| 案例、运行配置、单位或诊断 | 默认特性与目标 crate 测试 | 工作区测试和结构定义/参考检查 |
| 纯 Rust 查询或数值代码 | 默认特性 | 相关真实资料回放与确定性输出测试 |
| GRIB 读取器或 ecCodes 元数据 | `native-eccodes` 测试 | 原生差异测试与软件包快速运行测试 |
| NetCDF 读取器、混合坐标层或 HDF5 访问 | `native-netcdf` 测试 | 正式目标上的原生差异测试 |
| 原生加载或打包 | 完整特性构建 | Windows 与 Ubuntu 的全新解压探测 |
| CLI 输出渲染或任务控制 | 默认 CLI 构建 | 运行时约定与机器输出测试 |

默认特性适合编辑期间快速检查。修改涉及文件解释或软件包加载时，还要运行对应的原生读取器
检查。

## 工作区检查

依赖进入本地缓存后，主要源码检查可以离线运行：

```text
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
git diff --check
```

工作区的 lint 规则会拒绝缺少文档的公开 Rust 项、unsafe 代码和未处理的 `must_use` 结果。生产
目标中的 `panic!`、`todo!` 与 `unimplemented!` 同样无法通过，`unwrap` 和 `expect` 也受限制。
测试模块只在准备固定测试资料确有需要时局部放宽规则。

编辑过程中可先运行软件包级命令：

```text
cargo test --offline --package trajecta-met query::
cargo test --offline --package trajecta-core integrator::
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli
```

测试目标之后可以附加名称过滤器。模块路径变化后，先列出实际测试名称会更稳妥：

```text
cargo test --offline --package trajecta-core -- --list
```

## 文档环境

建议为文档建立独立 Python 环境，避免改变资料下载或分析环境中的软件包：

```text
python -m venv .venv-docs
```

激活环境后安装固定版本：

```text
python -m pip install --upgrade pip
python -m pip install -r requirements-docs.txt
```

严格构建站点前，先检查由源码生成的参考页：

```text
cargo build --locked --package trajecta-cli
python tools/generate_m5_1_reference.py --binary target/debug/trajecta-cli --check
python tools/validate_m5_1_docs.py --binary target/debug/trajecta-cli
mkdocs build --strict
```

Windows 上两个 `--binary` 参数改用 `target/debug/trajecta-cli.exe`。浏览器预览命令为：

```text
mkdocs serve
```

发布站点与开发预览站点使用不同的 MkDocs 配置。开发预览构建还会检查搜索引擎 `noindex`：

```text
mkdocs build --strict -f mkdocs.dev.yml -d site-dev
python tools/validate_m5_1_docs.py --site-dir site-dev --expect-noindex
```

## 独立目标目录

原生读取器重建与软件包矩阵适合使用单独的 Cargo 目标目录：

```text
CARGO_TARGET_DIR=target-native cargo build --offline --locked --release \
  --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

PowerShell 可在当前会话设置 `$env:CARGO_TARGET_DIR = "target-native"`。独立目录能够隔开
默认产物和原生读取器产物，也便于确认测试实际执行了哪个可执行文件。一般无需对整个工作区
运行 `cargo clean`；原生构建元数据失效时，移除对应的独立目标目录即可。
