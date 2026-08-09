---
title: 构建环境
description: 在 Windows 或 Ubuntu 上构建 Trajecta workspace 和完整原生 CLI，并运行本地贡献门禁。
---

# 构建环境

Trajecta 有两种常用构建形式。Cargo 默认构建适合日常 Rust 开发，完整构建会启用正式软件包使用的
ecCodes 与 netCDF-C reader。

本页命令都从仓库内的 `trajecta` 目录执行。

## 工具链

| 组件 | 项目基线 | 用途 |
| --- | --- | --- |
| Rust | `1.85.0` 或更高版本；edition 2024 | 五个 workspace crate 和 CLI binary |
| Cargo | 随 Rust 工具链安装 | 锁定依赖、构建、测试、clippy 和 rustdoc |
| Python | 3.11 或更高版本；CI 使用 3.12 | Validator、打包、资料助手和文档 |
| Git | 当前受支持版本 | 源码身份、软件包输入清单和 release worktree |
| C 工具链 | ABI 与 Rust target 一致 | 链接 ecCodes、netCDF-C 和 HDF5 |
| MkDocs 依赖 | `requirements-docs.txt` 中的固定版本 | 本地预览和严格文档构建 |

Workspace 声明了 `rust-version = "1.85"`。CI 会明确安装 1.85.0。日常使用较新工具链时，涉及
语言特性或标准库的修改仍需回到该基线检查。

```text
rustup toolchain install 1.85.0 --profile minimal
rustup override set 1.85.0
rustc --version
cargo --version
```

Windows 上的 package test 与 runtime test 路径较深，建议先启用 Git 长路径：

```text
git config --global core.longpaths true
```

## 构建 Rust CLI

有网络时先获取锁文件指定的依赖，随后构建 CLI：

```text
cargo fetch --locked
cargo build --locked --package trajecta-cli
```

开发 binary 位于：

| 主机 | Binary |
| --- | --- |
| Windows | `target/debug/trajecta-cli.exe` |
| Linux | `target/debug/trajecta-cli` |

确认 executable 可以启动，并检查 Cargo 选中的版本：

```text
target/debug/trajecta-cli --version
target/debug/trajecta-cli --help
```

PowerShell 使用 Windows 路径写法：

```text
.\target\debug\trajecta-cli.exe --version
.\target\debug\trajecta-cli.exe --help
```

优化后的纯 Rust binary 可用以下命令生成：

```text
cargo build --offline --locked --release --package trajecta-cli
```

Cargo 产物名为 `trajecta-cli`。确定性软件包构建器会将正式产品 executable 重命名为 Windows
上的 `trajecta.exe`，Linux 软件包中则使用 `trajecta`。

## 原生 reader 构建

正式产品启用的 feature set 为：

```text
trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

两个 feature 都需要显式启用。构建环境缺少原生库或 ABI 不匹配时，编译会直接失败；已经选择
的 reader 不会静默切换到另一种实现。

| 平台 | 原生开发环境 | 运行时条件 |
| --- | --- | --- |
| Ubuntu 24.04 x86_64 | `pkg-config`、ecCodes、netCDF-C 与 HDF5 的 header 和 library；重新生成 binding 时需要 libclang | 动态加载器能够找到链接的共享库；ecCodes 能找到匹配的 definition |
| Windows x86_64 GNU | MSYS2 UCRT64 GCC、`pkgconf`、UCRT64 netCDF-C/HDF5、相同 GNU ABI 的 ecCodes；重新生成 binding 时需要 libclang | UCRT64 与 ecCodes DLL 目录保留在 `PATH`；`ECCODES_DEFINITION_PATH` 指向匹配 definition |

Ubuntu 可先安装发行版提供的开发包：

```text
sudo apt-get update
sudo apt-get install pkg-config libeccodes-dev libnetcdf-dev libhdf5-dev libclang-dev
```

经过验证的 Windows GNU 环境使用 MSYS2 UCRT64 提供 netCDF-C、HDF5 和 `pkgconf`：

```text
pacman -S --needed mingw-w64-ucrt-x86_64-netcdf mingw-w64-ucrt-x86_64-pkgconf
```

Windows 环境中的路径应来自同一个 ABI family。启动 Cargo 前，典型的 UCRT64 配置包含：

```text
PATH=<ucrt64-bin>;<eccodes-bin>;%PATH%
NETCDF_DIR=<ucrt64-root>
PKG_CONFIG_PATH=<ucrt64-lib-pkgconfig>;<eccodes-lib-pkgconfig>
ECCODES_DEFINITION_PATH=<eccodes-share-definitions>
LIBCLANG_PATH=<directory-containing-libclang>
```

!!! warning "Windows 使用一套 ABI"

    `x86_64-pc-windows-gnu` Rust 应与 UCRT64 原生库配套。MSVC import library 属于另一套
    toolchain。

开始构建前，可同时确认 Rust host 和 C library：

```text
rustc -vV
pkg-config --modversion netcdf
pkg-config --modversion eccodes
```

使用与正式软件包相同的 feature 构建：

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
| Case、Profile、单位或诊断 | 默认 feature 与目标 crate test | Workspace test 和 schema/reference 检查 |
| 纯 Rust query 或数值代码 | 默认 feature | 相关真实资料 replay 与确定性输出测试 |
| GRIB reader 或 ecCodes metadata | `native-eccodes` test | Native differential test 与 packaged smoke |
| NetCDF reader、hybrid level 或 HDF5 access | `native-netcdf` test | Formal target 上的 native differential test |
| 原生加载或打包 | 完整 feature build | Windows 与 Ubuntu 的全新解压 probe |
| CLI rendering 或 job control | 默认 CLI build | Runtime contract 与机器输出 test |

默认 feature 适合编辑期间快速检查。修改涉及文件解释或软件包加载时，还要运行对应的原生 reader
门禁。

## Workspace 门禁

依赖进入本地 cache 后，主要源码门禁可以离线运行：

```text
cargo fmt --all -- --check
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps --workspace
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
git diff --check
```

Workspace lint 会拒绝公开 Rust item 缺少文档、unsafe code 和未处理的 `must_use` 结果。生产
target 中的 `panic!`、`todo!` 与 `unimplemented!` 同样无法通过，`unwrap` 和 `expect` 也受限制。
Test module 会在 fixture 准备确有需要时使用局部 allowance。

编辑过程中可先运行 package 级命令：

```text
cargo test --offline --package trajecta-met query::
cargo test --offline --package trajecta-core integrator::
cargo test --offline --package trajecta-job
cargo test --offline --package trajecta-cli
```

Test target 之后可以附加名称过滤器。Module 路径变化后，先列出实际 test name 会更稳妥：

```text
cargo test --offline --package trajecta-core -- --list
```

## 文档环境

建议为文档建立独立 Python 环境，避免改变资料下载或分析环境中的 package：

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

Release 与 dev 站点使用不同的 MkDocs 配置。Dev build 还会检查搜索引擎 `noindex`：

```text
mkdocs build --strict -f mkdocs.dev.yml -d site-dev
python tools/validate_m5_1_docs.py --site-dir site-dev --expect-noindex
```

## 独立 target 目录

Native rebuild 与软件包矩阵适合使用单独的 Cargo target：

```text
CARGO_TARGET_DIR=target-native cargo build --offline --locked --release \
  --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

PowerShell 可在当前 session 设置 `$env:CARGO_TARGET_DIR = "target-native"`。独立目录能够隔开
默认产物和 native artifact，也便于确认测试实际执行了哪个 binary。一般无需对整个 workspace
运行 `cargo clean`；native build metadata 失效时，移除对应的独立 target 目录即可。
