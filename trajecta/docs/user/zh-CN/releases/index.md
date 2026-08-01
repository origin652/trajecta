---
title: 发布策略
description: Trajecta 预发布软件与不可变文档快照的版本策略。
---

# 发布策略

软件版本遵循 Cargo 预发布语义。Release tag 冻结源码、产品安装包、checksum、演示资料、
Validation 证据和双语文档快照。

文档使用 `mike`。`dev` 版本跟随已接收修改，并禁止搜索引擎收录。Release 快照保持
不可变。当前默认 release alias 指向 `0.1.0-alpha.1`。

文档修订不会建立软件版本。如果已发布页面需要实质性更正，应透明记录更正内容，并保留
原始证据身份。
