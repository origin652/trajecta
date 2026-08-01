---
title: Tests and fixtures
description: Unit, contract, integration, real-data, product-package, performance, and documentation test layers in Trajecta.
---

# Tests and fixtures

Tests are layered by the claim they support.

| Layer | Purpose |
| --- | --- |
| Crate unit tests | Local invariants, parsing, algorithms, error paths |
| Contract tests | CLI envelopes, schemas, identity, lifecycle, path jail |
| Real-data replay | Reader, query, derivation, boundary, and population behavior |
| Runtime contracts | Production daemon and worker with retained attempts |
| Product matrix | Extracted package, native runtime, clean environment, platform coverage |
| Validation evidence | Frozen scientific and performance comparisons |
| Documentation gates | Executable examples, references, links, bilingual structure, SEO |

Real meteorological fixtures have recorded origin, size, and SHA-256. Tests must
not replace them with hand-written files when the claim concerns a production
reader or query path. Large or provider-controlled fixtures remain outside the
Git repository and are selected by frozen manifests.

Failure artifacts are evidence. A first-failure matrix keeps the attempt and
stops subsequent cells unless its contract explicitly permits continuation.
Never rewrite a failed artifact to make a gate appear successful.

Before submitting a change, run the narrow affected tests, workspace gates, and
`git diff --check`. Numerical, reader, job-recovery, package, or documentation
changes require their dedicated validators as well.
