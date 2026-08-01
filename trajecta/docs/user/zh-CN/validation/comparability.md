---
title: FLEXPART 可比较性矩阵
description: 说明冻结 Trajecta 与 FLEXPART 验证中准确、对齐、仅集合及不可比较的维度。
---

# FLEXPART 可比较性矩阵

可比较性需要逐字段和逐测量定义。共同使用“trajectory”名称，不能直接证明数值方法或输出
产品相等。

| 维度 | 状态 | 解释 |
|---|---|---|
| 输入源变量 | Exact | 使用相同冻结气象源内容 |
| 编码气象值 | Aligned | 审计共同数值、单位和物理解释 |
| 输出时刻 | Exact | 20、40 和 60 分钟三个共同物理时刻 |
| 粒子数与释放质量 | Exact | 合同数值相等 |
| 经度、纬度、海拔高度 | Aligned | 集合指标采用共同坐标含义 |
| 集合测量 | Aggregate-only | 对比质心、离散、输送和占用 |
| 单粒子轨迹 crosswalk | Not comparable | 采样 RNG 与积分器不同，无法按 ID 配对 |
| SQLite/provenance overhead | Not comparable | 本矩阵中没有对应的 FLEXPART 审计产品 |

该矩阵支持方法限定的结论。它不覆盖化学、沉降、湍流、长时间输送、其他网格和生产规模
输出 schedule。

科学图使用记录中的 20、40 和 60 分钟时刻。原始冻结图与 aggregate 一同保存在 raw
证据中；发布图只修正横轴标签，没有修改数据值。
