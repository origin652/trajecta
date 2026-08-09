---
title: Trajecta 入门
description: 选择发布包或源码构建，校验归档，了解安装目录，并准备第一个 Trajecta 项目。
---

# 入门

Trajecta 以命令行程序的形式发布，安装包同时提供示例项目和离线恢复说明。气象资料使用
独立资产保存，更新程序时可以继续使用原有资料目录。初次使用可从随包的 CFSR 小型项目
开始，熟悉以后再沿用相同目录结构开展更长的模拟或切换资料系列。

本节从下载发布包或检出源码讲到获得可运行的可执行文件。随后可进入
[十五分钟快速入门](quickstart.md)，完整运行一次区域填充水汽追踪。

## 选择发布包或源码构建

| 路线 | 适用场景 | 获得的内容 |
|---|---|---|
| 发布包 | 直接运行 Trajecta，或跟随用户教程 | 可执行文件、原生读取器运行库、示例项目、资料工具、离线说明、许可证清单和构建清单 |
| 源码构建 | 参与开发，或阅读实现 | 本机编译的 `trajecta-cli` 可执行文件和完整 Rust 工作区 |

发布包是开始首个案例最直接的方式，同一个可执行文件中包含 Rust 读取器和随包的
原生读取器。源码路线会编译快速入门采用的 Rust 读取器，具体命令见
[构建步骤](quickstart.md#1)。

## 已发布的平台包

版本 `0.1.0-alpha.1` 提供以下目标：

| 平台 | 架构 | 归档格式 | 产品可执行文件 | 读取器选择 |
|---|---|---|---|---|
| Windows | x86_64 | ZIP | `trajecta.exe` | 纯 Rust 或随包原生读取器 |
| Ubuntu 24.04 | x86_64 | `tar.gz` | `trajecta` | 纯 Rust 或随包原生读取器 |

本文中的命令和全新解压软件包测试均使用这两个平台。源码也可在具备 Rust 1.85
工具链的兼容系统上构建，本机构建产物沿用 `trajecta-cli` 这一文件名。

读取器在运行配置中选择。`rust` 使用 Rust 依赖提供的解码器；`native` 调用发布包
中的 ecCodes 或 netCDF-C 运行库。两条读取路径都会生成轨迹引擎使用的统一气象字段。
[平台与读取器矩阵](../reference/platforms.md)列出了各数据格式可用的读取方式。

## 安装发布包

从 [0.1.0-alpha.1 发布页](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1)
下载对应平台的归档和相邻的 `.sha256` 文件，并将二者放在同一下载目录。

### 校验归档

解压前计算 SHA-256：

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -Algorithm SHA256
    Get-Content .\trajecta-0.1.0-alpha.1-windows-x86_64.zip.sha256
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    cat trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz.sha256
    ```

计算得到的 64 位十六进制值应与校验文件一致。这样可以在打开归档前发现下载中断或
文件内容变化。

!!! tip "解压前校验"

    归档和校验文件放在同一目录。发现不一致时，直接重新下载会更省事。

### 解压并打开安装目录

=== "Windows PowerShell"

    ```powershell
    Expand-Archive `
      .\trajecta-0.1.0-alpha.1-windows-x86_64.zip `
      -DestinationPath .
    Set-Location .\trajecta-0.1.0-alpha.1-windows-x86_64
    .\trajecta.exe --help
    ```

=== "Ubuntu 24.04"

    ```bash
    tar -xzf trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    cd trajecta-0.1.0-alpha.1-linux-x86_64
    ./trajecta --help
    ```

帮助页首先显示命令概要，随后列出 `config`、`project`、`run`、`job` 和 `result`
等命令组。后续入门命令从这个解压目录运行，文中的 `examples/` 与 `tools/` 路径即可
直接使用。

## 安装包中的内容

| 路径 | 用途 |
|---|---|
| `trajecta.exe` 或 `trajecta` | 命令行程序，同时提供本地守护进程与工作进程入口 |
| `examples/` | 完整的区域填充、释放型粒子、气团和臭氧示例项目 |
| `tools/` | 资料请求助手和准备工具 |
| `docs/quickstart/` | 可离线阅读的中英文快速入门、支持矩阵和恢复说明 |
| `requirements-data.txt` | ERA5 资料准备工具使用的 Python 包 |
| `BUILD-MANIFEST.json` | 构建来源标识及安装包内每个文件的散列 |
| `SBOM.cdx.json` | 软件组件清单 |
| `THIRD-PARTY-LICENSES.json` | 第三方许可清单 |
| `LICENSE` | Trajecta 的 MIT 许可证正文 |

示例目录已经包含项目文档和空的资料目录。CFSR 演示资料包用于前两个教程；ERA5 示例则
沿用后续指南中的资料计划（`data-plan`）和下载流程。

## 准备第一个项目

第一次运行可以分为四个阶段：

1. 创建本机配置，填写本地任务可用的 CPU 与内存。
2. 将 CFSR 演示文件放入随包的区域填充项目。
3. 执行项目定稿命令 `project finalize`，将案例、运行配置与本地资料绑定起来。
4. 选择运行配置并启动任务，随后验证结果并读取一条粒子轨迹。

[本机配置与环境检查](configuration.md)说明本机资源文件的设置方法，
[演示资料](demo-data.md)列出每个 CFSR 文件及其校验值。快速入门会把两部分连成一次
完整运行。

## 后续阅读

| 目标 | 下一页 |
|---|---|
| 从源码构建二进制 | [快速入门：获得可执行文件](quickstart.md#1) |
| 完成首个区域填充运行 | [十五分钟快速入门](quickstart.md) |
| 了解本机 CPU 和内存设置 | [本机配置与环境检查](configuration.md) |
| 下载并检查示例文件 | [CFSR 演示资料](demo-data.md) |
| 准备更完整的科学工作流 | [教程](../tutorials/index.md) |
