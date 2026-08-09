---
title: 十五分钟 domain-fill 快速入门
description: 使用四帧 CFSR 演示资料运行并完整验证一个小型 domain-fill 水汽案例。
---

# 十五分钟 domain-fill 快速入门

本流程使用 CFSR 全球压力层资料运行一个小型水汽追踪案例。Trajecta 在研究区域内
生成 1,000 个气团粒子，向前积分十分钟，并在起止时刻保存粒子状态。后续步骤会完成
项目校验、资料锁定、任务运行和结果读取。大型研究使用相同的 Case、RunProfile、
DatasetLock、任务队列和结果目录结构。

四帧演示资料约 22 MiB。使用已经构建好的发布包时，从解压到完成 full verify 通常可在
十五分钟内完成。第一次从源码构建会花费更多时间，主要耗时来自 Cargo 编译依赖。

## 1. 获得可执行文件

可以从 [GitHub Releases](https://github.com/origin652/trajecta/releases) 下载对应平台的
发布包，也可以直接从源码构建。下面的依赖表适用于本页使用的纯 Rust reader 构建。

| 构建项 | 本快速入门使用的配置 |
|---|---|
| Rust | `1.85.0`；项目最低版本为 `1.85` |
| Windows 编译工具 | 64 位 MSVC C/C++ Build Tools |
| Ubuntu 编译工具 | 常规 C/C++ build toolchain |
| ecCodes | 本案例选择 Rust reader；发布包内的 native reader 使用 `2.47.0` |
| netCDF-C | 本案例选择 Rust reader；发布包内的 native reader 使用 `4.9.3` |
| HDF5 | 本案例选择 Rust reader；发布包内的 native reader 使用 `1.14.6` |
| Python | `3.11` 或更高版本，用于资料下载助手 |

从源码构建 Release 二进制：

=== "Windows PowerShell"

    ```powershell
    git clone https://github.com/origin652/trajecta.git
    Set-Location .\trajecta\trajecta

    rustup toolchain install 1.85.0 --profile minimal
    rustup override set 1.85.0
    cargo build --locked --release -p trajecta-cli

    .\target\release\trajecta-cli.exe --help
    ```

=== "Ubuntu 24.04"

    ```bash
    git clone https://github.com/origin652/trajecta.git
    cd trajecta/trajecta

    rustup toolchain install 1.85.0 --profile minimal
    rustup override set 1.85.0
    cargo build --locked --release -p trajecta-cli

    ./target/release/trajecta-cli --help
    ```

`cargo build` 完成后，可执行文件位于：

| 平台 | 源码构建产物 | 发布包中的文件名 |
|---|---|---|
| Windows x86_64 | `target/release/trajecta-cli.exe` | `trajecta.exe` |
| Ubuntu 24.04 x86_64 | `target/release/trajecta-cli` | `trajecta` |

后面的命令统一写作 `trajecta`。在当前终端中为二进制定义这个短名称：

=== "Windows 发布包"

    ```powershell
    $TrajectaBinary = (Resolve-Path .\trajecta.exe).Path
    function trajecta { & $TrajectaBinary @args }
    ```

=== "Windows 源码构建"

    ```powershell
    $TrajectaBinary = (Resolve-Path .\target\release\trajecta-cli.exe).Path
    $env:TRAJECTA_BIN = $TrajectaBinary
    function trajecta { & $TrajectaBinary @args }
    ```

=== "Ubuntu 发布包"

    ```bash
    TRAJECTA_BINARY="$(pwd)/trajecta"
    trajecta() { "$TRAJECTA_BINARY" "$@"; }
    ```

=== "Ubuntu 源码构建"

    ```bash
    TRAJECTA_BINARY="$(pwd)/target/release/trajecta-cli"
    export TRAJECTA_BIN="$TRAJECTA_BINARY"
    trajecta() { "$TRAJECTA_BINARY" "$@"; }
    ```

源码构建路线的后续命令从仓库内层的 `trajecta` 目录运行；发布包路线则从解压目录运行。
这两个目录都包含本例需要的 `examples/` 和 `tools/`。

## 2. 下载并校验资料

下载
[`trajecta-demo-cfsr-20090101-v1.zip`](https://github.com/origin652/trajecta/releases/download/v0.1.0-alpha.1/trajecta-demo-cfsr-20090101-v1.zip)。
冻结的归档 SHA-256 为：

```text
cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8
```

校验归档后解压，并将四个文件复制到示例项目的数据目录。

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-demo-cfsr-20090101-v1.zip -Algorithm SHA256
    Expand-Archive .\trajecta-demo-cfsr-20090101-v1.zip -DestinationPath .\demo-data
    Copy-Item .\demo-data\trajecta-demo-cfsr-20090101-v1\data\* `
      .\examples\domain-fill-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-demo-cfsr-20090101-v1.zip
    unzip trajecta-demo-cfsr-20090101-v1.zip -d demo-data
    cp demo-data/trajecta-demo-cfsr-20090101-v1/data/* \
      examples/domain-fill-cfsr/data/
    ```

各文件身份见[演示资料](demo-data.md)。

解压后的资料包含 2009 年 1 月 1 日 00、06、12 和 18 UTC 四个时次。示例只积分
06:00 至 06:10 这十分钟，四帧文件共同构成该 dataset profile 使用的时间范围。

## 3. 了解示例项目

示例位于 `examples/domain-fill-cfsr`。项目索引中包含一个名为 `moisture` 的 Case，
以及一个名为 `product` 的 RunProfile。

| 设置 | 本例取值 |
|---|---|
| 模拟时段 | 2009-01-01 06:00:00 至 06:10:00 UTC |
| 方向 | 向前 |
| 粒子总体 | `domain_fill_air_mass`，1,000 个粒子 |
| 气象资料 | 逻辑数据集 `cfsr`，全球周期边界 |
| 积分器 | `rk2_spherical/v0`，步长 300 秒 |
| 垂直边界 | 地表反射，模式顶终止 |
| 输出 | 起止时刻的粒子状态，写入 SQLite |
| 随机种子 | 4202 |

模拟时段、粒子总体和数值设置位于 `cases/moisture.yaml`。本机路径及资源需求位于
`profiles/product.yaml`。项目索引把逻辑数据集 `cfsr` 绑定到
`cfsr-pgbl-pressure-v0`，运行时再通过 DatasetLock 确定实际使用的四个文件。

## 4. 创建本机配置

以下命令均在安装包根目录运行。

```text
trajecta --config quickstart.toml config init
trajecta --config quickstart.toml config set resources.cpu_slots 1
trajecta --config quickstart.toml config set resources.memory_reserve_mib 256
trajecta --config quickstart.toml config set resources.memory_pool_mib 1536
trajecta --config quickstart.toml config validate
```

`cpu_slots` 是本机可供调度器分配的 CPU 槽位。`memory_pool_mib` 设置 Trajecta worker
可使用的内存池，`memory_reserve_mib` 为操作系统、daemon 和同机程序预留空间。示例
Profile 申请一个 worker 线程和 1 GiB 内存，上述配置可同时接纳一个示例任务。

`quickstart.toml` 是本机配置，可以供同一台机器上的多个项目复用。Case 和 Profile
仍保存在各自的项目目录中。

## 5. 校验项目并生成 data-plan

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --format json --project examples/domain-fill-cfsr project data-plan --output data-plan.json
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

下载助手默认展示资料请求和目标路径。本例中，四帧文件复制完成后均显示为已存在。
DatasetLock 在下一步 finalization 时生成。

data-plan 中的这项需求应包含以下主要字段：

```json
{
  "case_name": "moisture",
  "profile_name": "product",
  "dataset_id": "cfsr",
  "dataset_profile": "cfsr-pgbl-pressure-v0",
  "reader_backend": "rust",
  "required_capabilities": [
    "domain_fill",
    "near_surface_transport",
    "transport"
  ],
  "status": "partial"
}
```

`partial` 表示项目文档已经解析完成，资料 lock 还在等待 finalization。下载助手读取
data-plan 后会逐一显示目标文件及其本地状态。

## 6. Finalize 并检查环境

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr project finalize
trajecta --config quickstart.toml --project examples/domain-fill-cfsr doctor --deep
```

`project finalize` 读取本地文件，为 `examples/domain-fill-cfsr/locks/cfsr.lock.json`
写入文件 SHA-256、有效时次、网格签名、压力层签名、dataset profile 和读取能力。
Case、Profile 与资料内容由此绑定为一次可运行的项目配置。

`doctor --deep` 检查所选本机配置、项目资料、DatasetLock 和结果文件系统，并执行一次
SQLite WAL 创建与 checkpoint 循环。命令结束时会按阶段列出检查结果；全部通过后即可
提交任务。

## 7. 前台运行

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr run --profile product
```

命令默认前台等待，终端会持续显示任务状态。输出中的 `job_series_id` 标识整个任务，
`run_id` 标识本次 attempt，`output_directory` 给出结果目录。任务接收完成后，本地 daemon
负责调度，worker 执行数值计算并写入结果。

成功运行的结果路径采用以下结构：

```text
examples/domain-fill-cfsr/runs/domain-fill-cfsr/RUN_ID/
```

需要让终端在任务接收后立即返回时，可以在后续项目中使用 `--detach`。后台任务可通过
`job events JOB_ID --follow` 连续查看事件，也可通过 `job wait JOB_ID` 等待终态。

## 8. 验证并读取结果

将 `RESULT` 替换为运行目录或 job-series 身份。

```text
trajecta --config quickstart.toml result verify RESULT --full
trajecta --config quickstart.toml result inspect RESULT
trajecta --config quickstart.toml result trajectory RESULT --particle-id 0
trajecta --config quickstart.toml run report --result RESULT
```

Full verify 成功后，结果状态为 `complete`。`result inspect` 会汇总 1,000 个粒子、
2 个输出时次、2,000 条粒子状态记录和异常终止计数。每次运行会产生新的 run ID。

结果目录中的主要文件如下：

| 文件 | 内容 |
|---|---|
| `run-manifest.json` | 运行状态、输入、数值设置、质量统计和文件身份 |
| `particles.sqlite` | 粒子、状态、质量收支、事件和终止记录 |
| `particles.sqlite-wal` | SQLite 写前日志；成功收尾后大小为零 |
| `resolved-case.json` | worker 实际执行的 Case |
| `resolved-run-profile.json` | 本次运行使用的本机绑定 |
| `provenance-bundle.json` | 输出记录关联的气象记录与变换过程 |
| `run-report.md` | `run report` 生成的可读摘要 |

`result trajectory` 会输出粒子 0 在起止时刻的两条记录。经纬度、气压、高度和时间展示
了该粒子在十分钟内的移动。读取更多粒子时，可以选择 JSONL 格式并把输出重定向到分析
文件。

## 完成检查

- Manifest 状态为 `complete`。
- 异常粒子数为零。
- Full verify 成功。
- `particles.sqlite` 通过完整性与行数检查。
- `provenance-bundle.json` 与 manifest 身份一致。

任一项目失败时，请保留结果目录并查阅[故障索引](../operations/troubleshooting.md)。

至此，一个项目已经完成从配置、资料锁定和任务执行到结果读取的完整流程。下一步可进入
[domain-fill 水汽追踪教程](../tutorials/domain-fill.md)，延长模拟时段并调整粒子数量与
输出间隔。
