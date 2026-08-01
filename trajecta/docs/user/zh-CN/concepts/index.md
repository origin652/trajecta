---
title: Project、Case、RunProfile 与 DatasetLock
description: 说明 Trajecta 如何通过四类文档分离科研意图和本机执行条件。
---

# Project、Case、RunProfile 与 DatasetLock

Trajecta 将可移植科研意图与本机执行细节分开保存。

| 对象 | 职责 | 常见维护者 | 运行前可修改 |
|---|---|---|---|
| Project | 为 Case、Profile 和资料 profile 映射命名 | 研究团队 | 可以 |
| Case | 定义时间、域、population、物理过程、数值参数和输出 | 科研用户 | 可以 |
| RunProfile | 绑定 Case、数据根、lock、输出根、reader 和资源 | 运行人员 | 可以 |
| DatasetLock | 记录准确文件、摘要、覆盖、profile 和能力 | Finalize 流程 | 需要显式重建 |

Project index 提供名称和相对路径。一个 Profile 选择一个 Case，一个项目可以包含多个
Case 和多个 Profile。同一组机器参数需要运行多个 Case 时，应建立多个 Profile entry；
这些 entry 分别指向目标 Case，也可以共享 dataset-profile mapping。

## Resolution 边界

Validate 检查文档形状和引用。Resolve 将组件文件展开为规范化内容。Finalize 检查真实
本地资料，并生成任务接收所需的准确 lock。提交运行时，resolved 副本进入结果目录，
后续编辑不会改写历史。

Project path 受项目根 jail 约束。绝对路径、父目录逃逸、重复 YAML map key 和重复逻辑
名称均会在发现阶段被拒绝。
