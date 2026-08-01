---
title: Project、Case、Profile 与 DatasetLock
description: Trajecta 项目文档的职责、关系、路径规则、draft 状态和 finalize 合同。
---

# Project、Case、Profile 与 DatasetLock

这些文档将科学意图、本机执行设置和已校验资料内容分开管理。

| 文档 | 职责 | 常用格式 |
| --- | --- | --- |
| Project index | 命名 Case 与 Profile，映射资料并选择默认 Profile | `trajecta-project.yaml` |
| Case | 科学时间、区域、population、边界、气象需求和输出 | YAML |
| RunProfile | 执行资源、reader、输出根目录和一条所选 Case 路径 | YAML |
| DatasetLock | 一个资料绑定的已校验文件、摘要、能力与覆盖范围 | JSON |

## 文档关系

一个项目可以索引多个 Case 和多个 Profile。每个 Profile 选择一个已索引且可解析的
Case。多个 Profile 可以选择同一个 Case，用于表达不同资源或 reader 设置。单个
Profile 不会展开为多个 Case；每项可运行的 Case 关联应有独立 Profile 条目。

`dataset_profiles` 将所选 Case 需要的各 dataset 标识映射到命名资料 Profile。
Finalize 会解析该映射，并将其绑定到 DatasetLock。

## 路径与 draft 文档

索引路径使用 `/`，保持为项目根目录内的相对路径，并且不能包含 `.` 或 `..` 段。
绝对路径与规范化后越界的路径会在触碰项目外文件前被拒绝。YAML mapping 中的重复键
同样无效。

Profile 缺少必填字段时，可以在项目组装期间保持 `draft`。未知字段、类型错误、重复键，
以及真实 Case 或 Profile 语义错误均按 error 报告。Draft 可以接受评审并参与资料规划，
但不能进入执行队列。

## Finalize

`project finalize` 会校验所选文档、检查真实资料与覆盖能力，随后原子写入 lock。该命令
不会下载资料。Case、Profile、dataset 映射或输入文件变化后，旧绑定会失效；用户需要
重新生成 data-plan 并显式 finalize。
