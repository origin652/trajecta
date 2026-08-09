---
title: How Trajecta is validated
description: Read Trajecta scientific, deterministic, product, performance, and clean-package validation results and reproduce the published charts.
---

# Validation overview

Trajecta validation follows a run from its meteorological inputs to the final
result directory. The checks cover the physical fields presented to the
integrator, particle lifecycle and mass accounting, reproducibility across
repeated runs, the SQLite and provenance products, and operation from a clean
release package.

This section also publishes a controlled comparison with FLEXPART. It uses a
short, shared advection case to place scientific ensemble measures and runtime
measurements on declared common boundaries. The raw JSON and CSV files are
available beside the charts, so the values can be read without extracting them
from an image.

## Validation layers

| Layer | What is fixed | What is checked |
| --- | --- | --- |
| Input identity | Source files, content hashes, dataset family, and resolved coverage | The run reads the intended immutable meteorological content |
| Meteorological interpretation | Variable names, units, grid coordinates, time axis, and vertical coordinates | Required fields are finite, queryable, and physically aligned with the dataset profile |
| Numerical lifecycle | Initial population, output schedule, termination classes, and mass rules | Every particle is accounted for at each event and the mass ledger closes |
| Determinism | Resolved inputs, worker setting, and digest definition | Repeated runs and declared worker comparisons retain the required normalized identities |
| Result product | Manifest schema, SQLite rows, WAL closeout, provenance, report, and CLI readers | The completed directory can be inspected and fully verified as one run product |
| Performance | Host, CPU affinity, executable hash, workload, timing boundary, warm-up, and repetitions | Median wall time, throughput, scaling, and peak resident memory are calculated from like measurements |
| Release package | Package manifest, bundled native libraries, clean extraction, and platform | Public commands can run a real project without relying on the source checkout |

These layers appear together because a timing sample is useful only when its
run also reaches the expected scientific and product state. A product check
likewise needs the input and run identities that locate the scientific
calculation it contains.

## Validation scopes

Trajecta uses several scopes rather than one all-purpose test:

### Unit and contract tests

Crate tests cover document parsing, interpolation, population updates,
boundaries, output encoding, job scheduling, local IPC, and result readers.
Schema validators keep public JSON shapes and examples synchronized with the
implementation. These tests run frequently and isolate a small behavior when a
regression occurs.

### Real-data integration runs

Integration cases open actual CFSR or ERA5 files and drive the production
reader, numerical path, output sink, and verifier. They exercise combinations
that synthetic arrays do not contain, including encoded coordinates, missing
values, vertical transforms, file boundaries, and reader selection.

### Product matrix

The product matrix runs the packaged CLI through project preparation, daemon
submission, result inspection, verification, trajectory reading, and report
generation. Cases span the supported dataset families, population modes,
directions, and reader choices. Clean-package runs are performed on Windows
x86-64 and Ubuntu 24.04 x86-64.

### Scientific and performance comparison

The current publication comparison fixes one ERA5 hybrid-level case, two
particle counts, one-hour transport, and a four-thread CPU allocation. It
evaluates shared ensemble quantities at three physical output times and reports
two different runtime boundaries. Details are split between the
[scientific method](science.md), [performance method](performance.md), and
[comparability matrix](comparability.md).

## How to read a result

Every formal aggregate has a top-level status and a set of named checks. The
status `passed` means the listed checks completed successfully under that
aggregate's fixed contract. Read the contract beside the status: particle
count, simulation duration, output cadence, dataset, reader, host, and
executable identity define where the numbers apply.

The published comparison has three forms:

| Form | Best use |
| --- | --- |
| Charts | Quick visual comparison of wall time, throughput, memory, and ensemble differences |
| CSV | Plotting, tabular analysis, and checking individual repetitions |
| JSON aggregate | Complete contract, executable identities, measurement order, derived medians, checks, and source artifact paths |

The [frozen data page](data.md) links all three and gives the local rebuild
command.

## Validation of a user's result

The formal publication matrix describes the release. A result produced on
another machine has its own validation path:

```text
trajecta result inspect RESULT
trajecta result verify RESULT --full
trajecta run report --result RESULT
```

`result inspect` summarizes the manifest, artifact inventory, and available
SQLite counts. Full verification checks lifecycle coverage, finite values,
mass accounting, SQLite consistency, provenance, and the canonical output
identity. The generated report places the run identity, inputs, resources,
quality summary, and artifact list in `RESULT/run-report.md`.

For a study with a new dataset, duration, domain, or physical process, record a
validation case that reflects that use. The published one-hour comparison is a
reference point for the included advection workflow; longer integrations and
additional processes can be evaluated with the same practice of fixed inputs,
declared metrics, repeated runs, and retained raw measurements.

## Published materials

- [Scientific validation method](science.md)
- [Performance measurement method](performance.md)
- [FLEXPART comparability matrix](comparability.md)
- [Frozen JSON, CSV, and charts](data.md)
