# Trajecta

**Open-source Lagrangian water-vapor tracking and atmospheric trajectory framework**

[简体中文](README.zh-CN.md)

[![Release](https://img.shields.io/github/v/release/origin652/trajecta?include_prereleases&sort=semver)](https://github.com/origin652/trajecta/releases)
[![Documentation](https://img.shields.io/badge/docs-English-4051b5)](https://origin652.github.io/trajecta/)
[![中文文档](https://img.shields.io/badge/docs-简体中文-4051b5)](https://origin652.github.io/trajecta/zh-CN/)
[![M5.1 docs CI](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml/badge.svg)](https://github.com/origin652/trajecta/actions/workflows/m5-1-docs-ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Trajecta runs forward and backward Lagrangian simulations for moisture-source
analysis and atmospheric trajectory studies. A project combines a scientific
Case, an execution Profile, verified meteorological inputs, and immutable run
artifacts. The same command-line workflow is available on Windows and Linux.

> **Alpha status:** the current release is `0.1.0-alpha.1`. The supported
> product surface consists of the CLI, configuration documents, published JSON
> schemas, and on-disk result artifacts. Rust crate APIs remain internal to
> contributors during the alpha series.

## What Trajecta provides

- Domain-filling water-vapor tracking for moisture-source studies
- Conventional particle releases with forward or backward integration
- Air-mass and stratospheric-ozone population workflows
- CFSR pressure-level, ERA5 pressure-level, and ERA5 hybrid meteorology paths
- Project validation before data are available, followed by an explicit data
  plan and finalization step
- Foreground runs by default, plus a local daemon and persistent multi-job queue
- Attempt-aware recovery after interruption, with completed work preserved
- SQLite trajectory results, run manifests, provenance bundles, and full result
  verification
- Human-readable output and stable JSON or JSONL envelopes for automation

Plugin loading and general-purpose result exporters are outside the current
alpha product surface. The documentation records their reserved boundaries
without presenting future commands as available features.

## Get started

1. Download the archive for your platform from
   [GitHub Releases](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1).
2. Verify the archive with its adjacent SHA-256 file before extraction.
3. Obtain the separately versioned four-frame CFSR demonstration data described
   by the quickstart.
4. Follow the domain-fill walkthrough for your platform.

The walkthrough covers configuration, data inspection, project finalization,
`doctor --deep`, a foreground run, full verification, result inspection, and
trajectory reading. Its tested completion target is 15 minutes from extraction
to a verified result.

- [15-minute quickstart](https://origin652.github.io/trajecta/getting-started/quickstart/)
- [15 分钟快速入门](https://origin652.github.io/trajecta/zh-CN/getting-started/quickstart/)

Trajecta does not fetch meteorological data when a run starts. The optional
data helper previews requests by default and downloads only when `--execute` is
present:

```text
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

## Documentation

| Audience | English | 简体中文 |
| --- | --- | --- |
| First run | [Getting Started](https://origin652.github.io/trajecta/getting-started/) | [入门](https://origin652.github.io/trajecta/zh-CN/getting-started/) |
| Scientific workflows | [Tutorials](https://origin652.github.io/trajecta/tutorials/) | [教程](https://origin652.github.io/trajecta/zh-CN/tutorials/) |
| Task-oriented help | [How-to Guides](https://origin652.github.io/trajecta/how-to/) | [操作指南](https://origin652.github.io/trajecta/zh-CN/how-to/) |
| Concepts | [Concepts](https://origin652.github.io/trajecta/concepts/) | [概念](https://origin652.github.io/trajecta/zh-CN/concepts/) |
| Recovery and operations | [Operations](https://origin652.github.io/trajecta/operations/) | [运行维护](https://origin652.github.io/trajecta/zh-CN/operations/) |
| Scientific evidence | [Validation](https://origin652.github.io/trajecta/validation/) | [验证](https://origin652.github.io/trajecta/zh-CN/validation/) |
| CLI and file formats | [Reference](https://origin652.github.io/trajecta/reference/) | [参考](https://origin652.github.io/trajecta/zh-CN/reference/) |
| Contributors | [Developer Guide](https://origin652.github.io/trajecta/developer/) | [开发手册](https://origin652.github.io/trajecta/zh-CN/developer/) |

The English manual is the normative source. Release documentation keeps the
English and Chinese page trees synchronized.

## Supported release platforms

| Platform | Architecture | Package status |
| --- | --- | --- |
| Windows | x86_64 | Supported prerelease package |
| Ubuntu 24.04 | x86_64 | Supported prerelease package |

Each package contains the executable, native runtime dependencies, four example
projects, the data helper, and a compact bilingual offline guide. Scientific
input data are distributed separately from the software archives.

## Reproducibility and validation

Every completed run records resolved Case and Profile identities, input content
hashes, lifecycle totals, numerical quality evidence, and output identities.
The validation manual publishes the scientific comparison method, frozen raw
CSV or JSON evidence, and reproducible static charts. Performance figures are
reported with their workload, platform, reader backend, and measurement scope.

Result directories are immutable scientific records. Use the product commands
before reading SQLite directly:

```text
trajecta result verify RESULT --full
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id PARTICLE_ID
trajecta run report --result RESULT
```

## Repository layout

```text
.
├── trajecta/
│   ├── crates/          Rust workspace
│   ├── docs/user/       bilingual public manual sources
│   ├── docs/engineering/ plans, contracts, and execution records
│   ├── examples/        executable tutorial projects
│   ├── packaging/       package-only guides and release support files
│   ├── testdata/        schemas, examples, and frozen validation contracts
│   └── tools/           validation, packaging, and data helpers
└── .github/workflows/   CI, product checks, and documentation publishing
```

Engineering records stay outside the public documentation navigation, search
index, and sitemap.

## Build from source

The release packages are the supported path for scientific users. Contributors
can build the Rust workspace directly:

```text
cd trajecta
cargo build --offline --workspace
cargo test --offline --workspace
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo doc --offline --no-deps
```

Build the bilingual manual with MkDocs Material:

```text
cd trajecta
python -m pip install -r requirements-docs.txt
mkdocs build --strict
```

See the [Developer Guide](https://origin652.github.io/trajecta/developer/) for
native dependencies, crate responsibilities, test fixtures, and release gates.

## Contributing and support

Trajecta welcomes reproducible bug reports, documentation corrections, and
focused pull requests. Include the platform, exact command, structured
diagnostic code, and retained attempt artifacts when reporting runtime issues.
Scientific changes should include an executable regression and evidence for
their numerical effect.

- [Open an issue](https://github.com/origin652/trajecta/issues/new)
- [Browse current issues](https://github.com/origin652/trajecta/issues)

## License

Trajecta is available under the [MIT License](LICENSE).
