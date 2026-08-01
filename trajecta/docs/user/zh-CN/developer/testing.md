---
title: 测试与 fixture
description: Trajecta 的单元、合同、集成、真实资料、产品安装包、性能和文档测试层。
---

# 测试与 fixture

测试按其支撑的声明分层。

| 测试层 | 目的 |
| --- | --- |
| Crate 单元测试 | 局部不变量、解析、算法和错误路径 |
| 合同测试 | CLI envelope、schema、身份、生命周期和路径 jail |
| 真实资料 replay | Reader、查询、派生、边界和 population 行为 |
| 运行时合同 | 使用生产 daemon 与 worker，并保留 attempt |
| 产品矩阵 | 解压安装包、native 运行时、干净环境和平台覆盖 |
| Validation 证据 | 冻结科学与性能比较 |
| 文档门禁 | 可执行示例、参考页、链接、双语结构和 SEO |

真实气象 fixture 记录来源、大小和 SHA-256。涉及生产 reader 或查询路径的声明，不能用
手写文件替代真实 fixture。大型资料或由 provider 控制的 fixture 保持在 Git 仓库外，
通过冻结 manifest 选择。

失败 artifact 属于证据。First-failure 矩阵会保留 attempt，并在合同未明确允许继续时停止
后续单元。不得改写失败 artifact 来制造门禁成功。

提交修改前，需要运行受影响的窄测试、workspace 门禁与 `git diff --check`。数值、reader、
任务恢复、安装包或文档修改还需要运行各自专用 validator。
