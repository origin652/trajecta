---
title: ERA5 hybrid ozone 教程
description: 使用准备后的 ERA5 hybrid 层资料运行反向平流层臭氧 domain-fill 工作流。
---

# ERA5 hybrid ozone 工作流

Ozone population 先建立 domain-fill 气团 population，再应用冻结的 PV60 平流层臭氧
规则。本教程使用准备后的 ERA5 137 层 hybrid 资料开展反向运行。

## 项目绑定

--8<-- "examples/ozone-era5-hybrid/trajecta-project.yaml"

--8<-- "examples/ozone-era5-hybrid/profiles/product.yaml"

完整 Case 位于 `examples/ozone-era5-hybrid/cases/ozone.yaml`。规则身份、随机种子、
边界、方向和粒子数均进入 resolved Case 身份。

## 准备与执行

```text
trajecta --project examples/ozone-era5-hybrid project data-plan --output hybrid-plan.json
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json --execute
trajecta --project examples/ozone-era5-hybrid project finalize
trajecta --project examples/ozone-era5-hybrid run --profile product
```

ERA5 hybrid 输入需要三维模式层变量、对数地面气压、地面配套变量和 preparation
metadata。能力覆盖不完整时，finalize 必须失败。对于反向 Case，应按物理时间顺序解释
结果，同时保留所记录的执行方向。

跨运行比较臭氧质量或粒子状态前，请执行 full verify，并保留准确的 data lock 与准备后
文件身份。
