---
title: Trajecta tutorials
description: Four executable scientific workflows for domain filling, releases, air-mass populations, and stratospheric ozone.
---

# Tutorials

Each tutorial corresponds to one complete project under `examples/`. The
example files are the configuration source; Markdown includes them during the
site build. Relative paths work on Windows and Ubuntu 24.04.

| Tutorial | Population | Dataset family | Direction |
|---|---|---|---|
| [Domain-fill moisture](domain-fill.md) | Dry-air domain fill | CFSR pressure | Forward |
| [Release workflow](release.md) | Scheduled release | CFSR pressure | Forward |
| [Air-mass workflow](air-mass.md) | Dry-air domain fill | ERA5 pressure | Forward |
| [Ozone workflow](ozone.md) | Stratospheric-ozone domain fill | ERA5 hybrid | Backward |

The cases are intentionally short. Extend time coverage and particle count only
after `project data-plan`, `project finalize`, and `doctor --deep` agree with
the expanded study.
