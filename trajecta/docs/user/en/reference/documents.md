---
title: Project, Case, RunProfile, and DatasetLock reference
description: Look up Trajecta project relationships, document fields, path rules, draft states, data planning, locks, and finalization behavior.
---

# Project, Case, RunProfile, and DatasetLock

Trajecta separates portable scientific intent from machine-local execution and
the exact meteorological files used by a run. A project connects those parts by
name while keeping each source document readable on its own.

## Document roles

| Document | Format | Main role | Created or read by |
| --- | --- | --- | --- |
| Project index | YAML, `trajecta-project.yaml` | Names Cases and Profiles, selects dataset profiles, records an optional default Profile | `project init`, `project get/set`, `project validate`, `project finalize` |
| Case | YAML or JSON | Defines scientific time, meteorology requirements, population, numerics, physics, and output schedule | `case validate`, `case resolve`, project validation, worker |
| RunProfile | YAML or JSON | Selects one Case and supplies local output, dataset bindings, reader, and resources | Project validation, finalization, scheduler, worker |
| Data plan | JSON, `trajecta.data-plan/v1` | Lists deterministic file and capability requirements plus current local status | `project data-plan` |
| DatasetLock | JSON | Binds one logical dataset to immutable local files, hashes, coverage, and capabilities | `data lock`, `project finalize`, `doctor --deep`, worker |
| Resolved Case | JSON result artifact | Fully expands Case components and records source digests | `case resolve`, run admission, worker |
| Resolved RunProfile | JSON result artifact | Canonicalizes local paths and records the exact execution binding | Run admission and worker |

## Project index

The project index has schema identity `trajecta.project-index/v1`.

```yaml
schema_version: trajecta.project-index/v1
name: moisture-study
cases:
  wet-season: cases/wet-season.yaml
  dry-season: cases/dry-season.yaml
profiles:
  wet-local:
    path: profiles/wet-local.yaml
    dataset_profiles:
      met: era5-hybrid
  dry-local:
    path: profiles/dry-local.yaml
    dataset_profiles:
      met: era5-hybrid
default_profile: wet-local
```

### Fields

| Field | Constraint | Meaning |
| --- | --- | --- |
| `schema_version` | Exactly `trajecta.project-index/v1` | Index format identity |
| `name` | Non-empty string | Human project name |
| `cases` | Mapping with non-empty unique names and paths | Case name to project-relative document path |
| `profiles` | Mapping with non-empty unique names | Runnable Profile entries |
| `profiles.<name>.path` | Non-empty project-relative path | RunProfile document selected by that entry |
| `profiles.<name>.dataset_profiles` | Mapping from logical dataset ID to named profile | Selects preparation and lock behavior for every Case dataset |
| `profiles.<name>.template` | Optional non-empty name | Machine configuration Profile template used during preparation |
| `profiles.<name>.template_sha256` | Required with `template`; 64 hexadecimal characters | Identity of the selected template content |
| `default_profile` | Optional existing Profile name | Profile used when a project-aware command permits an omitted selection |

Every key at one mapping level is unique. Duplicate YAML mapping keys are
rejected before conversion to a typed index, including duplicates inside
`dataset_profiles`.

## Relationships between Cases and Profiles

A project may contain many Cases and many Profiles. Each Profile entry selects
one RunProfile document, and that document's `case_path` selects one indexed
Case. Several Profiles can refer to the same Case to express different reader,
thread, memory, or output choices.

One Profile does not expand across several Cases. A study that runs four Cases
with two machine configurations can represent the desired combinations as
separate named Profile entries. Each resulting run receipt then identifies one
resolved Case and one resolved RunProfile.

The logical dataset identifiers used by the selected Case must appear in the
Profile entry's `dataset_profiles` mapping. Extra or missing mappings are
reported during project validation and finalize preflight.

## Case document

A Case uses the current numeric Case schema version and `kind: case`. Top-level
objects reject unknown fields.

| Field | Role |
| --- | --- |
| `schema_version` | Numeric Case document contract version |
| `kind` | `case` |
| `metadata` | Name, description, authorship, or labels supported by the metadata contract |
| `time` | Direction, start and end instants, transport step, and output timing requirements |
| `meteorology` | Domains, logical dataset references, required fields, coverage, and vertical interpretation |
| `particle_population` | Release, air-mass, ozone, or domain-fill initialization and lifecycle settings |
| `substances` | Tracked substance definitions and initial mass relationships |
| `numerics` | Integration and boundary-control settings |
| `physics` | Selected physical modules and their parameters |
| `outputs` | Product and output-event specifications |

Components may be inline or refer to local component documents. `case resolve`
expands references, normalizes values, and records every source digest. The
worker receives this resolved form, so later edits to a source component do not
silently alter an admitted attempt.

Validation intent changes the minimum complete shape:

```text
trajecta case validate cases/study.yaml --intent simulation
trajecta case validate cases/study.yaml --intent met-probe
trajecta case validate cases/study.yaml --intent migration
```

Simulation intent requires the components needed for a numerical run. A
meteorological probe can use a smaller Case focused on query coverage. Migration
intent supports document conversion checks.

## RunProfile document

A RunProfile uses the current numeric Case schema version and
`kind: run_profile`.

| Field | Role |
| --- | --- |
| `schema_version` | Numeric RunProfile contract version |
| `kind` | `run_profile` |
| `metadata` | Descriptive Profile metadata |
| `case_path` | Local path to the one selected Case |
| `output_root` | Root under which unique run and attempt directories are created |
| `datasets` | Logical dataset bindings with lockfile, roots, optional cache root, and optional reader override |
| `profile_sources` | Explicit Profile file or non-recursive directory sources used by meteorological readers |
| `execution.worker_threads` | Positive scheduler CPU request |
| `execution.memory_budget_bytes` | Positive scheduler memory request in bytes |
| `execution.executor` | Non-empty executor identity |
| `execution.meteorology_reader` | Default `rust` or `native` reader for bindings without an override |

The resolved RunProfile canonicalizes local paths and records source digests.
Its memory budget is rounded upward to MiB for queue admission. A dataset
binding can override the Profile reader for one logical dataset.

## Project path rules

Paths stored in the project index follow a lexical and canonical project jail:

- they are relative to the project root;
- `/` is the portable separator in the index;
- empty components, repeated separators, `.` segments, and `..` segments are
  rejected;
- absolute paths and paths that normalize outside the root are rejected;
- canonicalization through an existing filesystem object may not escape the
  project root.

`project set` validates the resulting complete index before writing. A rejected
path leaves the index bytes unchanged and does not create a file outside the
project.

RunProfile data roots and output roots represent machine-local paths within the
resolved Profile contract. Project preparation keeps generated plan and lock
paths project-relative where the public project format requires it.

## Draft, configured, and finalized states

| State | Meaning | Available work |
| --- | --- | --- |
| `draft` | One or more required Case or Profile values are still absent | Continue incremental `project set`; inspect partial status |
| `configured` | Documents parse and resolve, while data, locks, or output preparation remains | Generate a data plan, prepare files, and run validation |
| `finalized` | Required documents, locks, capabilities, coverage, and output root are ready | Run doctor and submit a selected Profile |

Missing required RunProfile values can remain a draft during incremental setup.
Unknown fields, wrong types, duplicate keys, and semantic Case errors are
reported as document errors rather than draft fields.

## Data plan

```text
trajecta --project PROJECT project data-plan
trajecta --format json --project PROJECT project data-plan --output data-plan.json
```

The plan derives temporal and spatial coverage from the selected Cases, then
lists requirements by dataset and capability. It reports whether the current
local state is missing, partial, or ready. Requirements, roots, and capabilities
use deterministic ordering, so the same project state produces identical plan
bytes.

A project can be configured before data arrives. The plan describes what to
prepare and does not create a DatasetLock. After files are present, explicit
finalization inspects their real metadata and content.

## DatasetLock and finalization

A DatasetLock records:

- its schema and dataset-profile identity;
- coverage interval and spatial capability set;
- every selected file's relative path, size, and SHA-256;
- named local roots used to resolve locked paths;
- reader and metadata needed to verify the binding.

`data lock` builds one lock directly from a root, profile, and Case:

```text
trajecta data lock --root DATA --profile PROFILE --case CASE --output LOCKFILE
```

`--replace` permits replacement of an existing output after the full build
succeeds. Without it, an existing lock returns `data.lock_exists`.

Project finalization handles all selected mappings together:

```text
trajecta --project PROJECT project finalize
```

It validates the index and referenced documents, checks Profile-to-Case
selection, inspects data roots, derives required coverage and capabilities,
builds candidate locks, and checks the output root. Existing lockfiles are
replaced only after the complete preflight succeeds. A failed preflight returns
nested diagnostics and keeps the previous lock bytes.

Changing a Case, Profile, mapping, template identity, or locked input file makes
the old binding stale. Generate a new plan, prepare any changed files, and run
finalize again before submitting that Profile.
