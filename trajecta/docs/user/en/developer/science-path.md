---
title: Meteorology and numerical path
description: Source indexing, profile selection, query preparation, interpolation, domain filling, and particle integration in Trajecta.
---

# Meteorology and numerical path

## Meteorological loading

`project finalize` gives the runtime a DatasetLock containing profile identity,
file hashes, time coverage, grid signature, vertical signature, and capabilities.
The worker verifies that binding before it loads data.

The meteorology path then:

1. scans only declared data roots;
2. matches containers to the requested dataset Profile;
3. indexes source-native coordinates, roles, valid times, and fields;
4. derives canonical fields with explicit quality and provenance;
5. prepares the frame window and query structures before particle execution.

Particle-loop queries read prepared memory. Hidden source-file I/O during the
execute phase is forbidden by the performance contract.

## Query and integration

The query engine performs spatial and temporal interpolation on the source
topology. It returns typed status, quality, values, and provenance assignments.
The core integrator consumes this public result and advances particle state with
the selected direction and time step. Boundary policies classify crossings and
terminal conditions.

Exact repeated queries may be reused within the frozen cache contract. Reuse
cannot merge scientifically distinct keys or alter normalized output identity.

## Domain filling

Domain-fill populations derive native atmospheric cells from meteorology,
compute valid layer mass, and assign particles according to the selected air-mass
or stratospheric-ozone rule. Boundary inflow may create later cohorts. Stable
particle identity is independent of worker scheduling, so deterministic output
does not depend on thread count.

Changes in this path require real-data replay, mass-ledger verification,
determinism checks, and the relevant numerical contracts.
