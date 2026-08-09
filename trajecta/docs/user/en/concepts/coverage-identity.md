---
title: Time coverage, spatial coverage, jobs, and attempts
description: Understand trajectory time, interpolation frames, domain support, dataset identity, job series, run IDs, and retained attempts.
---

# Coverage and execution identity

Two kinds of continuity matter in a Trajecta study. Meteorological coverage
keeps every trajectory query inside a prepared time and spatial support.
Execution identity keeps every queue submission, rerun, and result directory
distinct. The data plan and DatasetLock handle the first; job-series, run, and
attempt identities handle the second.

## Physical time in a Case

A Case records `start`, `end`, and `direction`:

```yaml
time:
  start: { seconds_since_unix_epoch: 1543644000, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1543608000, nanosecond: 0 }
  direction: backward
```

`start` is the population initialization side of the run. The integrator moves
toward `end` in the declared direction. A forward Case has a later end; a
backward Case has an earlier end.

For data acquisition and locking, Trajecta orders the two instants
chronologically:

```text
coverage start = min(Case start, Case end)
coverage end   = max(Case start, Case end)
```

This means a backward Case requests the same physical interval that a forward
Case with reversed endpoints would request. Direction still affects trajectory
advancement, boundary flux, event order, and particle age.

## Why interpolation needs neighboring frames

Meteorological fields are stored at discrete valid times. A query between two
frames uses the surrounding pair to interpolate in time. The lock requirement
therefore includes one frame before and one frame after the physical Case
coverage.

Consider a six-hourly dataset and a run from 06:10 to 11:50 UTC:

```text
00:00   06:00   [06:10 ───────── 11:50]   12:00   18:00
          pair for early queries ───────────┘
```

The 06:00 and 12:00 analyses bracket the Case. The additional before/after
frame rule can retain 00:00 and 18:00 as interpolation buffers for the locked
coverage. Dataset profile cadence and actual valid-time inventory determine the
exact selection.

An endpoint that lands exactly on a frame still follows the profile's declared
coverage rule. Generate the data plan rather than estimating file count from
duration alone.

## Data-plan coverage

`project data-plan` combines every indexed Profile with its selected Case and
emits a requirement containing:

- physical coverage start and end;
- one before-frame and one after-frame buffer;
- dataset ID and dataset-profile name;
- required capability set;
- project-relative data roots and lock path;
- a SHA-256 identity of the project input used for the plan.

The plan is deterministic for unchanged project content. The acquisition helper
expands it into provider-specific timestamps and target paths. A changed Case
or Profile produces a different project identity and calls for a new plan.

## DatasetLock coverage

Finalization inspects the valid times contained in actual files. The lock
builder selects profile-matched files that cover the Case plus interpolation
buffers. It reports separate diagnostics for:

| Gap | Meaning |
| --- | --- |
| Coverage outside available times | The physical Case interval extends beyond the inventory |
| Missing warmup frame | No required frame exists before the selected interval |
| Missing following frame | No required frame exists after the selected interval |
| No profile-matched files | Files exist, though none belong to the selected interpretation profile |

The DatasetLock records every selected file and its valid times. At submission,
the inventory is rebuilt from that lock and the RunProfile's local data roots.
Changing a file's bytes, grid, vertical topology, or profile identity breaks
the binding and is handled through a new finalization.

## Spatial support and the safe core

A meteorological dataset provides a horizontal grid and vertical support. A
Case meteorology domain connects a logical dataset to a domain ID, priority,
and `horizontal_halo_cells`.

The halo reserves neighboring grid cells for spatial interpolation. The
trajectory-safe core lies inside the outer source boundary after that reserve
is applied. For a global periodic grid, longitude wraps and the safe core spans
the periodic direction. For a limited grid, lateral faces define where
continuous domain crossing and domain-fill exchange occur.

Vertical support is bounded by the local transport floor and the available
model top. Terrain and surface pressure can move the lower valid boundary from
one horizontal location to another. Release placement, domain-fill
initialization, meteorological queries, reflection, and model-top termination
all use this resolved support.

## Several meteorological domains

A Case may list more than one domain. Each entry has a unique ID, logical
dataset, and priority. This supports a high-resolution nested domain with a
coarser surrounding dataset when both are prepared under compatible scientific
settings.

At a query point, domain precedence selects the highest-priority valid safe
core. The Case still declares one domain for a domain-fill population, because
its initial mass budget and lateral boundary need one coherent grid. Other
domains may participate in trajectory meteorology according to the Case domain
selection rules.

Each logical dataset obtains its own RunProfile binding and DatasetLock. Data
planning takes the union of requirements across the selected Case/Profile pair.

## Four layers of identity

It helps to separate configuration, data, execution, and scientific output:

| Layer | Examples | Where it is recorded |
| --- | --- | --- |
| Resolved configuration | Case SHA-256, RunProfile SHA-256, numerical model IDs | Run manifest and resolved documents |
| Input data | DatasetLock SHA-256, profile SHA-256, individual file SHA-256 | DatasetLock, manifest, and provenance |
| Execution | Job-series ID, run ID, attempt, resources, software versions | Job catalog and run manifest |
| Output content | Exact SQLite SHA-256, canonical SQL digest, provenance content digest, canonical output digest | Manifest and verification result |

Two runs may have different run IDs while sharing identical resolved input and
canonical output identities. Conversely, a file with the same name and a new
SHA-256 is a different input even when the Case did not change.

## Job-series ID

The local queue creates a UUID-v7 job-series ID when it accepts a new logical
submission. This ID is the normal argument for `job status`, `job wait`,
`job events`, cancellation, rerun, and forget.

The series groups attempts that originate from one accepted request. It is
stable across `job rerun` and keeps event/history queries connected.

## Run ID and attempt number

Every attempt receives a new UUID-v7 run ID and a one-based attempt number.
Together they select one worker execution and one result directory.

```text
job series S
├── attempt 1, run R1: interrupted
├── attempt 2, run R2: cancelled
└── attempt 3, run R3: complete
```

The attempt number communicates order within a series. The run ID is globally
distinct and is used in manifest, SQLite, provenance, particle-state joins, and
result lookup.

`job rerun` preserves the series and creates the next run ID. It keeps old
attempt directories intact. Submitting an edited Project/Profile pair creates a
new logical series because the requested input has changed.

## Stable particle identity

Particle IDs are derived from population identity and deterministic birth
coordinates such as event ordinal or domain-fill stratum ordinal. They do not
contain the run ID. Identical resolved scientific inputs can therefore use the
same particle IDs across worker counts and reruns.

Every trajectory record also contains job-series ID, run ID, and attempt. An
analysis can compare particle `42` across two runs while retaining the identity
of each source attempt.

## Restart and recovery identity

The local daemon records worker process identity and attempt state in its
catalog. After a daemon restart, it reconciles active rows with operating-
system process identity:

- a confirmed live worker remains owned by its attempt;
- a missing worker becomes `interrupted`;
- queued work remains eligible for dispatch;
- terminal work stays terminal.

Recovery changes lifecycle state when needed. It does not allocate a new run
ID or repeat a complete attempt. Another execution appears only after an
explicit rerun or submission.

## Comparing two results

Use identities according to the question:

- Compare Case and Profile digests to confirm equivalent resolved setup.
- Compare DatasetLock and dataset-content digests to confirm equivalent input
  bytes and interpretation.
- Compare stable particle IDs when aligning trajectories.
- Compare canonical output digests for normalized result equality.
- Keep run ID and attempt in every table so that operational histories remain
  distinguishable.

`result verify --full` recomputes the normalized output identities and audits
the lifecycle rows. The resulting record is a practical starting point for a
cross-run comparison.
