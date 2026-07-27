# Trajecta M4-A4.6：boundary / lifecycle 修复后 WSL 正式执行报告（B）

日期：2026-07-26
状态：**正式执行在 S100-F 首次失败后停止。**
本报告不宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 执行边界与冻结身份

本轮只执行 A 冻结后的门禁、WSL preflight、S50-F 单格性能门、formal resume、artifact 审计与报告整理。B 未修改 Rust/Python 实现、合同、schema、容差、资料、SQLite/provenance/query/boundary 路径或验收规则。

- Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
- **开始 source-tree SHA-256**：`64c6a04f66c1b3798207bdff6d96a0ad392f42f70b0f2325fcb8ec21de29f847`
- source identity file count：316
- WSL release binary：`/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae`
- binary SHA-256：`193ad94714baf14f710b073df6d2918f80ee1c7aeec5bebd6e760c1c5a3e9ccf`
- WSL kernel：`6.18.33.2-microsoft-standard-WSL2`
- Rust / Cargo：`1.94.0`
- 新 artifact root：`target/m4-a4.6/post-boundary-lifecycle-formal/`

旧 roots `memory-formal/` 与 `post-invalid-met-formal/` 均只读保留，未被复用。未 reset、clean、checkout、stash、revert、删除 attempt、commit 或 push；未读取或修改外层 `origo-validation-v1.json`。

## 2. 本地冻结门禁

完整日志：`target/m4-a4.6/post-boundary-lifecycle-formal/gates/local-gates.log`

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| production hybrid domain-fill builder | 1 passed |
| `trajecta-core --lib` | 132 passed |
| `trajecta-met --lib` | 159 passed |
| ignored real hybrid boundary replays | 5 passed, 2 filtered out |
| workspace clippy `-D warnings` | passed |
| workspace tests | 446 passed, 11 ignored |
| workspace docs | passed |
| `py_compile` | passed |
| `git diff --check` | passed |

未启用任何 M3 million-point ignored 测试。

## 3. 新源码 WSL preflight

命令：

```text
python tools/run_m4_a4_real_matrix.py preflight --platform wsl \
  --wsl-distro Ubuntu-24.04 \
  --artifact-root target/m4-a4.6/post-boundary-lifecycle-formal \
  --resume --stop-on-fail
```

结果：**passed**，1000 particles / forward / w4，154.4 s。

| Evidence | Value |
|---|---|
| attempt | `cells/wsl__era5-hybrid__forward__p1000__w4/attempt-1/` |
| cell summary SHA-256 | `0a9f758d4a75d61cb7872044702b3845bee6739389fdea1cc1a7a12fb6e448b4` |
| manifest SHA-256 | `91c20f03123a06a62efec5bdf95990ee238350a207da752adb342148ba64ade8` |
| manifest / outcome | `complete` / `Complete` |
| abnormal / normal | 0 / `population_outflow=109` |
| lifecycle | 7 scheduled times; 6,701 expected / 6,701 actual rows; valid |
| execute-phase I/O | five deltas all 0 |
| exact particle-loop query | 19 logical / 13 executed / 6 reuse; unique=13; repeated=0; valid |
| SQLite | integrity `ok`; WAL=0 |
| provenance | present; streaming semantic validator passed |
| mass / finite / population | all hard gates passed |

## 4. S50-F 单格与 12 分钟门

单格命令：

```text
python tools/run_m4_a4_real_matrix.py single --platform wsl \
  --wsl-distro Ubuntu-24.04 --direction forward \
  --particles 50000 --workers 4 \
  --artifact-root target/m4-a4.6/post-boundary-lifecycle-formal \
  --resume --stop-on-fail
```

S50-F **passed**，12-minute run-only gate 通过：

```text
runner_run_milliseconds = 685,771 ms
frozen limit            = 720,000 ms
```

| Evidence | Value |
|---|---|
| attempt | `cells/wsl__era5-hybrid__forward__p50000__w4/attempt-1/` |
| summary SHA-256 | `971a9105ddbc128ac8cc83cd769d0d8266153ae62e6890a6b1176510cfaf9150` |
| manifest SHA-256 | `5a6dad932f624bb3920a73f85cdd0f6129e1d6f4c0b1b110db16c5b0b42139d8` |
| outcome / manifest | `Complete` / `complete` |
| abnormal | 0 |
| initial / inflow / total particles | 50,000 / 3,297 / 53,297 |
| lifecycle | valid; 347,754 expected / actual state rows; duplicate/nonmonotonic/mismatch all 0 |
| peak RSS | 716,226,560 bytes |
| SQLite / WAL | 102,203,392 / 0 bytes; integrity `ok` |
| provenance | 151,912,094 bytes; records=45,005; field sets=9,001; samples=347,754 |
| execute I/O | five deltas all 0 |
| particle-loop query audit | valid; repeated exact executions=0 |

## 5. 正式 6-cell 矩阵

formal command 正确 resume 了本轮同 root/SHA 的 S50-F attempt，未重跑：

```text
[resume] wsl__era5-hybrid__forward__p50000__w4 ... attempt-1
```

| Cell | Parameters | Status | Reason / evidence |
|---|---|---|---|
| S50-F | forward / 50k / w4 | **passed** | 同上，attempt-1 |
| S100-F | forward / 100k / w4 | **failed** | exact query gate failure；attempt-1 |
| S50-B | backward / 50k / w4 | not run | `--stop-on-fail` after S100-F |
| S100-B | backward / 100k / w4 | not run | `--stop-on-fail` after S100-F |
| D100-F | forward / 100k / w1 | not run | `--stop-on-fail` after S100-F |
| D100-B | backward / 100k / w1 | not run | `--stop-on-fail` after S100-F |

## 6. S100-F 专项审计

完整保留的 first failed attempt：

```text
target/m4-a4.6/post-boundary-lifecycle-formal/cells/
  wsl__era5-hybrid__forward__p100000__w4/attempt-1/
```

其中包含 `command.json`、`binary-identity.json`、host/source identity、stdout/stderr、GNU time、concurrent-reader JSON、summary/cell-result、manifest、SQLite/WAL/SHM 与 bundle。

| Item | Measured value |
|---|---|
| cell summary SHA-256 | `75f6e2fd6816c9272e620af9dd9a2b3b2ebecd7eedc9fd9fa8f17e4bcd8e3934` |
| manifest SHA-256 | `45e2ad759b3f226fdaf22be26b42660dbe20c230e198bf2abdf77263efef815c` |
| return code | **101** |
| direct process wall | 30:11.94 |
| runner run time | 1,752,433 ms |
| manifest status / outcome | `complete` / `Complete` |
| initial / dynamically born / total particles | 100,000 / 8,875 / 108,875 |
| numerical steps / scheduled times / output rows | 6 / 7 / 20,342 |
| normal terminations | 11,462: `population_outflow=11,460`, `model_top=2` |
| abnormal / invalid meteorology / reflection limit | **0 / 0 / 0** |
| lifecycle | valid; 706,177 expected / actual rows; duplicate/nonmonotonic/mismatch all 0 |
| record / field-set / sample counts | 101,745 / 20,349 / 706,177; both dictionaries within caps |
| provenance | present; 329,191,285 bytes; SHA `c3153cd172513f84d55d23a365974a3636f1b90a113429e7d58d8ca0fb28c206` |
| peak RSS | 1,423,413,248 bytes (`<=2 GiB`) |
| SQLite / WAL / run directory | 207,486,976 / 0 / 536,719,954 bytes; integrity `ok` |
| execute I/O | five deltas all 0 |
| mass ledger | no tolerance violation |
| normalized digests | content `15d108b2…`; SQL `50c70f73…`; canonical output `76e3e924…` |

### Actual failure

The formal test’s first terminal error is:

```text
A4 particle-loop queries must match macro-step plus distinct interior-cohort
expectations, account for every execute/reuse, and never execute one exact key
twice; inspect the written summary
```

S100-F’s particle-loop query audit was invalid:

```text
logical requests:          17,769
executed batches:          17,769
exact key reuses:               0
unique exact keys:         17,768
repeated exact executions:      1
first repeated key:
0d13f5ec89307363fb3e3acec66786d5ea2cb6a32562a5125f70328f00941522
```

Therefore it is a real `failed` formal cell, not `external_blocked`, even though the repaired boundary/lifecycle, mass, SQLite, WAL, provenance, dictionary and RSS measurements pass.

## 7. Aggregate gates and stop point

Formal aggregate:

```text
target/m4-a4.6/post-boundary-lifecycle-formal/summary/formal-20260726T132419Z.json
SHA-256: 4d929a07e6961b8c4253d1d87cd385fa99d88f5a3ac53cdaee7eb493c20ffafb
```

It reports 2/6 observed cells and `passed=false`. The independently measurable forward scaling ratio also fails:

```text
S50-F = 685,771 ms
S100-F = 1,752,433 ms
ratio  = 2.5554201037955817 > 2.4
```

Backward scaling, four-100k RSS/SQLite aggregate, and w1/w4 determinism were not evaluated because the remaining cells were correctly not run. Exact SQLite/bundle SHA equality was not required across runs.

## 8. Handoff to A

Boundary/lifecycle defects from the prior S100-F are no longer observed in this attempt: `invalid_meteorology=0`、`reflection_limit=0`，and lifecycle is valid. The remaining blockers are:

1. particle-loop query audit has one repeated exact execution and test exit 101;
2. forward 50k→100k run-only ratio exceeds 2.4.

B stopped without retrying or modifying the implementation. **未 commit / 未 push / 不宣称 M4-A4.6、M4-A4 或 M4 完成。**
