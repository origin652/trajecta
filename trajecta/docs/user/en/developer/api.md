---
title: Rust API
description: Contributor-only Rust API documentation and compatibility status for Trajecta workspace crates.
---

# Rust API

Generate crate API documentation locally:

```text
cargo doc --offline --no-deps --workspace
```

Open `target/doc/trajecta_case/index.html` and follow workspace crate links.
Crate-level contract comments describe ownership and module responsibilities.
The build denies missing public documentation and forbids unsafe code throughout
the workspace.

Rust APIs are contributor-facing in `0.1.0-alpha.1`. They are not the stable
automation surface. External integrations should use CLI commands, public
schemas, machine envelopes, DatasetLocks, manifests, provenance bundles, and
read-only result products.
