---
title: 平台与 reader 支持
description: 查找 Trajecta 0.1.0-alpha.1 release archive、操作系统、architecture、bundled native library、dataset family、reader、population 与方向。
---

# 平台与 reader 支持

Trajecta `0.1.0-alpha.1` 为两个 x86-64 platform 提供 self-contained command-line package。
Formal product matrix 在 source checkout 外运行 extracted archive，并覆盖 local control plane
与 scientific result command。

## Release package

| Host | Architecture | Archive | Binary | Local control transport |
| --- | --- | --- | --- | --- |
| Windows x64 | x86-64 | `trajecta-0.1.0-alpha.1-windows-x86_64.zip` | `trajecta.exe` | Named pipe |
| Ubuntu 24.04 LTS | x86-64 | `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz` | `trajecta` | Unix-domain socket |

每个 release 还会发布 `.sha256` file。Archive 中包含：

- Trajecta binary；
- 带有 source、binary、payload、platform 与 native component identity 的
  `BUILD-MANIFEST.json`；
- License 与 package README；
- 完整 example project；
- 双语 quickstart、support matrix 与 recovery note；
- 独立 Python data-fetch helper 及其 requirements file；
- Packaged reader 所需 native library 与 ecCodes definition。

Archive 解压后，packaged binary 可以放在任意目录。Project file 与 data 位于 program
directory 外部。

## Package 中的 native runtime

两个软件包在全新目录解压后都会探测原生库。各平台采用随构建环境提供的版本：

| 组件 | Windows x86_64 | Ubuntu 24.04 x86_64 | 用途 |
| --- | ---: | ---: | --- |
| ecCodes | 2.47.0 | 2.34.1 | 解码 GRIB 并加载定义文件 |
| netCDF-C | 4.9.3 | 4.9.2 | 为原生读取器提供 NetCDF 访问 |
| HDF5 | 1.14.6 | 1.10.10 | netCDF-C 使用的底层存储运行库 |

Windows 在 executable 旁分发所需 DLL set。Linux binary 通过 `$ORIGIN/lib` runtime path 加载
packaged library。ecCodes definition 作为 package data 提供，或由 build manifest 记录的
packaged memory-filesystem runtime 提供。

Release archive 用户不需要分别安装这三个 component。Source build 使用的 development
dependency 见[构建指南](../developer/build.md)。

!!! tip "发布包已包含原生运行库"

    只有从源码构建 Trajecta 时才需要安装 ecCodes、netCDF-C 和 HDF5 开发包。

## 气象资料 family

| Family | 常见 source | Vertical coordinate | Rust reader | Native reader | 方向 |
| --- | --- | --- | --- | --- | --- |
| CFSR pressure | NCEP CFSR pressure-level GRIB2 | Pressure level | Supported and default | Supported | Forward 与 backward |
| ERA5 pressure | ERA5 pressure-level GRIB 或 NetCDF preparation | Pressure level | Supported and default | Supported | Forward 与 backward |
| ERA5 hybrid | ERA5 model-level 与 surface field | 带 half-level coefficient 的 137 个 hybrid model level | Supported and default | Supported | Forward 与 backward |

Reader choice 控制 meteorological file access 与 query preparation。两个 reader 都向同一个
Trajecta numerical core 提供数据。RunProfile 可以设置 `execution.meteorology_reader`，一个
dataset binding 还可以提供更具体的 `reader_backend`。

`rust` reader 是 `config init` 创建的 default。`native` reader 使用上面的 bundled library。
本地 file 与 Profile 可以通过以下命令检查：

```text
trajecta data inspect FILE
trajecta --project PROJECT project finalize
trajecta --project PROJECT doctor --deep
```

## Formal product matrix

每个 supported platform 从 clean package 运行 30 个 formal cell：

| Group | 每个平台的 cell 数 | Coverage |
| --- | ---: | --- |
| Rust 1k | 18 | 三种 dataset family × 三种 population × 两个方向，1,000 粒子，一个 worker |
| Rust 10k | 6 | Selected dataset、population 与 direction combination，10,000 粒子，四个 worker |
| Native 1k | 6 | 三种 dataset family × release population × 两个方向，1,000 粒子，一个 worker |

三种 population 为 regular release、dry-air-mass domain filling 与 stratospheric-ozone domain
filling。每个 cell 会准备 project，finalize data binding，通过 daemon 运行到 terminal product，
验证 manifest 与 SQLite，读取 trajectory output，并重新生成 run report。

Formal matrix 前还有两个 clean-package smoke cell，分别覆盖 CFSR forward release 的 Rust
reader，以及 CFSR backward release 的 native reader。

## Result-reader 支持

Trajecta product command 支持 active result 的 read-only snapshot，并使用 writer 发布的
indexed high-water boundary。Complete attempt 还会完成 full verification 与 terminal SQLite
checkpoint，此时 WAL 为零或不存在。

Third-party SQLite program 可以在两个平台读取 public version 1 schema。Database 应以
read-only 打开；active writing 期间，WAL 与 SHM sidecar 保持在主库旁。[SQLite 参考](results-sqlite.md)
列出 query ordering 与 lifecycle rule。

## 其他系统

当前 release workflow 生成并正式运行 Windows x64 与 Ubuntu 24.04 x86-64 archive。其他
Linux distribution 可能具有兼容的 GNU C library 和 native runtime，但 `0.1.0-alpha.1` 没有
对应 release matrix。该版本不发布 macOS 和 ARM64 archive。

WSL2 中可以在 selected Linux distribution 内使用 Ubuntu 24.04 package。大容量 result 与
meteorological data 可以放在适合 SQLite 和 large sequential I/O 的 filesystem，随后在项目
位置运行 `doctor --deep`。
