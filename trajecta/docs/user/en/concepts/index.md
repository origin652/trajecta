---
title: Project, Case, RunProfile, and DatasetLock
description: Understand the four documents that separate scientific intent from local execution in Trajecta.
---

# Project, Case, RunProfile, and DatasetLock

Trajecta separates portable scientific intent from local execution details.

| Object | Responsibility | Typical owner | Mutable before a run? |
|---|---|---|---|
| Project | Names Cases, Profiles, and dataset-profile mappings | Study team | Yes |
| Case | Defines time, domains, population, physics, numerics, and outputs | Scientist | Yes |
| RunProfile | Binds a Case to data roots, locks, output root, reader, and resources | Operator | Yes |
| DatasetLock | Records exact files, hashes, coverage, profile, and capabilities | Finalization | Recreated explicitly |

The project index provides names and relative paths. A Profile may select one
Case, while a project may contain multiple Cases and multiple Profiles. Create
separate Profile entries when one machine setting must run several Cases; each
entry points to its intended Case and can share dataset-profile mappings.

## Resolution boundary

Validation checks document shape and references. Resolution expands component
files into normalized content. Finalization inspects the actual local data and
creates the exact lock required for admission. A submitted run stores resolved
copies in its result directory, so later edits do not rewrite history.

Project paths are jailed under the project root. Absolute paths, parent escapes,
duplicate map keys, and duplicate logical names are rejected at discovery.
