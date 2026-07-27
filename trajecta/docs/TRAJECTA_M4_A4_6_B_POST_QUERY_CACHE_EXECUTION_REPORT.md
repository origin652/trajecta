# Trajecta M4-A4.6：query-cache / backward-boundary 修复后 WSL 正式执行报告（B）

日期：2026-07-27
状态：**六个 formal cell 均完成并通过各自 harness 审计，但 formal aggregate 失败：两方向 50k→100k scaling ratio 超过 2.4。**
本报告不宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 执行范围、身份与边界

本轮由 B 机械执行；**未使用 C，也未调用子代理**。B 未修改实现、合同、schema、caps、容差、异常分类、版本或验收规则，未 commit / push。

- Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
- **运行开始 source-tree SHA-256**：`61f277d126ec2d4a8ab35bf223a33440b236c2673e33d9c6b681d85529144546`
- identity file count：318；所有 preflight 与正式 cell 的 recorded source identity 均与该 SHA 一致
- WSL release binary：`/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae`
- binary SHA-256：`193ad94714baf14f710b073df6d2918f80ee1c7aeec5bebd6e760c1c5a3e9ccf`
- 平台：WSL Ubuntu-24.04，kernel `6.18.33.2-microsoft-standard-WSL2`
- 唯一新 artifact root：`target/m4-a4.6/post-query-cache-formal/`

历史 `memory-formal`、`post-invalid-met-formal`、`post-boundary-lifecycle-formal` 与 A 的 Windows diagnostic roots 均未复用。未读取、修改或加入 Git 外层 `origo-validation-v1.json`。

## 2. 本地冻结门禁

完整日志：`target/m4-a4.6/post-query-cache-formal/gates/local-gates.log`

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| `trajecta-core --lib` | 135 passed |
| `trajecta-met --lib` | 161 passed |
| ignored real hybrid replays | 5 passed, 2 filtered out |
| workspace clippy `-D warnings` | passed |
| workspace tests | 451 passed, 11 ignored |
| workspace docs | passed |
| Python compile | passed |
| `git diff --check` | passed |

未启用任何 million-point ignored 测试。

## 3. WSL preflight

```text
python tools/run_m4_a4_real_matrix.py preflight --platform wsl \
  --wsl-distro Ubuntu-24.04 \
  --artifact-root target/m4-a4.6/post-query-cache-formal \
  --stop-on-fail
```

结果：**passed**。

| Evidence | Value |
|---|---|
| attempt | `cells/wsl__era5-hybrid__forward__p1000__w4/attempt-1/` |
| cell-summary SHA | `faf16682bb3d274b26edc6f455a81e9403cd55e3ec361641256cae89aca8c9c9` |
| manifest SHA | `b55e4135340c68e7b7091bf0f48838d124e43f724b4097c811e34805c61656cd` |
| outcome / manifest / abnormal | `Complete` / `complete` / 0 |
| query gate | `19 logical / 13 executed / 6 reuse`; unique=13; repeated=0 |
| lifecycle | 7 scheduled times; valid |
| execute I/O | five deltas all 0 |
| SQLite | integrity `ok`; WAL=0 |
| provenance/digests/mass/population | all hard checks passed |

## 4. S50-F 单格与运行时间门

S50-F passed：

```text
runner_run_milliseconds = 654,036 ms
frozen limit            = 720,000 ms
```

- attempt：`cells/wsl__era5-hybrid__forward__p50000__w4/attempt-1/`
- summary SHA：`fbd81227efc99c5534885f864f1b583d40b1aba2e94a24148053ca6d175d46b4`
- manifest SHA：`471b51891b6c26114c77149139c79471ce9bbe76f16cc3b5552b92377ae2cec0`
- outcome/manifest=`Complete`/`complete`; abnormal=0
- particles：50,000 initial、3,297 inflow、53,297 total
- lifecycle valid；SQLite=102,203,392 bytes；WAL=0；RSS=724,430,848 bytes
- scale-aware query: B=3,297; `6,613 logical / 6,607 executed / 6 reuse`; unique=6,607; repeated=0
- I/O five deltas=0；mass/finite/population/bundle/digest all passed

## 5. 正式 6-cell 矩阵

formal `--resume --stop-on-fail` 正确复用上述新 root 的 S50-F；所有 six cells 的 test return code=0、outcome=`Complete`、manifest=`complete`、abnormal=0、lifecycle valid、SQLite integrity=`ok`、terminal WAL=0、execute I/O five delta=0、scale-aware query repeated=0。

| Cell | Attempt | Status | runner ms | RSS bytes | DB bytes / WAL |
|---|---|---:|---:|---:|---:|
| S50-F forward 50k w4 | attempt-1 | passed | 654,036 | 724,430,848 | 102,203,392 / 0 |
| S100-F forward 100k w4 | attempt-1 | passed | 1,666,472 | 1,418,600,448 | 207,486,976 / 0 |
| S50-B backward 50k w4 | attempt-1 | passed | 666,402 | 663,576,576 | 102,006,784 / 0 |
| S100-B backward 100k w4 | attempt-1 | passed | 1,700,220 | 1,358,344,192 | 207,159,296 / 0 |
| D100-F forward 100k w1 | attempt-1 | passed | 1,919,125 | 1,408,643,072 | 207,486,976 / 0 |
| D100-B backward 100k w1 | attempt-2 | passed | 1,932,191 | 1,321,443,328 | 207,159,296 / 0 |

### D100-B interrupted attempt preservation

The first all-six invocation exceeded the external 7,200-second agent observation window while D100-B was running. Its **attempt-1** remains intact and unmodified (`manifest=running`, partial WAL, no formal result); no existing attempt was overwritten. A fresh formal `--resume` invocation reused the five passed cells and generated D100-B **attempt-2**, which completed and is the only attempt used by the aggregate. This was an execution-control interruption, not a scientific/acceptance result and not a passed replacement artifact.

## 6. Scale-aware exact-query audit

All formal cells satisfy the frozen scale-aware formula and `repeated_exact_executions=0`.

| Cell | B interior births | logical | executed | reuse | unique | repeated |
|---|---:|---:|---:|---:|---:|---:|
| S50-F | 3,297 | 6,613 | 6,607 | 6 | 6,607 | 0 |
| S100-F | 8,875 | 17,769 | 17,763 | 6 | 17,763 | 0 |
| S50-B | 2,462 | 4,943 | 4,937 | 6 | 4,937 | 0 |
| S100-B | 7,541 | 15,101 | 15,095 | 6 | 15,095 | 0 |
| D100-F | 8,875 | 17,769 | 17,763 | 6 | 17,763 | 0 |
| D100-B | 7,541 | 15,101 | 15,095 | 6 | 15,095 | 0 |

Observed integrator and output logical query counts equal their formula-derived expected counts in every cell. Boundary/boundary-corner/lifecycle helper totals were not misclassified as particle-loop reuse.

## 7. Backward high-boundary read-only audit

B used read-only SQLite connections and the actual schema columns (`particle.origin_kind`, `particle.birth_seconds/birth_nanosecond`, `termination.physical_seconds/physical_nanosecond`).

| Cell | Inflow | invalid origin/direction | B | integrator observed/expected | reuse/repeated | exact-birth boundary termination | abnormal | lifecycle |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| S50-B | 2,462 | 0 / 0 | 2,462 | 4,936 / 4,936 | 6 / 0 | 0 | 0 | valid |
| S100-B | 7,541 | 0 / 0 | 7,541 | 15,094 / 15,094 | 6 / 0 | 0 | 0 | valid |
| D100-B | 7,541 | 0 / 0 | 7,541 | 15,094 / 15,094 | 6 / 0 | 0 | 0 | valid |

Thus boundary-origin particles were not an entire birth-time `population_outflow` batch. Normal reasons are legitimate: S50-B `population_outflow=5,626`; the two 100k backward cells each have `population_outflow=11,403` and `model_top=2`.

## 8. 100k evidence, RSS/SQLite/determinism

All four 100k cells are below RSS 2 GiB and SQLite 512 MiB limits. All have record/field-set counts below 128,000/64,000 caps, valid provenance semantic validation, mass/finite/population gates, and valid lifecycle.

| Cell | process wall | records / field sets / samples | bundle size / SHA prefix | normalized content / SQL / canonical |
|---|---|---|---|---|
| S100-F w4 | 28:45.70 | 101,745 / 20,349 / 706,177 | 329,191,285 / `8425e69d…` | `36c731ab…` / `cd40a6a5…` / `f44b3091…` |
| D100-F w1 | 33:00.49 | 101,745 / 20,349 / 706,177 | 329,191,285 / `49e689e2…` | `36c731ab…` / `cd40a6a5…` / `f44b3091…` |
| S100-B w4 | 29:18.92 | 94,795 / 18,959 / 700,018 | 314,134,274 / `cc1df2ec…` | `7c4136e8…` / `72921216…` / `236a7fb3…` |
| D100-B w1 | 33:30.76 | 94,795 / 18,959 / 700,018 | 314,134,274 / `9a2c521b…` | `7c4136e8…` / `72921216…` / `236a7fb3…` |

Both forward and backward w1/w4 normalized content, SQL and canonical-output digest triplets are exactly equal. Exact SQLite/bundle SHA and run IDs differ as allowed.

## 9. Aggregate failure and A handoff

Formal aggregate:

```text
target/m4-a4.6/post-query-cache-formal/summary/formal-20260727T001031Z.json
SHA-256: e54b795547d0d3f6eb785c06ad8eb1814a8d5c4ab1fb205bf86ea8f10bdafc02
```

Six cells were observed, but aggregate is `passed=false` for exactly these two gates:

```text
forward scaling:  1,666,472 / 654,036 = 2.5479820682653553 > 2.4
backward scaling: 1,700,220 / 666,402 = 2.5513428831246006 > 2.4
```

No individual scientific, lifecycle, abnormal termination, dictionary-cap, RSS, SQLite/WAL, I/O, digest, exact-query or backward-high-boundary gate failed. The two scaling failures are real performance failures, **not** `external_blocked`.

**未 commit / 未 push；不宣称 M4-A4.6、M4-A4 或 M4 完成。** Report creation is a post-run documentation change and does not invalidate the frozen identity recorded above; B will not rerun the matrix after this write.交 A 最终裁决。
