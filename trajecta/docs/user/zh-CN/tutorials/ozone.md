---
title: ERA5 混合模式层臭氧教程
description: 准备 ERA5 混合模式层资料，按 PV60 规则生成平流层臭氧粒子，并进行反向轨迹计算。
---

# ERA5 混合模式层臭氧教程

臭氧示例组合了三项能力：按干空气质量填充气象区域，在粒子生成时分配携带物质，并从受体时刻
向更早时间反向积分。气象资料使用 ERA5 全部 137 个混合模式层。

案例从 2018 年 12 月 1 日 06:00 UTC 反向积分至 05:50 UTC。初始粒子只从 PV60 规则选出的
平流层区域生成，数量固定为 1,000。下载范围与气压层教程相同，为北纬 45–53°、东经 0–10°。

## 项目与运行配置

项目索引把逻辑资料 `era5-hybrid` 映射到内置资料配置
`era5-cds-hybrid137-v0`：

```yaml
--8<-- "examples/ozone-era5-hybrid/trajecta-project.yaml"
```

运行配置选择纯 Rust 读取器和本机目录：

```yaml
--8<-- "examples/ozone-era5-hybrid/profiles/product.yaml"
```

案例位于 `examples/ozone-era5-hybrid/cases/ozone.yaml`。主要设置如下：

| 案例设置 | 本例取值 |
| --- | --- |
| 物理起点 | 2018-12-01 06:00 UTC |
| 物理终点 | 2018-12-01 05:50 UTC |
| 积分方向 | 反向 |
| 区域 | 有限 ERA5 混合模式层网格 |
| 初始粒子群 | PV60 有效区域内的 1,000 个干空气载体粒子 |
| 携带物质 | `ozone` |
| 数值步长 | 300 秒 |
| 边界 | 地表反射、模式顶终止、有限区域终止 |
| 输出 | 两个物理端点写入 SQLite |
| 随机种子 | `4203` |

`product` 申请一个工作线程和 1 GiB 内存。资料锁位于
`locks/era5-hybrid.lock.json`，准备后的文件从 `data/` 下发现。

## 混合模式层如何表示气压

ERA5 模式层随地形和大气状态变化。第 `k` 层的气压由该层系数与当地地表气压共同重建，因此同一
层编号在不同位置和时刻可能对应不同气压。仅有三维变量文件还不足以完成垂直坐标查询；每个时次
还需要地表气压和官方混合坐标系数。

准备流程组合以下输入：

| 输入组 | 内容 |
| --- | --- |
| 三维模式层变量 | 第 1–137 层的气温、东西风、南北风、比湿和压力垂直速度 |
| 地表气压输入 | 用于恢复规范地表气压的对数地表气压 |
| 地表基础变量 | 位势、10 米风、2 米气温与露点、粗糙度、边界层高度和摩擦速度 |
| 地表通量 | 瞬时感热通量和水汽通量 |
| 垂直坐标元数据 | 官方半层系数表和准备过程标识 |

内置资料配置会恢复地表气压和近地层比湿，统一通量符号，并为 `diagnostics` 能力计算位涡。
混合模式层资料每三小时一个锚点。本例虽然只覆盖 05:50–06:00 UTC，时间插值仍需要
00、03、06 和 09 UTC 四组资料。

## PV60 粒子初始化

PV60 规则先筛选海拔 3,000 米以上的干空气，并要求按半球统一符号后的位涡大于
2 PVU。PVU 是位涡单位，1 PVU 等于 10⁻⁶ K·m²·kg⁻¹·s⁻¹。南半球位涡会先调整符号，
再应用同一个阈值。

Trajecta 在符合条件的干空气质量中建立 1,000 个等质量区间，并从每个区间抽取一个粒子。
臭氧摩尔分数按照每 PVU 60 ppbv 的斜率与位涡相联系，随后结合臭氧和干空气的摩尔质量，
换算为粒子携带的臭氧质量。

结果中保存两类相互关联的质量：

| 结果字段 | 含义 |
| --- | --- |
| `dry_air_mass_kg` | 粒子代表的有效平流层干空气质量 |
| `particle_mass` 中的 `ozone` | 粒子生成时分配的臭氧质量 |

轨迹输送会让臭氧质量随粒子移动。粒子群质量账本跟踪干空气载体质量在区域流入、流出和终止过程
中的变化。

## 在下载前生成资料计划

校验项目并保存计划：

```text
trajecta --project examples/ozone-era5-hybrid project validate
trajecta --project examples/ozone-era5-hybrid project data-plan --output hybrid-plan.json
```

计划中的单项资料要求应包含：

| 字段 | 预期值 |
| --- | --- |
| 逻辑资料 | `era5-hybrid` |
| 内置资料配置 | `era5-cds-hybrid137-v0` |
| 读取器 | `rust` |
| 能力 | `diagnostics`、`domain_fill`、`near_surface_transport`、`transport` |
| 时间覆盖 | 按先后顺序写为 2018-12-01 05:50–06:00 UTC |
| 资料锁路径 | `locks/era5-hybrid.lock.json` |

资料计划始终按时间先后书写覆盖范围，反向方向保存在解析后的案例中。

安装资料工具依赖并预览请求：

```text
python -m pip install -r requirements-data.txt
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json
```

预览会列出 CDS 请求、模式层、变量代码和地表配套资料，并给出区域、锚点及本地目标。账户凭据由
服务方客户端从标准配置或环境变量读取。

## 下载并准备资料

执行完整的下载和准备流程：

```text
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json --execute
```

服务方原始文件、派生字段和坐标系数的中间结果保存在 `data/.trajecta-fetch/`。准备过程会检查：

- 维度和变量是否齐全；
- 有效时次与资料锚点是否一致；
- 地表配套变量是否覆盖同一网格；
- 模式层顺序是否为 1–137；
- 混合坐标系数表是否匹配。

通过检查的 NetCDF 锚点会移入 `data/ready/`。助手还会写出
`data/TRAJECTA_FETCH_MANIFEST.json`，其中包含请求参数和文件散列。

混合模式层请求的数据量和准备步骤都多于气压层教程。CDS 可能异步准备三维模式层请求。重复执行
同一命令时，资料工具会复用已经匹配的下载和中间结果。

## 完成项目定稿并运行

先检查一个准备好的锚点，再创建资料锁：

```text
trajecta data inspect examples/ozone-era5-hybrid/data/ready/ERA5_HYBRID_READY_FILE.nc
trajecta --project examples/ozone-era5-hybrid project finalize
trajecta --project examples/ozone-era5-hybrid doctor --deep
```

将 `ERA5_HYBRID_READY_FILE.nc` 换成实际文件名。检查结果应识别
`era5_cds_hybrid137`、137 个模式层、有效时次、源角色和准备后网格。项目定稿会把主三维变量、
对数地表气压、地表文件和坐标系数元数据写入同一份资料锁。

以前台方式运行：

```text
trajecta --project examples/ozone-era5-hybrid run --profile product
```

工作进程先读取 06:00 UTC 快照，计算位涡，筛选符合 PV60 的干空气质量区间，并分配臭氧质量。
初始化完成后，积分器向 05:50 UTC 推进并写出两个端点。

!!! tip "反向案例的起点是较晚时刻"

    本例的粒子在 06:00 UTC 生成，随后向 05:50 UTC 积分。结果中的“初始位置”对应受体时刻。

## 读取反向结果

验证并查看结果：

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
trajecta run report --result RESULT
```

事件顺序与执行方向一致：第一项计划事件是 06:00，第二项是 05:50 UTC。随着积分进入更早时刻，
`integration_offset_ns` 变为负值；`elapsed_age_ns` 仍为非负数，表示粒子自 06:00 生成后已经
积分了多长时间。

粒子摘要会分别给出干空气载体总质量和臭氧总质量。粒子来源记录其所属粒子群和区域。若粒子在
两个端点之间穿出边界，终止记录会保存精确的物理时刻和边界面。

需要分析全部粒子时，可以用 JSONL 流读取状态，再通过 `particle_id` 与 SQLite 中的 `ozone`
质量行连接。按受体位置、早期位置或终止边界分组，可以得到对这段反向输送过程的不同观察。

## 扩展臭氧研究

延长反向时段时，将 `time.end` 移到更早时刻，然后重新生成资料计划。新增的三小时锚点准备完成
后，再执行项目定稿。需要观察中间演变时，可以把端点输出改为固定间隔输出。

提高 `target_particle_count` 会细化有效平流层质量的抽样，并降低每个粒子代表的干空气质量。
位涡计算、轨迹积分和 SQLite 写入也会相应增加。工作线程和内存预算可以在运行配置中单独调整，
无需改动科学案例。

更换区域时，需要为新网格准备完整的模式层、地表变量和坐标系数。有限区域边界由准备后的网格
定义，区域变化会同时改变流出数量和边界新生粒子的空间含义。

## 接下来

[粒子群模型](../concepts/populations.md)比较三种初始化策略。
[资料系列与轨迹方向](data-families.md)说明混合模式层与气压层为何使用不同的时次间隔和垂直签名。
科学对比方法与结果见[验证](../validation/index.md)。
