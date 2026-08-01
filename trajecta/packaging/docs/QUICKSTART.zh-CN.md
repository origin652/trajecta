# Trajecta 离线快速入门

请将单独发布的四帧 CFSR 演示资料复制到
`examples/domain-fill-cfsr/data`。核对 SHA-256 后依次运行：

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --project examples/domain-fill-cfsr project finalize
trajecta doctor --deep
trajecta --project examples/domain-fill-cfsr run --profile product
trajecta result verify RUN_ID --full
```

下载地址和完整结果读取流程见：
https://origin652.github.io/trajecta/zh-CN/getting-started/quickstart/
