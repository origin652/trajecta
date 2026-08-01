---
title: 性能测量方法
description: 说明 Trajecta 与 FLEXPART 对比的公平计时边界、重复运行、扩展性、内存和解释限制。
---

# 性能测量方法

冻结对比记录 FLEXPART 11.1 commit
`dace3affa2ba71677f12f3858b04aaf59f8ee51e` 及两侧可执行文件摘要。测试使用一台
Ubuntu 24.04 主机、CPU set `0-3`，每个粒子规模先预热一次，再按轮换顺序正式重复三次。

## 两种计时边界

| 边界 | FLEXPART | Trajecta |
|---|---|---|
| Equivalent core | 以输送为中心的执行 | 积分器、population、边界和气象路径 |
| Complete product | 粒子 NetCDF | SQLite 生命周期与科学产品，加 provenance 和验证工作 |

Equivalent-core 硬门为 `Trajecta / FLEXPART <= 1.5`。Complete-product 包含的审计工作
不同，因此只报告计时，不套用该硬门。

## 冻结中位数

| 粒子数 | FLEXPART core | Trajecta equivalent core | Core ratio | FLEXPART product | Trajecta product |
|---:|---:|---:|---:|---:|---:|
| 10,000 | 1.330 s | 0.465 s | 0.350 | 1.310 s | 2.195 s |
| 50,000 | 1.340 s | 1.469 s | 1.097 | 1.370 s | 8.680 s |

Trajecta product RSS 中位数在 10k 时约为 84 MiB，在 50k 时约为 163 MiB。50k
complete-product 差异主要来自 SQLite 与 provenance 收尾。解释时应分开讨论 core 与
product 计时。

正式运行紧邻从 `0.0.0` 到 `0.1.0-alpha.1` 的一次性版本标记。随后下载公开安装包，
并在 Windows 与 Ubuntu 24.04 完成 clean-product smoke。计时表仍绑定 aggregate 中的
冻结可执行文件身份，不承诺其他硬件和 workload 的表现。
