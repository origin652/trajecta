---
title: ERA5 压力层 air-mass 教程
description: 使用 ERA5 压力层资料准备并运行有限域气团 population。
---

# ERA5 压力层 air-mass 工作流

本项目在有限 ERA5 压力层区域上应用干空气 domain filling。Case 选择逻辑资料集和域，
RunProfile 将该资料集绑定到本地文件及 `era5-cf-pressure-netcdf-v0` profile。

## 项目真源

--8<-- "examples/air-mass-era5-pressure/trajecta-project.yaml"

--8<-- "examples/air-mass-era5-pressure/cases/air-mass.yaml"

## Finalize 前准备资料

```text
trajecta --project examples/air-mass-era5-pressure project data-plan --output era5-plan.json
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json
```

请检查 provider request 中的目标路径、时间、变量、压力层和区域。CDS 凭据只能来自
官方 CDS 配置或环境，不得写入项目、日志和 provenance。批准请求后再增加 `--execute`。

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json --execute
trajecta --project examples/air-mass-era5-pressure project finalize
trajecta --project examples/air-mass-era5-pressure run --profile product
```

有限域流出属于正常终止。Invalid meteorology、reflection limit 和 lifecycle mismatch
属于异常结果，需要单独调查。
