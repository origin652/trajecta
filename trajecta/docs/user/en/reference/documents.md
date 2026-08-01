---
title: Project, Case, Profile, and DatasetLock
description: Roles, relationships, path rules, draft states, and finalization contracts for Trajecta project documents.
---

# Project, Case, Profile, and DatasetLock

These documents separate scientific intent from local execution and verified
input content.

| Document | Responsibility | Typical format |
| --- | --- | --- |
| Project index | Names Cases and Profiles, maps datasets, selects a default Profile | `trajecta-project.yaml` |
| Case | Scientific time, domain, population, boundaries, meteorology requirements, outputs | YAML |
| RunProfile | Execution resources, reader, output root, and one selected Case path | YAML |
| DatasetLock | Verified files, hashes, capabilities, and coverage for one dataset binding | JSON |

## Relationships

A project may index many Cases and many Profiles. Each Profile selects one
indexed resolved Case. Multiple Profiles may select the same Case to express
different resource or reader choices. A single Profile does not fan out across
several Cases; create one Profile entry for each runnable Case association.

`dataset_profiles` maps every dataset identifier required by the selected Case
to a named dataset profile. Finalization resolves that mapping and binds it to a
DatasetLock.

## Paths and draft documents

Indexed paths use `/`, remain relative to the project root, and cannot contain
`.` or `..` segments. Absolute paths and normalized escapes are rejected before
any file outside the project can be touched. Duplicate YAML mapping keys are
invalid.

A Profile with missing required fields may remain a `draft` while the project is
being assembled. Unknown fields, wrong types, duplicate keys, and semantic Case
or Profile errors are reported as errors. A draft can be reviewed and used to
form a data plan, but cannot be admitted for execution.

## Finalization

`project finalize` validates every selected document, inspects actual data,
checks coverage and capabilities, then writes locks atomically. It does not
download data. Changing a Case, Profile, dataset mapping, or input file makes the
old binding stale and requires a new data plan and explicit finalization.
