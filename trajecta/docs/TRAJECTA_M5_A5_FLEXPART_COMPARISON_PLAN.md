# Trajecta M5-A5 FLEXPART comparison and prerelease plan

Date: 2026-08-01
Status: amended A5.5 contract executed; technical comparison passed
Release status: user approved `0.1.0-alpha.1` publication closeout

## 1. Purpose

M5-A5 compares the complete Trajecta transport product with FLEXPART 11.1 on
one shared Linux host.  The comparison has two independent purposes:

1. measure transport-core and complete-product performance without hiding
   SQLite or provenance cost; and
2. describe scientific comparability at common output times without treating
   FLEXPART as a bitwise oracle for a different integrator.

FLEXPART remains an external GPL test reference.  It is not linked into,
copied into, or required by the MIT Trajecta product.

The original 300-second comparison was amended during A5.5 with explicit user
approval. The final accepted comparison uses a 600-second transport step and
1200-second output interval in both programs. This document records the final
contract; failed 300-second attempts remain preserved evidence.

## 2. Frozen identities

| Item | Frozen value |
|---|---|
| Platform | Ubuntu 24.04 x86_64 under WSL2 |
| Trajecta baseline | Git `main`; formal runs record the exact commit and source-tree SHA-256 |
| Trajecta version before release approval | `0.0.0` |
| FLEXPART source | `dace3affa2ba71677f12f3858b04aaf59f8ee51e` (`v11.1-5-gdace3af`) |
| FLEXPART compiler | system GNU Fortran, recorded in the build evidence |
| FLEXPART libraries | system ecCodes and netCDF-Fortran, recorded in the build evidence |
| CPU quota | four logical CPUs; Trajecta workers = 4, FLEXPART OpenMP threads = 4 |
| Run ordering | sequential; formal repetitions rotate tool/mode order |

The formal runner must fail if any identity changes.  It must not silently
rebuild from another FLEXPART branch, install a package, or substitute a
different meteorological product.

## 3. Shared meteorology

Both programs derive their meteorology from the same immutable official ERA5
hybrid-137 source fields for 2018-12-01 00:00, 03:00, 06:00, and 09:00 UTC.
Four frames are required: a three-frame Trajecta lock correctly fails with
`lock_build.coverage.missing_following` for this run window.

| Official source artifact | SHA-256 | Role |
|---|---|---|
| `raw/era5_hybrid137_20181201.nc` | `788e4695029c6b11bc6304b855c04d3744933dc24f9c69d8ef9f796a9170f78d` | 137-level `u`, `v`, `w`, `t`, and `q` |
| `raw/era5_lnsp_20181201.nc` | `0a68ff7bb74962b357c3e0f4ca0852c754c3917e6cb2bdaca7cf39c579fd2e4a` | logarithmic surface pressure |
| `raw/era5_hybrid_surface_base_20181201.nc` | `133b3a55408aa251f45491e44ee5ebf541dd52cc0bd807de427ac4f3f36be1a2` | common surface fields |
| `raw/era5_hybrid_surface_flux_20181201.nc` | `92ab45b3fd70f65a10ba0e14f484f4a67b0d6f72f369b3387235280631ed69d6` | Trajecta-only surface flux fields |
| `raw/era5_hybrid_pv_probe.grib` | `ea6e73eb680c31f358259e3dff7175f95cde26631ecb67ae438da9603da5669c` | official 137-level hybrid coefficients |

The two programs require different runtime encodings. Trajecta reads the
deterministically prepared NetCDF files below; FLEXPART reads four staged
GRIB frames named `EA18120100`, `EA18120103`, `EA18120106`, and
`EA18120109`.

| Trajecta ready artifact | SHA-256 |
|---|---|
| `era5_hybrid137_prepared_20181201.nc` | `eb8f8554c2ed87e0a49d8a7e5ddf52b7337c96461db7c650a8d890e3f400e0e6` |
| `era5_surface_20181201.nc` | `3c21ad12b54e559331710ada547d4fed6cdf8ece1450d7c92ec364691b51a730` |

The formal meteorology audit proves exact equality between common official
source fields and Trajecta ready fields, except the documented derived
`sp = exp(lnsp)` round-off. Every one of the 2,772 FLEXPART GRIB messages
must reproduce its source field within that message's ecCodes
`packingError`. The two zero-precipitation messages are structural FLEXPART
inputs only; convection and wet deposition are disabled. Thus source-field
identity is exact, while the two program-specific encoded byte streams are
not claimed to be identical.

## 4. Comparable scientific case

- direction: forward;
- population: ordinary instantaneous release;
- particle counts: 10,000 and 50,000;
- release time: 2018-12-01 03:00:00 UTC;
- horizontal source: longitude 5.00--5.50 degrees east and latitude
  49.00--49.50 degrees north;
- vertical source: 500--1,000 m above sea level;
- released tracer mass: 1 kg total, inert air tracer;
- transport step/synchronization: 600 s;
- output interval: 1200 s, including every common time available from both
  products;
- surface behavior: reflection;
- limited-domain exit: normal termination;
- convection, turbulent displacement, mesoscale turbulence, subgrid
  topography, chemistry, wet deposition, dry deposition, settling, restart,
  gridded concentration output, and receptor output: disabled.

The finite source box supplies a non-degenerate ensemble while keeping the
comparison advection-only.  FLEXPART and Trajecta use different random
generators to sample that box, so particle IDs are not presumed to form a
scientific crosswalk.

With `CTL < 0`, FLEXPART uses `LSYNCTIME` for particle transport. Its option
validator requires `LOUTSTEP` and `LOUTAVER` to be at least twice
`LSYNCTIME`; the harness enforces the final 600/1200 relationship.

## 5. Timing boundaries

Each particle count receives one untimed warm-up followed by three formal
repetitions.  `/usr/bin/time -v` (or an equivalently audited host mechanism)
records wall time, user time, system time, and peak RSS.

### 5.1 Equivalent core

- FLEXPART: direct executable with gridded and particle products disabled.
- Trajecta: the production runner with normal SQLite/provenance output still
  enabled; the equivalent-core duration is `runner_total - runner_output`
  from the existing opt-in low-overhead performance counters.

Trajecta does not gain a null sink or alternate numerical path for this
comparison.  The derived core duration and the complete product duration come
from the same scientific run.

For each particle count:

```text
median(Trajecta equivalent core) / median(FLEXPART core process) <= 1.5
```

If the ratio is larger, A must attribute and reasonably optimize the gap.
Output removal, lower precision, relaxed quality gates, or altered physics are
not permitted fixes.  Any residual exception requires written user approval.

### 5.2 Complete product

- FLEXPART product mode writes instantaneous longitude, latitude, and height
  particle NetCDF at the frozen output interval.
- Trajecta product mode writes its normal SQLite, provenance bundle, manifest,
  resolved documents, and reportable identities.

Complete-product timings are reported side by side but are not substituted
for the equivalent-core gate because the products intentionally differ.

## 6. Scientific metrics

At every common physical output time, report:

- active and terminated particle counts;
- finite/nonfinite counts;
- spherical horizontal centroid;
- great-circle distance from the source centroid;
- RMS great-circle dispersion about the centroid;
- mean and standard deviation of height ASL;
- occupied cells on a fixed 0.1 degree by 0.1 degree by 250 m audit grid;
- termination distribution by normalized class.

Cross-model rows additionally report horizontal centroid separation, vertical
centroid difference, transport-distance difference, dispersion-width ratio,
and occupancy ratio.  These differences are report-only in A5: the hard
scientific gates concern identity, coverage, finite values, lifecycle, mass,
and quality, not an outcome-selected tolerance between different algorithms.

## 7. Comparability classifications

Every published metric is labelled exactly one of:

- `exact`: identical definition and directly comparable bytes or counts;
- `aligned`: same physical quantity after a documented unit/time convention;
- `aggregate-only`: ensemble comparison is valid but particle pairing is not;
- `not-comparable`: one product lacks the required semantic quantity.

Expected initial decisions are:

| Item | Classification | Reason |
|---|---|---|
| Common official source fields | `exact` | Same frozen ERA5 values and source SHA-256 |
| Program-specific meteorology encodings | `aligned` | NetCDF values are exact/derived-aligned; FLEXPART GRIB values remain within each message's packing error |
| Simulation window and output instants | `exact` | Same UTC contract |
| Particle count and released total mass | `exact` | Same declared values |
| Longitude, latitude, height | `aligned` | Same physical coordinates after unit normalization |
| Ensemble centroid, width, travel, occupancy | `aggregate-only` | No justified particle-ID crosswalk |
| Per-particle continuous trajectory error | `not-comparable` | Different source sampling RNG and integrator identity |
| SQLite/provenance overhead | `not-comparable` | FLEXPART does not produce the same product |

The matrix may change only if evidence establishes a stronger or weaker
relationship; the reason must change with it.

## 8. Hard gates

Formal comparison fails if any of the following occurs:

- source, compiler, library, input, CPU quota, scenario, or command identity
  differs from the frozen contract;
- either program exits unsuccessfully or omits a declared output time;
- particle counts, lifecycle, mass, or quality accounting is inconsistent;
- an active scientific coordinate is nonfinite;
- Trajecta is not `Complete`, has abnormal termination, fails SQLite integrity,
  leaves a non-empty terminal WAL, or fails provenance/digest verification;
- FLEXPART output cannot be parsed or its active-particle/status accounting is
  ambiguous;
- repeated Trajecta canonical SQL or explicitly path-normalized provenance
  semantics differ;
- the equivalent-core median ratio exceeds 1.5 without completed attribution
  and an accepted exception;
- a failed attempt is overwritten, silently retried, or replaced with a
  smaller run.

Infrastructure absence is `external_blocked`, not passed.  A numerical,
scientific, lifecycle, or performance failure is `failed`, not
`external_blocked`.

## 9. Deliverables

- one machine aggregate JSON and schema;
- raw per-run timing JSON/CSV;
- raw per-output scientific JSON/CSV;
- four deterministic performance SVG charts: core wall time, complete product
  wall time, throughput/scaling, and peak RSS;
- one deterministic scientific comparison SVG;
- comparability matrix with the classifications above;
- FLEXPART build identity and command/configuration archive;
- Trajecta manifests, product identities, and representative complete run
  artifacts;
- A execution and adjudication report.

## 10. Version and release boundary

Implementation, preflight, performance work, and scientific adjudication all
remain at `0.0.0`.  Only after every A5 gate passes and the user explicitly
approves prerelease does A change all workspace versions once to
`0.1.0-alpha.1`, create the tag, publish the GitHub Prerelease, download it,
and pass clean-extraction smoke.  No intermediate version, `v2`, `fix2`, or
extra prerelease number is created.
