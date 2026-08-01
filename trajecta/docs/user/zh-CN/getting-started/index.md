---
title: Trajecta 入门
description: 说明支持平台、归档校验、安装方法，以及获得首个已验证结果的路径。
---

# 入门

正式安装包能够独立支持日常命令行使用。CFSR 演示资料作为单独的 release asset 发布，
更新二进制时无需重复下载气象资料。

## 支持平台

| 平台 | 架构 | 归档格式 | Reader 支持 |
|---|---|---|---|
| Windows | x86_64 | ZIP | Rust reader 和随包 native reader |
| Ubuntu 24.04 | x86_64 | `tar.gz` | Rust reader 和随包 native reader |

贡献者可以尝试在其他系统构建。Alpha 阶段不对这些环境作发布支持承诺。

## 安装与校验

1. 从 [0.1.0-alpha.1 release](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1)
   下载平台归档。
2. 下载对应的 `.sha256` 文件。
3. 解压前计算归档摘要。

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -Algorithm SHA256
    Expand-Archive .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -DestinationPath .
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    tar -xzf trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    ```

请逐字符核对 64 位十六进制摘要。任一字符不符时，应停止解压和运行。

## 首个结果

接下来完成[十五分钟快速入门](quickstart.md)。该流程覆盖正式研究同样使用的配置、
finalize、运行和结果验证接口。
