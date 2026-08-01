# Trajecta offline quickstart

Use `examples/domain-fill-cfsr` with the separately published four-frame CFSR
demonstration dataset. Verify the dataset SHA-256 list, copy the files into the
example `data` directory, and run:

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --project examples/domain-fill-cfsr project finalize
trajecta doctor --deep
trajecta --project examples/domain-fill-cfsr run --profile product
trajecta result verify RUN_ID --full
```

The online manual contains the exact download URL and complete inspection
sequence: https://origin652.github.io/trajecta/getting-started/quickstart/
