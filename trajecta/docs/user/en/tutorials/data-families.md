---
title: Data families and trajectory direction
description: Reference table for CFSR pressure, ERA5 pressure, ERA5 hybrid data, and forward or backward coverage.
---

# Data families and trajectory direction

The scientific workflow remains the same across dataset families. File
identity, frame cadence, vertical coordinate, capability set, and spatial
coverage differ.

| Family | Public profile | Vertical coordinate | Nominal frame cadence | Tutorial scope |
|---|---|---|---:|---|
| CFSR pressure | `cfsr-pgbl-pressure-v0` | Isobaric | 6 h | Global four-frame demo |
| ERA5 pressure | `era5-cf-pressure-netcdf-v0` | Isobaric | 6 h | Limited-area official preparation |
| ERA5 hybrid | `era5-cds-hybrid137-v0` | 137 model levels | 3 h | Limited-area official preparation |

Forward execution advances from `time.start` toward a later `time.end`.
Backward execution advances numerically toward an earlier end. Data planning
normalizes required physical coverage into ordered `coverage_start` and
`coverage_end` values. Provider requests must include interpolation anchors
outside the exact trajectory interval when the selected reader requires them.

Do not copy one tutorial and change only a filename. Regenerate the data plan,
select the matching dataset profile, inspect required capabilities, and create
a new DatasetLock through explicit finalization.
