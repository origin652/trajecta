---
title: Performance measurement method
description: Read the frozen Trajecta and FLEXPART timing boundaries, repetitions, throughput, scaling, and peak-memory measurements.
---

# Performance measurement method

The published benchmark separates numerical transport from the complete result
product. This distinction matters for Trajecta because its default product
includes a queryable particle-history database, lifecycle closeout, provenance,
and verification work. A transport timing and a finished-product timing answer
different operational questions.

## Benchmark environment

| Item | Frozen value |
| --- | --- |
| Operating system | Ubuntu 24.04.1 LTS under WSL2 |
| Architecture | x86-64 |
| CPU affinity for formal runs | Logical CPUs `0-3` |
| Worker or thread count | 4 |
| Meteorology | Aligned ERA5 hybrid-level case |
| Simulation duration | 3,600 seconds |
| Transport step | 600 seconds |
| Product output interval | 1,200 seconds |
| Particle counts | 10,000 and 50,000 |
| Warm-up | One run of each mode at each particle count |
| Formal samples | Three runs of each mode at each particle count |

The three formal repetitions use rotated order. At 10,000 particles the order
starts with each of the three modes in turn; the same rotation is used at
50,000. This spreads first-run, cache, and neighboring-process effects across
the measured modes.

## Executable identity

The comparison records the exact executable bytes:

| Program | Identity |
| --- | --- |
| Trajecta benchmark binary | SHA-256 `266a3c31dfb98e5206ee3e1c1d885d207b667a0ecb9f6e6caee438435a81b24f` |
| Trajecta source tree | SHA-256 `5fe6b857c1c59072b1eae3603fe6a51b77f01171481cdbe1e8313cba74e225c1` |
| FLEXPART | Version 11.1, commit `dace3affa2ba71677f12f3858b04aaf59f8ee51e` |
| FLEXPART binary | SHA-256 `5b6aacf1ac653c26dca2fd14e498d5e705bfdf7a38896a789869c3c11d24c629` |
| FLEXPART toolchain | GNU Fortran 13.3.0, ecCodes 2.34.1, netCDF-Fortran 4.5.4 |

The formal benchmark binary carried the internal pre-release version `0.0.0`.
The source was subsequently stamped `0.1.0-alpha.1` without a numerical or
performance change between the benchmark source identity and that version
update. The tables on this page remain attached to the hashes above.

## Timing boundaries

Three modes are run independently:

| Mode | Timed work | Produced artifact |
| --- | --- | --- |
| FLEXPART core | The transport-oriented executable path with comparison output minimized | Core-run status and timing record |
| FLEXPART product | Transport plus its particle NetCDF product | One particle NetCDF and run record |
| Trajecta product | Packaged product run through its normal result lifecycle | Manifest, SQLite, provenance, verification inputs, and run record |

### Equivalent core

Trajecta instruments the production runner into named stages. Its equivalent-
core time is:

```text
runner_total - runner_output
```

This retains meteorological queries, population work, integration, and boundary
handling while removing the measured output stage. It is compared with the
FLEXPART core run. The formal ratio is:

```text
Trajecta equivalent-core median / FLEXPART core median
```

The frozen acceptance limit is `1.5`. Both particle counts fall below that
ratio.

### Complete product

Complete-product wall time follows each program's ordinary product path.
FLEXPART writes its particle NetCDF. Trajecta writes indexed SQLite particle
history and closes the manifest and provenance product used by its result
commands. The table reports these values side by side without applying the core
ratio limit.

!!! tip "Choose one timing boundary"

    Use equivalent-core when comparing transport work. Use complete-product
    when estimating the runtime a user waits for.

## How samples are summarized

Warm-up runs establish the code and data path before formal measurement. They
do not enter the published medians. For each formal mode and particle count:

1. wall time is collected with nanosecond precision where the runner provides
   it and with `/usr/bin/time -v` around the process;
2. peak resident set size, user CPU time, system CPU time, and filesystem
   counters are retained;
3. the three wall-time samples are sorted;
4. the middle sample becomes the published median;
5. throughput is `particle count / median seconds`.

The raw CSV includes every warm-up and formal sample. For example, one 50,000-
particle FLEXPART core repetition took 2.87 seconds, while the other two took
1.31 and 1.34 seconds. The published median is 1.34 seconds.

## Median wall time

| Particles | FLEXPART core | Trajecta equivalent core | Core ratio | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 1.330 s | 0.465 s | 0.350 | 1.310 s | 2.195 s |
| 50,000 | 1.340 s | 1.469 s | 1.097 | 1.370 s | 8.680 s |

At 10,000 particles, Trajecta's measured transport path is shorter than the
FLEXPART core median. At 50,000, the two core paths are close, with Trajecta at
about 1.10 times the FLEXPART median.

The Trajecta complete product grows more strongly because output rows grow with
the particle population. At 50,000 particles, 1.469 seconds belong to the
equivalent-core boundary and the complete product takes 8.680 seconds. The
remaining interval includes the SQLite and provenance output path plus the
surrounding product closeout.

## Throughput

| Particles | FLEXPART core | Trajecta equivalent core | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: | ---: |
| 10,000 | 7,519 particles/s | 21,486 particles/s | 7,634 particles/s | 4,556 particles/s |
| 50,000 | 37,313 particles/s | 34,029 particles/s | 36,496 particles/s | 5,760 particles/s |

Throughput is useful for this fixed one-hour workload; it is not a rate per
model step or per output row. A longer simulation adds transport steps, while a
shorter output interval adds product rows. Those changes affect the two timing
boundaries differently.

## Scaling from 10,000 to 50,000 particles

| Boundary | 50k / 10k wall-time ratio |
| --- | ---: |
| FLEXPART core | 1.008 |
| FLEXPART product | 1.046 |
| Trajecta equivalent core | 3.157 |
| Trajecta product | 3.955 |

The FLEXPART time is nearly flat over these two populations. This is a short
six-step case, so its fixed startup, meteorology preparation, and step-level
array work occupy much of the measured interval; increasing the particle array
from 10,000 to 50,000 adds little to the median on this host.

Trajecta's equivalent-core time grows by 3.16 times for five times as many
particles. Batching and the in-memory numerical path keep this below linear
growth. The product time grows by 3.95 times because SQLite state rows and
provenance records scale with the population and output events.

These ratios describe one step count and output cadence. With a longer
simulation, fixed startup occupies a smaller fraction of each run. With more
frequent output, product writing occupies a larger fraction.

## Peak resident memory

| Particles | FLEXPART core | FLEXPART product | Trajecta product |
| ---: | ---: | ---: | ---: |
| 10,000 | 354.0 MiB | 354.6 MiB | 84.0 MiB |
| 50,000 | 361.8 MiB | 368.5 MiB | 163.1 MiB |

Values are medians of the formal process peak RSS observations. Trajecta's
product path uses less peak resident memory in this matrix and shows a clearer
population-dependent increase. The RunProfile memory budget should still be
sized from the intended dataset, domain, duration, reader, and concurrency;
the queue uses that declared budget for admission.

## Applying the numbers to another workload

A small local calibration gives a better estimate than multiplying one table
entry across a different study. Keep the intended dataset and output cadence,
then run two particle counts and record:

```text
trajecta result inspect RESULT
trajecta run report --result RESULT
```

Compare runner time, output rows, SQLite size, peak RSS, and the number of
output events. For long studies, vary duration separately from particle count.
This separates cost per transport step from cost per stored particle state.

The [frozen data and charts](data.md) page provides all repetitions and the
static wall-time, throughput, and memory plots.
