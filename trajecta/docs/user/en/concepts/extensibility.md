---
title: Extensibility status
description: Current Trajecta extension boundaries and the reserved future plugin surface.
---

# Extensibility

Plugin loading is not implemented. Trajecta `0.1.0-alpha.1` has **no plugin
mechanism**. There is no plugin
manifest, discovery directory, configuration field, dynamic loader, or plugin
CLI command in the current product.

The architecture keeps typed boundaries around meteorology readers, numerical
models, population strategies, output sinks, and runtime backends. These
boundaries allow contributors to review future extension designs without
exposing an unstable product contract today.

A future plugin proposal must define version negotiation, capability
declaration, reproducible packaging, provenance identity, sandboxing, failure
isolation, and schema ownership. It also needs an explicit decision about
scientific validation for third-party code.

Do not add undeclared keys to current configuration documents. Do not write
automation that assumes a future plugin command. Until a versioned plugin
contract is published, extensions require a source build and contributor-level
review.
