---
title: Extensibility 状态
description: 说明 Trajecta 当前扩展边界，以及为未来插件保留的设计入口。
---

# Extensibility

Trajecta `0.1.0-alpha.1` **尚未实现插件机制**。当前产品没有 plugin manifest、发现目录、
配置字段、动态加载器或插件 CLI 命令。

架构在气象 reader、数值模型、population strategy、输出 sink 和 runtime backend 周围
保留类型边界。这些边界支持贡献者讨论未来扩展设计，同时避免过早暴露不稳定产品合同。

未来插件提案需要定义版本协商、能力声明和可复现打包，并规定 provenance 身份与 schema
所有权。提案还需覆盖沙盒及故障隔离。第三方科学代码的验证责任也必须形成明确决策。

请勿向当前配置文档增加未声明字段，也不要编写依赖未来插件命令的自动化。发布带版本的
插件合同前，扩展功能需要修改源码并接受贡献者级审阅。
