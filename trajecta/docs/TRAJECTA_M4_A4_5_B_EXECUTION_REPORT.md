# Trajecta M4-A4.5 B execution report

## Scope and responsibility

This report records only mechanical execution, artifact validation, aggregation,
and profiler availability for the frozen A4.5 plan.

- **A** owns hotspot explanation, optimization authorization, architectural and
  numerical decisions, performance acceptance, and all completion decisions.
- **B** was started separately by the user; B did not call a subagent.  B only
  ran the frozen repeat matrix, collected artifacts, checked the stated facts,
  and recorded the unavailable profiler.
- No Rust, Python, schema, registry, Case, Profile, numerical implementation,
  output format, cache, buffering, query, boundary, or threading code was
  modified by B during this execution.
- No 50k/100k A4 formal cell was run.  No commit or push was made.  This report
  does **not** claim completion of M4-A4.5, M4-A4, or M4.

## Execution identity and lightweight gates

| Item | Observed |
|---|---|
| Git HEAD | `91caefe115e29ee4153caaa975e518773ba97b9e` |
| WSL distro | Ubuntu-24.04, WSL version 2 |
| WSL kernel | `6.18.33.2-microsoft-standard-WSL2` |
| Rust | `rustc 1.94.0 (4a4ef493e 2026-03-02)` |
| Cargo | `cargo 1.94.0 (85eff7c80 2026-01-15)` |
| CPU | Intel Core i9-10900X, 20 logical CPUs, 10 cores / 2 threads |
| WSL memory at probe | `MemTotal=16226980 kB`; `MemAvailable=15414880 kB` |
| `perf` | unavailable: `bash: perf: command not found` |
| `perf_event_paranoid` | `2` |

The required lightweight gates passed:

```text
cargo fmt --all -- --check
cargo test --offline -p trajecta-met performance  # 2 passed, 1 ignored
cargo test --offline -p trajecta-core --test m4_a4_real_perf --no-run
python -m py_compile tools/validate_m4_a4_5_performance.py \
  tools/summarize_m4_a4_5_attribution.py \
  tools/run_m4_a4_5_perf_profile.py \
  tools/run_m4_a4_real_matrix.py
git diff --check
```

### Dirty worktree at execution start

The working tree was intentionally dirty and was preserved.  The complete
`git status --short` captured at start was:

```text
 M crates/trajecta-core/src/boundary/met_path.rs
 M crates/trajecta-core/src/integrator/mod.rs
 M crates/trajecta-core/src/output/provenance_bundle.rs
 M crates/trajecta-core/src/output/sqlite.rs
 M crates/trajecta-core/src/population/vertical_resolve.rs
 M crates/trajecta-core/src/runner/mod.rs
 M crates/trajecta-met/src/frame/mod.rs
 M crates/trajecta-met/src/grid/mod.rs
 M crates/trajecta-met/src/lib.rs
 M crates/trajecta-met/src/query/cache.rs
 M crates/trajecta-met/src/query/engine.rs
 M crates/trajecta-met/src/query/mod.rs
 M crates/trajecta-met/src/query/output.rs
 M crates/trajecta-met/src/vertical/mod.rs
 M docs/TRAJECTA_M4_MODEL_ASSIGNMENT.md
 M docs/TRAJECTA_M4_PARTICLE_LOOP_PLAN.md
?? ../origo-validation-v1.json
?? crates/trajecta-core/tests/m4_a4_real_perf.rs
?? crates/trajecta-met/src/performance.rs
?? crates/trajecta-met/src/query/metrics.rs
?? docs/B_PROMPT_M4_A4_5_EXECUTION.md
?? docs/B_PROMPT_M4_A4_EXECUTION.md
?? docs/TRAJECTA_M4_A4_5_A_IMPLEMENTATION_REPORT.md
?? docs/TRAJECTA_M4_A4_5_PERFORMANCE_ATTRIBUTION_PLAN.md
?? docs/TRAJECTA_M4_A4_B_EXECUTION_REPORT.md
?? testdata/M4_A4_5_PERFORMANCE_ATTRIBUTION.schema.json
?? tools/run_m4_a4_5_perf_profile.py
?? tools/run_m4_a4_real_matrix.py
?? tools/summarize_m4_a4_5_attribution.py
?? tools/validate_m4_a4_5_performance.py
```

## Instrumentation-on matrix: 3 × (w1 / w4)

Command executed exactly once with continuation enabled:

```text
python tools/run_m4_a4_real_matrix.py attribution --platform wsl \
  --repetitions 3 --continue-on-fail \
  --artifact-root target/m4-a4.5/b-matrix
```

All six attempts completed.  All six timing artifacts were individually
validated by `tools/validate_m4_a4_5_performance.py`; each reported schema
`trajecta.m4-a4.5-performance-attribution/v1` and `status=valid`.

| Worker | Attempt | run ms | runner total ns | complete | rows | normal / abnormal | execute I/O delta | Artifact |
|---:|---:|---:|---:|---|---:|---:|---|---|
| 1 | 1 | 47,994 | 47,994,674,604 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w1/attempt-1` |
| 1 | 2 | 47,897 | 47,897,379,949 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w1/attempt-2` |
| 1 | 3 | 48,257 | 48,257,352,072 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w1/attempt-3` |
| 4 | 1 | 47,459 | 47,459,419,675 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w4/attempt-1` |
| 4 | 2 | 48,976 | 48,976,408,764 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w4/attempt-2` |
| 4 | 3 | 49,501 | 49,501,004,270 | yes | 6,701 | 109 / 0 | all five counters 0 | `b-matrix/...w4/attempt-3` |

All are the frozen ERA5 hybrid, forward, 1,000-particle, 3,600 s / 600 s /
600 s scenario.  The scenario permits its existing 109 normal
`population_outflow` terminations; B did not reintroduce a 7,000-row hard gate.

## Instrumentation-off w4 baseline: 3 runs

The frozen `preflight` command was invoked three times independently with
`--continue-on-fail` and artifact root `target/m4-a4.5/b-baseline`.

| Attempt | mode | workers | run ms | complete | rows | normal / abnormal | attribution | execute I/O delta |
|---:|---|---:|---:|---|---:|---:|---|---|
| 1 | preflight | 4 | 48,795 | yes | 6,701 | 109 / 0 | `null` | all five counters 0 |
| 2 | preflight | 4 | 49,557 | yes | 6,701 | 109 / 0 | `null` | all five counters 0 |
| 3 | preflight | 4 | 48,735 | yes | 6,701 | 109 / 0 | `null` | all five counters 0 |

## Aggregated scaling and observation overhead

Aggregator artifact:

```text
target/m4-a4.5/b-matrix/summary/M4_A4_5_PERFORMANCE_ATTRIBUTION.json
SHA-256 a59e70b205d75fdb88f7769bdec6bd2dd71897be823ed860ffb6dc25ca56b1e9
```

| Metric | Value |
|---|---:|
| valid attribution repetitions, w1 / w4 | 3 / 3 |
| w1 median run ms | 47,994.0 |
| w4 median run ms | 48,976.0 |
| w1→w4 speedup | 0.9799493629532833 |
| w1 median top-level accounted fraction | 0.9895605712272453 |
| w4 median top-level accounted fraction | 0.9909150430841209 |
| instrumentation-off w4 median ms | 48,795.0 |
| instrumentation-on w4 median ms | 48,976.0 |
| measured observation overhead | 0.003709396454554703 (0.37094%) |
| target | <= 3% |
| overhead within target | yes |

This is a measurement, not an A performance/optimization decision.

## Normalized digest and correctness matrix

All nine manifest paths (six attribution-on plus three baseline) were read.
Every one reported `status=complete`, `terminations.abnormal_count=0`,
`terminations.normal_count=109`, and `provenance.sample_count=6701`.

| Group | Attempts | normalized content SHA | normalized SQL SHA | canonical output SHA |
|---|---|---|---|---|
| attribution w1 | 1, 2, 3 | `6a8acea3a935ab69b4aa25cb6c72c92b22bcf55949295defb09e986d00aa27c2` | `1779b14f7d6ab28f151b3992321e4f02be7fd2129ada5f003667ca28688ed2a9` | `a7c88412980a7f68a2fe1d6d13f09cd1ebe138509e8b86509ba7d48f4459e705` |
| attribution w4 | 1, 2, 3 | same | same | same |
| baseline w4 | 1, 2, 3 | same | same | same |

Thus each of the three normalized digest dimensions had exactly one value across
all nine successful attempts.  Bundle SHA, file SHA, and run IDs were not used
as equality gates.

## WSL CPU sampling profile

Command executed:

```text
python tools/run_m4_a4_5_perf_profile.py \
  --artifact-root target/m4-a4.5/b-profiles
```

Result artifact:

```text
target/m4-a4.5/b-profiles/cells/
  wsl__era5-hybrid__forward__p1000__w4__perf/attempt-1/
  M4_A4_5_PERF_PROFILE_RESULT.json
SHA-256 77d4d3e01cdb11cfb4c46c681a0d97a8621bd156dc21a5528be6a26f39f826e2
```

The profiler wrapper return code was 127 and its recorded result is `failed`,
but the underlying, exact external cause is:

```text
.../wsl-perf-run.sh: line 20: perf: command not found
```

The run produced no `perf.data`, report, script, folded, or flamegraph files;
therefore there is no inclusive/self top-20 table to invent or interpret.
Under the A4.5 contract this profiler branch is **external_blocked**, not a
numerical/algorithm failure and not passed.  No package, profiler, FlameGraph
tool, or dependency was installed.  `perf_event_paranoid=2` was read and
recorded despite the unavailable executable.

## Primary-hypothesis cross-evidence (no B adjudication)

The attribution artifacts contain the following observed median stage values:

| Stage | w1 median ns | w4 median ns |
|---|---:|---:|
| `runner_output_finish` | 36,132,083,300 | 37,447,642,459 |
| `output_finish_provenance_bundle` | 35,598,157,274 | 36,916,351,454 |
| `provenance_stream_write` | 35,447,457,793 | 36,758,489,169 |

These measurements are consistent with the existing hypothesis path:

```text
ProvenanceBundleBuilder::finalize_after_sqlite
  -> stream_write_bundle
  -> lockstep_write_samples
  -> unbuffered DigestingFile::write_all
  -> /mnt/e artifact
```

They are **not** a B hotspot explanation, causality claim, or authorization to
buffer/optimize.  B made no implementation change.

## Status matrix and handoff

| Work item | implemented | executed | passed | blocked |
|---|---|---|---|---|
| instrumentation-on runner / timing artifact | A-provided | yes, 6/6 | yes, all 6 artifacts valid | no |
| instrumentation-off baseline | A-provided | yes, 3/3 | yes | no |
| aggregate scaling / overhead | A-provided | yes | measured; overhead <=3% | no |
| normalized digest correctness matrix | B mechanical extraction | yes, 9/9 | yes, all three dimensions identical | no |
| WSL `perf` CPU profile | A-provided wrapper | attempted once | no | **external_blocked: `perf` unavailable** |
| 50k/100k formal matrix | not in this run | no | no | intentionally not run |

B stops here for A review.  No optimization, architecture/science conclusion,
commit, push, or phase-completion claim is made.  The user-controlled external
validation file was not modified.
