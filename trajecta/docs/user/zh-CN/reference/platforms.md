---
title: 平台与 reader 支持
description: Trajecta 0.1.0-alpha.1 正式验证的安装包平台、气象资料家族、reader、population 和方向覆盖。
---

# 平台与 reader 支持

## 产品安装包

| 平台 | 架构 | 安装包状态 | 本地控制传输 |
| --- | --- | --- | --- |
| Windows | x86_64 | 已正式验证 | 本地 named pipe |
| Ubuntu 24.04 LTS | x86_64 | 已正式验证 | 本地 Unix-domain socket |

其他 Linux 发行版可能可以运行兼容二进制，但不在本版本正式支持声明内。当前不声明
macOS 或非 x86_64 安装包支持。

## 气象资料 reader

| 资料家族 | Rust reader | Native reader | 正向与反向 | 产品矩阵范围 |
| --- | --- | --- | --- | --- |
| CFSR pressure | 支持且为默认值 | 已包含 | 均支持 | Rust 覆盖 release、air-mass、ozone；native 覆盖 release |
| ERA5 pressure | 支持且为默认值 | 已包含 | 均支持 | Rust 覆盖 release、air-mass、ozone；native 覆盖 release |
| ERA5 hybrid | 支持且为默认值 | 已包含 | 均支持 | Rust 覆盖 release、air-mass、ozone；native 覆盖 release |

Native 安装包运行时包含 ecCodes、netCDF-C 和 HDF5。`native` 表示 reader 选择，
不会更换数值核心。验证声明只覆盖上表单元与冻结 fixture 范围。每套本地资料仍需运行
`data inspect`、`project finalize` 和 `doctor --deep`。

并发结果 reader 使用带索引的 high-water snapshot。Trajecta 产品命令支持在写入期间
读取；第三方工具必须以只读方式打开 SQLite，并能处理 WAL snapshot。
