---
title: 构建环境
description: Trajecta 贡献者可复现的 Rust、Python、native reader、文档和本地门禁环境。
---

# 构建环境

## 必需工具

- Rust `1.85` 或更高版本与 Cargo；workspace 使用 edition 2024。
- Python `3.11` 或更高版本，用于 validator、打包、证据工具和文档。
- Windows 上启用长路径支持的 Git。
- 文档工作使用 `requirements-docs.txt` 固定的 MkDocs 依赖。

依赖与 native 组件准备完成后，仓库支持离线 Cargo 门禁：

```text
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
cargo fmt --all -- --check
python tools/validate_m4_a0_contracts.py
python tools/validate_m5_a0_contracts.py
git diff --check
```

## Native reader

产品构建启用 `trajecta-met/native-eccodes` 与 `trajecta-met/native-netcdf`。正式安装包包含
ecCodes、netCDF-C、HDF5 和所需 ecCodes definitions。启用 feature 后如果 native 运行时
缺失，构建或执行必须失败，不能静默切换 reader。

常规开发可先运行不带 native feature 的纯 Rust 测试。修改文件解释、元数据、native
加载或打包时，需要覆盖两条 reader 路径。

## 文档环境

建立独立 Python 环境，安装固定依赖，并从 `trajecta` 目录构建：

```text
python -m pip install -r requirements-docs.txt
mkdocs build --strict
mkdocs serve
```

发布站点在 CI 中使用相同命令。
