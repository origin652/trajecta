---
title: 打包与发布
description: 确定性产品 archive、原生运行库清单、供应链文件、文档快照和 GitHub release 流程。
---

# 打包与发布

Trajecta 为 Windows x86_64 和 Ubuntu 24.04 x86_64 发布 host-native archive。Archive 由 release
build 与声明的 payload 组装；解压后运行无需 Rust 或 C 开发环境。

软件包、演示资料和文档是三个独立 release artifact。这样可以避免在两个平台 archive 内重复存放
气象 sample，手册也可以作为带版本的静态站点发布。

## Release artifact 集合

`0.1.0-alpha.1` 预期包含以下公开文件：

| Artifact | 用途 |
| --- | --- |
| `trajecta-0.1.0-alpha.1-windows-x86_64.zip` | Windows GNU-hosted 产品软件包 |
| `trajecta-0.1.0-alpha.1-windows-x86_64.zip.sha256` | Archive checksum |
| `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz` | Ubuntu 24.04 产品软件包 |
| `trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz.sha256` | Archive checksum |
| `trajecta-demo-cfsr-20090101-v1.zip` | 包含四个文件的 CFSR 演示资料 |
| 演示资料 checksum 与 build result | Dataset archive identity |
| GitHub release note | 面向使用者的范围、下载入口、变更和限制 |
| `0.1.0-alpha.1` 文档快照 | GitHub Pages 上不可变的双语手册 |

Validation CSV、JSON 和 chart 位于文档源码与发布站点。它们由冻结 comparison output 生成，不会
复制到 executable archive。

## 软件包内容

`tools/m5_a4_package.py` 会建立以版本和平台命名的单一根目录。Payload 包含：

| 分组 | 文件 |
| --- | --- |
| Executable | Windows 的 `trajecta.exe` 或 Linux 的 `trajecta` |
| 原生运行库 | 非系统 ecCodes、netCDF-C、HDF5 和传递依赖共享库 |
| ecCodes data | 复制的 definition tree，或经过验证的 embedded-MEMFS marker |
| 供应链 | `BUILD-MANIFEST.json`、CycloneDX 1.5 `SBOM.cdx.json`、`THIRD-PARTY-LICENSES.json` |
| 法律与软件包信息 | 项目 `LICENSE` 和 package `README.md` |
| 离线帮助 | 中英文快速入门、恢复说明和支持矩阵 |
| Example project | Domain-fill CFSR、release CFSR、ERA5 pressure air mass、ERA5 hybrid ozone |
| 资料工具 | Data-plan helper、CFSR/ERA5 获取与准备工具、quickstart runner |
| Python requirements | 可选 provider helper 使用的 `requirements-data.txt` |

软件 archive 不包含气象资料和 provider credential。使用者可以另外下载演示 asset，或者准备自己
的 locked dataset。

## Formal build 的主机条件

正式软件包在目标操作系统上原生构建：

| 软件包 | Rust host | Build host |
| --- | --- | --- |
| Windows | `x86_64-pc-windows-gnu` | Windows x64 与同一 GNU ABI 的 native library |
| Linux | `x86_64-unknown-linux-gnu` | Ubuntu 24.04 x86_64 |

Package tool 会检查 Rust host triple。Ubuntu 24.04 以外的 Linux build 只有在显式使用 nonformal
preflight flag 时才可生成，不能作为 release archive。

Release binary 使用以下 feature set：

```text
cargo build --offline --locked --release --package trajecta-cli \
  --features trajecta-met/native-eccodes,trajecta-met/native-netcdf
```

Linux 软件包会加入 `$ORIGIN/lib` runtime search path。Windows builder 递归遍历 executable 的 DLL
import，并从指定 native-library directory 复制非系统 dependency。

## 确定性 archive 构造

编译前，package builder 先记录 source identity，其中包含 Git commit、tracked 与 unignored source
file 的整体 digest、file count 和 dirty path。Release review 使用明确选择的 source tree；生成的
target directory 与无关 run artifact 不属于 package input。

Timestamp 来自 `SOURCE_DATE_EPOCH`，默认使用 commit time。Archive writer 随后规范化：

- 使用正斜杠表示相对 member path；
- Root directory name；
- Member ordering；
- File 与 directory mode；
- Tar archive 中的 owner information；
- ZIP 与 gzip timestamp；
- Gzip filename metadata；
- JSON key order 和最终 line ending。

!!! tip "每次构建使用新的 output root"

    Builder 不会覆盖已有 stage、archive 或 checksum。每次 build 使用单独目录。

## 原生库收集与 probe

Archive 的 component inventory 必须各声明一次 ecCodes、netCDF-C 和 HDF5。Builder 会加载复制后
的运行库，并调用 version API：

| Component | Probe |
| --- | --- |
| ecCodes | `codes_get_api_version` |
| netCDF-C | `nc_inq_libvers` |
| HDF5 | `H5get_libversion` |

探测版本必须与 `--native-component` 提供的版本一致。这样可以发现 release metadata 描述的库和
软件包实际收集的 DLL 或 shared object 不一致。

Windows system DLL 由操作系统提供。Linux 会跳过系统 C runtime 与 loader，将其余已解析依赖复制
到 `lib/`。出现 unresolved `ldd` entry 或递归 DLL import 缺失时，build 会停止。

## Build manifest 与供应链清单

`BUILD-MANIFEST.json` 是软件包索引，记录：

- Product version、target triple 与 minimum operating system。
- Rust/Cargo version、feature set、source date 与 source-tree identity。
- Binary path、byte count 和 SHA-256。
- Archive format 与 root directory。
- 每个 payload path 的 role、size 和 SHA-256。
- Native component 与 version probe result。
- SBOM 和 license-inventory identity。

CycloneDX 文档根据锁定的 Cargo dependency closure 和三个 native component 生成。License
inventory 使用相同 component set，并记录 source link 与 license expression。Package verify 会
交叉检查这些文件，不会将它们视为互不相关的附件。

## 软件包验证

Verify 从 archive 和相邻 checksum 开始：

```text
python tools/m5_a4_package.py verify \
  --archive <archive> \
  --extract-root <new-directory> \
  --result <verification-result.json>
```

Verifier 按以下顺序工作：

1. 检查 archive SHA-256 和 checksum filename。
2. 拒绝 absolute path、parent traversal、duplicate member、link 和异常 archive root。
3. 解压到新目录，不覆盖现有 tree。
4. 验证 manifest schema 与 product/platform identity。
5. 重新计算每个 payload digest，并拒绝未列入清单的文件。
6. 检查 executable name，以及 Linux execute permission。
7. 验证 CycloneDX 与 license inventory。
8. 加载解压后的 native library，重复 version probe。

输出 JSON 会记录后续 smoke test 使用的精确 extracted root 和 identity。Matrix runner 应读取该
root，不应自行猜测 archive directory name。

## 演示资料 asset

`tools/build_m5_1_demo_asset.py` 构建单独发布的 CFSR sample。Input manifest 固定四个 GRIB filename、
byte count、SHA-256、source information 和 data-use note。ZIP 使用固定 root 与 timestamp。

```text
python tools/build_m5_1_demo_asset.py \
  --source <directory-with-four-cfsr-files> \
  --output <release-directory>/trajecta-demo-cfsr-20090101-v1.zip
```

工具会写 archive、相邻 checksum 和 JSON build result。Quickstart runner 在解压后还会再次验证
四个 file hash。

## Release 前门禁

Release candidate 应在准备打 tag 的同一 source commit 上通过以下分组：

| 分组 | 要求结果 |
| --- | --- |
| Source | Workspace test、clippy warnings denied、rustdoc、fmt、M4/M5 contract validator |
| Documentation | Generated reference、bilingual validator、validation asset、严格 release/dev build |
| Package | 在全新目录 build 并 verify 两个 host-native archive |
| Product smoke | 两个平台各两个 clean-package CFSR cell |
| Product matrix | Windows 三十格，Ubuntu 三十格 |
| Tutorial | 两个平台的 domain-fill 与 release；Ubuntu 的 air-mass 与 ozone |
| Validation | 冻结 scientific/performance comparison aggregate 与 chart regeneration |
| Operations | 每轮 matrix 后没有遗留 live daemon/worker；失败 attempt 连同 summary 保留 |

Formal matrix 在首个失败处停止。实现发生修改后，应生成新的 source identity、package、extraction
root 和 attempt tree；失败 artifact 保持原样。

## GitHub workflow

M5.1 在 `.github/workflows` 下使用三个 workflow：

| Workflow | Trigger | 工作内容 |
| --- | --- | --- |
| `m5-1-docs-ci.yml` | 文档相关 pull request 或手动运行 | 构建 CLI 参考源，验证 helper 与文档，严格构建 release/dev site，运行 domain-fill source quickstart，检查 external link |
| `m5-1-docs-publish.yml` | Main 文档修改、release 或显式 dispatch | 构建 static site，上传 artifact，通过 `mike` 发布 mutable `dev` 或 immutable release docs |
| `m5-1-product-docs.yml` | 每周计划、release 或手动运行 | 下载公开软件包与 demo data，运行 packaged tutorial，检查公开 Pages 和 crawler asset |

Pull request 不发布 Pages。Main branch 的文档修改更新 `dev` 版本。Release event 或经过批准的手动
snapshot 会创建不可变 `0.1.0-alpha.1` 目录，并更新 `latest` alias。

## 文档发布

Release site 使用 `mkdocs.yml` 构建。Dev site 通过 `mkdocs.dev.yml` 继承配置，将 URL 改为 `/dev/`，
并生成 `noindex` metadata。发布前，两个 static tree 都会作为 workflow artifact 上传。

`mike` 在 `gh-pages` branch 保存带版本 HTML。创建 release snapshot 前，workflow 会确认对应版本
目录尚不存在。Root `robots.txt` 与 `sitemap.xml` 从 release build 生成；root sitemap 指向带版本
canonical URL，crawler 不访问 `/dev/`。

生成的 `site` 是普通静态内容。相同 artifact 可以由 GitHub Pages、Nginx 或 Caddy 提供，无需
Node 或 Python server。

## Credential 与发布权限

Release workflow 使用 GitHub short-lived token 下载 release 并更新 Pages branch。ERA5 tutorial
从 repository secret 读取 `CDSAPI_URL` 和 `CDSAPI_KEY`，只通过 process environment 传递。

Stage release file 前，需要扫描 source 与计划上传的 artifact，检查常见 credential pattern、本机
home path、provider configuration file 和 private key。Runtime log 与 provenance 可以记录 request
identity 和公开 dataset metadata，不应包含 authorization header 或 secret environment value。

## 发布顺序

1. 选择 commit，并确认 workspace version 为 `0.1.0-alpha.1`。
2. 运行 source、documentation、validation 和 product gate。
3. 在新 output root 中构建每个 host-native archive。
4. 将每个 archive verify 到新 extraction directory，再运行 package smoke 和 formal matrix。
5. 构建并验证 demonstration-data asset。
6. 一并审阅 `BUILD-MANIFEST.json`、SBOM、license inventory、checksum 和 release note。
7. 为已选 commit 创建 tag，发布软件与资料 asset。
8. 发布不可变双语文档 snapshot。
9. 在全新 browser session 打开公开下载、quickstart、带版本手册、sitemap、language switch 与 chart。
10. 按 workflow artifact retention 设置保留 summary 和 failed-attempt artifact。

Release 后的文档修订继续进入 `dev`。只有准备新的软件 release 时才修改软件版本。
