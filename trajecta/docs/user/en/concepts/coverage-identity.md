---
title: Time coverage, spatial coverage, jobs, and attempts
description: Understand meteorological coverage, interpolation anchors, job-series identity, run identity, and retained attempts.
---

# Coverage and execution identity

## Time and space

A Case records start, end, direction, and logical meteorological domains. A
data plan converts every selected Case/Profile pair into ordered physical time
coverage and required capabilities. A DatasetLock records the coverage proven
by actual files.

Spatial coverage comes from inspected meteorological grids. Logical domains in
a Case select datasets and precedence; they do not authorize arbitrary local
paths. Interpolation halos and neighboring time anchors may enlarge the data
requirement beyond the nominal trajectory interval.

## Job series, run, and attempt

| Identity | Meaning |
|---|---|
| Job series | Stable user-visible request across reruns |
| Run ID | One admitted execution with immutable inputs |
| Attempt | Ordered execution history retained under the series |

`job rerun` creates a new attempt. It does not overwrite the previous result.
Daemon recovery reconciles admitted and running work, but a completed terminal
attempt is not resubmitted. Content identity, source identity, and lifecycle
state allow operators to distinguish recovery from a new scientific run.
