---
title: Trajecta — Lagrangian water-vapor tracking
description: Open-source framework for Lagrangian moisture tracking, domain filling, and forward or backward atmospheric trajectories.
---

# Lagrangian water-vapor tracking with Trajecta

Trajecta is an open-source Lagrangian water-vapor tracking and atmospheric
trajectory framework. It provides reproducible domain filling, Lagrangian
moisture tracking, release experiments, and atmospheric trajectories from a
command-line workflow. Both forward and backward trajectories are supported.

The framework is designed for research runs that must retain their numerical
inputs, configuration identity, lifecycle record, and output provenance. A run
produces an immutable result directory with a manifest, particle-state SQLite
database, and a content-addressed provenance bundle.

## Choose an entry point

| Goal | Start here |
|---|---|
| Obtain a verified result in about 15 minutes | [Domain-fill quickstart](getting-started/quickstart.md) |
| Learn one scientific workflow end to end | [Tutorials](tutorials/index.md) |
| Configure a project or operate a queue | [How-to guides](how-to/index.md) |
| Understand populations, coverage, and identity | [Concepts](concepts/index.md) |
| Recover from interruption or resource pressure | [Operations](operations/index.md) |
| Inspect scientific and performance evidence | [Validation](validation/index.md) |
| Look up exact commands and schemas | [Reference](reference/index.md) |
| Build or contribute to Trajecta | [Developer guide](developer/index.md) |

## Product boundary

Version `0.1.0-alpha.1` supports Windows x86_64 and Ubuntu 24.04 x86_64
release packages. The stable product-facing contracts during the alpha series
are the CLI, configuration documents, published schemas, and disk artifacts.
Rust crate APIs remain contributor-facing and may change before a stable
release.

!!! note "Alpha software"
    Preserve source data and result artifacts independently. Review the
    [alpha limitations](reference/alpha.md) before planning a production study.
