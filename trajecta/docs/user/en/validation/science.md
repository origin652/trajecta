---
title: Scientific validation method
description: Meteorology alignment, lifecycle gates, ensemble metrics, repeated runs, and interpretation limits for Trajecta validation.
---

# Scientific validation method

The frozen M5-A5 comparison uses official ERA5 hybrid-level meteorology and a
one-hour advection case. Both executables use a 600-second transport step, a
1,200-second output interval, four CPU threads, and equal 10,000 or 50,000
particle populations.

## Meteorological alignment

The audit fixes source-file identities, query points, physical times, units,
vertical interpretation, and encoded values. It rejects missing coverage,
ambiguous fields, unexpected non-finite values, or changed input content.

## Internal hard gates

Every Trajecta run must complete with zero abnormal particles and pass:

- lifecycle and population accounting;
- finite-value and quality checks;
- mass-ledger closure;
- output-event ordering;
- SQLite integrity and terminal WAL checks;
- provenance and canonical-output identity;
- repeated-run determinism.

## Cross-model measures

The comparison evaluates common physical output times with ensemble centroid,
transport distance, horizontal dispersion, vertical mean, and occupied audit
cells. Different source-sampling random generators and trajectory integrators
prevent a particle-ID crosswalk. Ensemble differences are report-only; they do
not replace either program's internal hard gates.

Across the frozen repetitions, horizontal centroid separation ranges from
about 61 m to 277 m. Dispersion-width ratios range from 1.0012 to 1.0051.
These values describe this one contract and dataset. They are not universal
accuracy tolerances.
