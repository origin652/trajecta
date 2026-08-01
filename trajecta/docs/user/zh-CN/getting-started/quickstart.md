---
title: 十五分钟 domain-fill 快速入门
description: 使用四帧 CFSR 演示资料运行并完整验证一个小型 domain-fill 水汽案例。
---

# 十五分钟 domain-fill 快速入门

本流程在 CFSR 全球网格上运行 1,000 个气团粒子，模拟时长为十分钟。四帧源资料约
22 MiB。在受支持机器上，从已解压安装包到完成 full verify 的目标时间为十五分钟；
该时间不含网络下载。

## 1. 下载并校验资料

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

## 2. 选择本机配置

以下命令均在安装包根目录运行。

```text
trajecta --config quickstart.toml config init
trajecta --config quickstart.toml config set resources.cpu_slots 1
trajecta --config quickstart.toml config set resources.memory_reserve_mib 256
trajecta --config quickstart.toml config set resources.memory_pool_mib 1536
trajecta --config quickstart.toml config validate
```

## 3. 校验项目并生成 data-plan

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --format json --project examples/domain-fill-cfsr project data-plan --output data-plan.json
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

下载助手默认执行 dry-run。本例中它应报告四帧 CFSR 文件已存在。该助手不会创建
DatasetLock。

## 4. Finalize 并检查环境

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr project finalize
trajecta --config quickstart.toml --project examples/domain-fill-cfsr doctor --deep
```

`project finalize` 校验本地资料并显式写入 lock。Deep doctor 会检查所选配置、资料、
lock、文件系统，以及一次真实的 SQLite WAL 与 checkpoint 循环。

## 5. 前台运行

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr run --profile product
```

命令默认前台等待。请保存输出中的 `job_series_id`、`run_id` 和结果路径。任务被接收后，
本地 daemon 和 worker 持有执行权；客户端关闭不会清除该任务。

## 6. 验证并读取结果

将 `RESULT` 替换为运行目录或 job-series 身份。

```text
trajecta --config quickstart.toml result verify RESULT --full
trajecta --config quickstart.toml result inspect RESULT
trajecta --config quickstart.toml result trajectory RESULT --particle-id 0
trajecta --config quickstart.toml run report --result RESULT
```

Full verify 成功时，状态为 `complete`，canonical、SQLite 和 provenance 摘要均匹配。
`run-report.md` 是派生视图，生成报告不会改变科学身份。

## 完成检查

- Manifest 状态为 `complete`。
- 异常粒子数为零。
- Full verify 成功。
- `particles.sqlite` 通过完整性与行数检查。
- `provenance-bundle.json` 与 manifest 身份一致。

任一项目失败时，请保留结果目录并查阅[故障索引](../operations/troubleshooting.md)。
