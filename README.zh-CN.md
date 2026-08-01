# Trajecta

**开源的拉格朗日水汽追踪与大气轨迹框架**

[English](README.md)

[![Release](https://img.shields.io/github/v/release/origin652/trajecta?include_prereleases&sort=semver)](https://github.com/origin652/trajecta/releases)
[![中文文档](https://img.shields.io/badge/docs-简体中文-4051b5)](https://origin652.github.io/trajecta/zh-CN/)
[![M5.1 docs CI](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml/badge.svg)](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Trajecta 用于水汽源区分析和大气轨迹研究，可执行正向或反向拉格朗日模拟。
科学 Case 与执行 Profile 共同定义一次研究。气象资料经过核验后进入运行，结果以
不可变产物保存。
Windows 与 Linux 使用同一套命令行工作流。

> **Alpha 状态：** 当前版本为 `0.1.0-alpha.1`。受支持的产品边界覆盖 CLI 与配置文档。
> 公开 JSON Schema 和磁盘结果产物也属于产品合同。Alpha 阶段的 Rust crate API 主要供
> 仓库贡献者使用。

## 主要能力

- 通过 domain filling 开展水汽源区研究
- 运行常规粒子释放实验，并选择正向或反向积分
- 支持 air-mass 与平流层臭氧 population 工作流
- 读取 CFSR pressure 与两类 ERA5 资料（pressure 和 hybrid）
- 在资料尚未齐备时先建立项目，随后生成 data plan 并显式 finalize
- 默认前台运行，也可使用本地 daemon 和持久任务队列
- 在中断后保留既有 attempt，已完成任务无需重跑
- 将轨迹结果写入 SQLite，同时生成 run manifest 和 provenance bundle
- 为终端用户提供 human 输出，为自动化提供稳定 JSON 或 JSONL envelope

当前 Alpha 版本没有插件加载机制，也没有通用结果导出器。文档只保留扩展边界，
不会提供尚未实现的命令或配置示例。

## 快速开始

1. 从 [GitHub Releases](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1)
   下载对应平台的软件包。
2. 解压前使用同名 SHA-256 文件核验归档。
3. 按快速入门说明取得独立发布的四帧 CFSR 演示资料。
4. 运行 domain-fill 示例并核验结果。

快速入门按三个阶段组织：

| 阶段 | 内容 |
| --- | --- |
| 准备 | 配置与资料检查，然后执行项目 finalize |
| 执行 | `doctor --deep` 与前台运行 |
| 核验 | full verify，随后检查结果并读取单粒子轨迹 |

从解压软件到得到通过核验的结果，验收目标为 15 分钟以内。

- [15 分钟快速入门](https://origin652.github.io/trajecta/zh-CN/getting-started/quickstart/)
- [English quickstart](https://origin652.github.io/trajecta/getting-started/quickstart/)

运行任务时，Trajecta 不会自行下载气象资料。资料助手默认只展示请求；加入
`--execute` 后才会写入 data plan 声明的资料目录。

```text
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

## 文档

| 内容 | 简体中文 | English |
| --- | --- | --- |
| 首次运行 | [入门](https://origin652.github.io/trajecta/zh-CN/getting-started/) | [Getting Started](https://origin652.github.io/trajecta/getting-started/) |
| 科研工作流 | [教程](https://origin652.github.io/trajecta/zh-CN/tutorials/) | [Tutorials](https://origin652.github.io/trajecta/tutorials/) |
| 任务操作 | [操作指南](https://origin652.github.io/trajecta/zh-CN/how-to/) | [How-to Guides](https://origin652.github.io/trajecta/how-to/) |
| 模型概念 | [概念](https://origin652.github.io/trajecta/zh-CN/concepts/) | [Concepts](https://origin652.github.io/trajecta/concepts/) |
| 恢复与维护 | [运行维护](https://origin652.github.io/trajecta/zh-CN/operations/) | [Operations](https://origin652.github.io/trajecta/operations/) |
| 科学证据 | [验证](https://origin652.github.io/trajecta/zh-CN/validation/) | [Validation](https://origin652.github.io/trajecta/validation/) |
| 命令与格式 | [参考](https://origin652.github.io/trajecta/zh-CN/reference/) | [Reference](https://origin652.github.io/trajecta/reference/) |
| 仓库贡献 | [开发手册](https://origin652.github.io/trajecta/zh-CN/developer/) | [Developer Guide](https://origin652.github.io/trajecta/developer/) |

英文手册是规范源。正式发布要求两种语言的页面结构和内容范围保持同步。

## 支持平台

| 平台 | 架构 | 软件包状态 |
| --- | --- | --- |
| Windows | x86_64 | 支持的预发布软件包 |
| Ubuntu 24.04 | x86_64 | 支持的预发布软件包 |

每个平台包内含可执行程序和 native runtime。四个示例项目随包提供，资料助手与
精简的双语离线手册也放在归档内。科学输入资料独立发布，不重复放入软件归档。

## 可复现性与验证

每个完成的运行都会记录 resolved Case 与 Profile identity，并保存输入内容散列。
lifecycle 统计与数值质量检查随结果保存，输出 identity 也进入证据链。Validation 手册
给出科学比较方法，并保留冻结的 CSV 或 JSON 证据。站内图表可由这些原始数据重新生成。

结果目录属于不可变科研记录。常规读取优先使用产品命令：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id PARTICLE_ID
trajecta run report --result RESULT
```

SQLite 只读结构见高级参考页面。

## 仓库布局

```text
.
├── trajecta/
│   ├── crates/           Rust workspace
│   ├── docs/user/        双语用户手册源文件
│   ├── docs/engineering/ 工程计划与执行记录
│   ├── examples/         可执行教程项目
│   ├── packaging/        软件包内离线资料
│   ├── testdata/         Schema 与冻结验证合同
│   └── tools/            项目工具与资料助手
└── .github/workflows/    CI 和发布工作流
```

工程记录不会进入用户站导航，也不会进入站内搜索和 sitemap。

## 从源码构建

科研用户应优先使用发布软件包。仓库贡献者可以直接构建 Rust workspace：

```text
cd trajecta
cargo build --offline --workspace
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
```

双语手册使用 MkDocs Material：

```text
cd trajecta
python -m pip install -r requirements-docs.txt
mkdocs build --strict
```

更多构建细节见
[开发手册](https://origin652.github.io/trajecta/zh-CN/developer/)。

## 参与项目

仓库接受可复现的缺陷报告与文档修订，也接受范围清晰的 Pull Request。报告运行问题时，
请注明平台并附上完整命令。结构化 diagnostic code 与保留的 attempt 产物也需要随报告提交。涉及科学行为的
改动需要提供可执行回归，并说明数值影响。

- [提交 Issue](https://github.com/origin652/trajecta/issues/new)
- [查看现有 Issue](https://github.com/origin652/trajecta/issues)

## 许可证

Trajecta 使用 [MIT License](LICENSE)。
