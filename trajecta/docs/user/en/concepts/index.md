---
title: Project, Case, RunProfile, and DatasetLock
description: Understand how Trajecta separates a study, scientific intent, local execution choices, and exact meteorological inputs.
---

# Project, Case, RunProfile, and DatasetLock

Trajecta represents a run through four related objects. The split lets a study
keep its scientific design stable while selecting data locations and machine
resources for a particular computer.

| Object | Main question | Typical contents |
| --- | --- | --- |
| Project | Which experiments belong together? | Names and paths for Cases and Profiles; dataset-profile mappings; default Profile |
| Case | What physical simulation is intended? | Time, direction, meteorological domains, population, substances, numerical model, boundaries, and outputs |
| RunProfile | How will one Case run here? | Case path, output root, dataset roots and locks, reader backend, worker threads, and memory budget |
| DatasetLock | Which exact meteorological input is admitted? | File paths, byte sizes, hashes, valid times, grid signature, vertical signature, and interpretation-profile identity |

The command line selects a Project and one named Profile. That Profile selects
one Case. Together they resolve to one immutable input pair for the worker.

## Project: the study container

The project root contains `trajecta-project.yaml` plus project-relative files.
A minimal index has this form:

```yaml
schema_version: trajecta.project-index/v1
name: domain-fill-cfsr
cases:
  moisture: cases/moisture.yaml
profiles:
  product:
    path: profiles/product.yaml
    dataset_profiles:
      cfsr: cfsr-pgbl-pressure-v0
default_profile: product
```

Names on the left are local project identifiers. A Case refers to the logical
dataset `cfsr`; the selected project Profile maps it to the built-in
interpretation profile `cfsr-pgbl-pressure-v0`.

A project may contain any number of Cases and Profiles. Common arrangements
include:

- one Case with several Profiles for different worker counts or readers;
- forward and backward Cases that share one meteorological collection;
- several seasonal Cases that use equivalent Profile entries;
- a published configuration plus smaller local Profiles for quick checks.

Each Profile entry still points to one RunProfile document, and that document's
`case_path` selects one indexed Case. If two Cases use identical execution
settings, create two named Profile entries whose documents select their
respective Cases. This keeps the submission command unambiguous.

## Case: portable scientific intent

A Case expresses choices that affect the physical trajectory calculation. For
example:

```yaml
schema_version: 0
kind: case
metadata: { name: domain-fill-cfsr }
time:
  start: { seconds_since_unix_epoch: 1230789600, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1230790200, nanosecond: 0 }
  direction: forward
meteorology:
  domains:
    - id: global
      dataset: cfsr
      priority: 1
      horizontal_halo_cells: 1
particle_population:
  strategy: domain_fill_air_mass
  id: moisture-air
  domain_id: global
  target_particle_count: 1000
substances: []
numerics:
  time_step: { value: 300, unit: s }
  integrator: { model: rk2_spherical/v0 }
  boundaries:
    policies: [surface_reflect/v0, model_top_terminate/v0, global_periodic/v0]
  random_seed: 4202
outputs:
  - product: particle_state/v1
    schedule: { mode: endpoints }
    sink: { model: particle_state_sqlite/v1 }
```

The Case uses logical dataset names and contains no provider credential. Its
timestamps are exact UTC instants represented as Unix seconds plus nanoseconds.
Direction controls physical traversal from `start` toward `end`; the data
coverage calculation later orders those two instants chronologically.

Component references can move large domain, population, or output fragments
into separate project files. `case resolve` expands those references and emits
the normalized Case used for further checks.

## RunProfile: local binding and resources

The RunProfile connects the Case to paths and execution settings:

```yaml
schema_version: 0
kind: run_profile
metadata: { name: product }
case_path: ../cases/moisture.yaml
output_root: ../runs
datasets:
  - dataset: cfsr
    lockfile: ../locks/cfsr.lock.json
    data_roots: { met: ../data }
    reader_backend: rust
execution:
  worker_threads: 1
  memory_budget_bytes: 1073741824
  executor: cpu
  meteorology_reader: rust
```

Paths are resolved relative to the RunProfile file and remain inside the
project root. `data_roots` assigns a local directory to each root ID used by the
DatasetLock. The same logical Case can therefore run from different storage
layouts by selecting another Profile.

`worker_threads` and `memory_budget_bytes` become the queue resource request.
The reader settings choose the implemented meteorological backend. They are
recorded with the resolved Profile in the result directory.

## DatasetLock: exact data inventory

A DatasetLock is produced by finalization after Trajecta inspects local files.
It binds the logical dataset and interpretation profile to:

- every selected file's root ID and relative path;
- exact byte length and SHA-256;
- physical valid times and file roles;
- normalized horizontal-grid signature;
- pressure-level or hybrid-coordinate signature;
- normalized dataset-profile SHA-256;
- generator name and version.

The lock contains relative file locations. The RunProfile supplies the actual
root directories, which allows the same lock structure to be resolved on
Windows and Ubuntu when the locked content is placed under corresponding
project roots.

Finalization selects enough frames to cover the Case interval and its temporal
interpolation neighbors. It also checks that the dataset profile exposes the
capabilities required by the chosen population.

## Three project states

`project status` derives one of three states:

| State | Meaning | Useful next action |
| --- | --- | --- |
| `draft` | A recognizable Case or Profile is still missing required fields | Continue incremental editing and validation |
| `configured` | The logical documents are complete; data or locks are pending | Generate a data plan and prepare meteorology |
| `finalized` | Documents, paths, data files, profile identities, and locks agree | Run `doctor --deep` and submit work |

An error can accompany any state, for example an invalid index path or a
malformed document. Read the diagnostics as well as the state name.

## Validation, resolution, and finalization

These operations answer different questions:

1. **Validation** checks document shape, field types, references, and the
   selected command intent.
2. **Resolution** expands local component references into normalized Case or
   RunProfile content.
3. **Finalization** opens the selected data, verifies coverage and
   capabilities, then creates or validates DatasetLocks.

The split supports projects whose data arrives later. Case and RunProfile
review can finish while the project is `configured`; file identity becomes
available during finalization.

## Path containment

Project-relative paths are confined to the project root. Discovery and edits
reject absolute paths, parent traversal, empty paths, duplicate logical names,
duplicate raw YAML map keys, and symbolic-link targets outside the project.
This provides one predictable location for documents, locks, data mappings,
outputs, and fetch work.

When a project moves between computers, preserve its relative layout. Machine
configuration remains separate because daemon capacity and catalog location
belong to the receiving computer.

## What a run preserves

After queue admission, the attempt owns resolved copies of the Case and
RunProfile, input identities, execution settings, numerical identifiers, and a
unique run ID. Later project edits apply to later submissions. They do not
rewrite an existing attempt directory.

This boundary is the reason a Profile can evolve during a study while old
results remain readable. Use `result inspect` to see the values recorded for a
particular attempt rather than reconstructing them from the current project.

## Related concepts

- [Domain-fill water-vapor tracking](domain-fill.md) explains the dry-air
  carrier representation and boundary exchange.
- [Particle populations](populations.md) compares release, air-mass, and ozone
  initialization.
- [Coverage and execution identity](coverage-identity.md) follows time anchors,
  job series, runs, and attempts.
- [Provenance and result directories](provenance.md) shows how resolved inputs
  connect to output samples.
