---
title: Performance measurement method
description: Fair timing boundaries, repetitions, scaling, memory, and limitations of the Trajecta and FLEXPART comparison.
---

# Performance measurement method

The frozen comparison identifies FLEXPART 11.1 commit
`dace3affa2ba71677f12f3858b04aaf59f8ee51e` and both executable hashes. It uses
one Ubuntu 24.04 host, CPU set `0-3`, one warm-up per particle count, and three
formal repetitions in rotated order.

## Two timing boundaries

| Boundary | FLEXPART | Trajecta |
|---|---|---|
| Equivalent core | Transport-focused execution | Integrator, population, boundary, and meteorology path |
| Complete product | Particle NetCDF | SQLite lifecycle/science product plus provenance and verification work |

The equivalent-core gate is `Trajecta / FLEXPART <= 1.5`. Complete-product
times are reported without applying that gate because the products carry
different audit work.

## Frozen medians

| Particles | FLEXPART core | Trajecta equivalent core | Core ratio | FLEXPART product | Trajecta product |
|---:|---:|---:|---:|---:|---:|
| 10,000 | 1.330 s | 0.465 s | 0.350 | 1.310 s | 2.195 s |
| 50,000 | 1.340 s | 1.469 s | 1.097 | 1.370 s | 8.680 s |

Median Trajecta product RSS is about 84 MiB at 10k and 163 MiB at 50k. The
50k complete-product difference is dominated by SQLite and provenance
finalization. Core and product timings must remain separate in interpretation.

The formal run immediately preceded the one-time version stamp from `0.0.0` to
`0.1.0-alpha.1`. Published packages subsequently passed clean-download product
smoke tests on Windows and Ubuntu 24.04. The timing table remains tied to the
frozen executable identity in the aggregate; it is not a promise for other
hardware or workloads.
