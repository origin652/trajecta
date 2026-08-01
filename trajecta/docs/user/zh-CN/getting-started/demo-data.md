---
title: CFSR 演示资料
description: 说明 Trajecta 快速入门资料的来源、文件清单、SHA-256 身份和使用条件。
---

# CFSR 演示资料

快速入门资产包含 2009 年 1 月 1 日的四帧 CFSR 六小时压力层 GRIB2 资料。该资产与
平台安装包分开发布。

## 冻结清单

| 文件 | 字节数 | SHA-256 |
|---|---:|---|
| `pgbl00.gdas.2009010100.grb2` | 5,465,079 | `fd3949957d371b720617c0ebf4b984fdee27cd15a12733a03dedd883dd34d65c` |
| `pgbl00.gdas.2009010106.grb2` | 5,436,947 | `00a6340960a47502b3ca043c77a02bb7e3381a0f30dc18b77203ad989d3c591b` |
| `pgbl00.gdas.2009010112.grb2` | 5,452,202 | `f3cf06669e267fa69ed8d389d9180f0cfe395918a34baa2ab6bdcdf59373c5c5` |
| `pgbl00.gdas.2009010118.grb2` | 5,494,468 | `a10c5bddf9e01a5c519fbb45461ac019e2b3916661467b07c940ea99ee4ddade` |

确定性 ZIP 还包含 `MANIFEST.json` 和来源说明。归档 SHA-256 为
`cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8`。

## 来源与使用条件

文件来自
[NOAA NCEI CFSR 六小时低分辨率目录](https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/2009/200901/20090101/)。
[CFSR 官方元数据](https://www.ncei.noaa.gov/access/metadata/landing-page/bin/iso?id=gov.noaa.ncdc:C00765)
使用 GRBLOW 标记低分辨率 GRIdded Binary (GRIB) 产品。Use Constraints 要求引用数据集，并说明资料不附带准确性
或完整性担保。Access Constraints 给出分发责任声明，没有禁止电子再分发。
[NOAA Open Data 页面](https://www.noaa.gov/information-technology/open-data-dissemination)
说明 NOAA 数据向公众开放，同时说明 full and open access 方针。这些官方页面已于
2026 年 8 月 2 日核查。

演示归档保留源文件的名称和内容，并附加 manifest、内容散列与来源说明。使用资料时
请引用 Saha et al. (2010)：
[*The National Centers for Environmental Prediction (NCEP) Climate Forecast System Reanalysis*](https://doi.org/10.1175/2010BAMS3001.1)。
Trajecta 的 MIT 许可仅覆盖项目代码和自建元数据，不替代源资料条件。该归档不表示
NOAA 对 Trajecta 的认可。
