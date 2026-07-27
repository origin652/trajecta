# Trajecta M4-A4.6：post-stable-ID 最终 WSL 正式复验报告（B）

日期：2026-07-27
状态：**正式六格 individual hard gates 与 aggregate hard gates 全部通过；最终裁决交 A。**
本报告不自行宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 执行边界、身份与 roots

本轮由 B 仅做机械执行、artifact 保全、只读审计与证据汇总。A 独立完成根因调查、stable-ID fast path 实现及数值/性能裁决；**无 C、无子代理**。B 未修改实现、合同、schema、caps、容差、异常分类、版本、测试门槛或执行参数；未 commit/push、未运行百万点、未触碰外层 `origo-validation-v1.json`。

- Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
- gates 后、所有 WSL run 开始 source-tree SHA-256：`8028c2eb403aa4bce6ff8776b7555d47201093b6080e4dc6924ab9fcbea70da1`
- identity file count：324；所有 preflight 和 six formal cell recorded source identity 与其一致
- WSL release binary：`/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae`
- binary SHA-256：`a44ab7fc942cf8ff286ca9f5e1578c6c35254451e7878644ad10fb4d65875dce`
- binary-identity JSON SHA-256：`099766ab24f18d9d992f4971a2fe035fcb4b6211b706cf0e2d2f711d7d04694b`
- platform：WSL Ubuntu-24.04; kernel `6.18.33.2-microsoft-standard-WSL2`
- 新 roots：
  - `target/m4-a4.6/post-stable-id-preflight/`
  - `target/m4-a4.6/post-stable-id-formal/`

没有 resume、复制或补齐 `post-query-cache-formal`、`post-sorted-sqlite-formal` 或 A 的 `a-stable-id-fastpath-backward-pair` cell。

## 2. 本地冻结门禁

完整日志：`target/m4-a4.6/post-stable-id-preflight/gates/local-gates.log`

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| workspace clippy `-D warnings` | passed |
| `trajecta-core --lib` | 139 passed |
| `trajecta-met --lib` | 161 passed |
| production digest matrix: content/SQL | 1 passed |
| production digest matrix: workers/order/chunk | 1 passed |
| workspace tests | 455 passed / 11 ignored |
| workspace docs | passed |
| Python compile (A0 + monitor + runner) | passed |
| `tools/validate_m4_a0_contracts.py` | passed |
| `git diff --check` | passed |

## 3. WSL preflight 与 concurrent reader

Preflight `wsl__era5-hybrid__forward__p1000__w4/attempt-1` 通过，elapsed 133.2 seconds。

| Evidence | Value |
|---|---|
| summary SHA | `fea3d9a636bd80b0ea1731071b2af8d6d44414cafdcd4c64c868a871d01f8771` |
| outcome / manifest / abnormal | `Complete` / `complete` / 0 |
| query gate | 19 logical / 13 executed / 6 reuse; unique=13; repeated=0 |
| lifecycle | 7 scheduled; 6,701 expected/actual state rows; valid |
| execute I/O | all five deltas=0 |
| SQLite | integrity `ok`; terminal WAL=0 |
| mass / finite / population / bundle / normalized digests | all hard checks passed |

Reader artifact is `M4_A4_CONCURRENT_READER.json` (SHA `b2bd1b0386b6257d940253df5b76d369e592faf521ab044e1b9e0af2bc117202`):

```text
schema_version    = trajecta.m4-a4-concurrent-reader/v1
snapshot_contract = indexed_high_water_marks/v1; exact terminal row counts are postflight-only
successful_active_snapshots = 43 >= required 1
```

Its samples contain `database_page_count`, `output_event_max_sequence`, `particle_max_id` and `particle_state_high_water`, with running-before/running-after short read-only transactions. Runtime `COUNT(*)` polling was not restored; exact terminal row counts remain postflight evidence.

## 4. Formal six-cell outcome

All six direct WSL binary cells passed their individual harness checks. Every cell has `RunOutcome::Complete`, manifest `complete`, abnormal=0, valid lifecycle/mass/finite/population audits, execute-phase I/O five delta=0, scale-aware query `repeated_exact_executions=0`, SQLite integrity=`ok`, terminal WAL=0, valid semantic bundle and manifest identity, and dictionaries under 128,000 records / 64,000 field sets.

| Cell | Attempt | runner ms | GNU user/sys/wall | FS in/out | v/invol ctx | RSS bytes | SQLite / bundle bytes |
|---|---|---:|---|---|---|---:|---:|
| S50-F | attempt-1 | 202,401 | 323.22 / 14.47 s / 4:24.21 | 19,960,685 / 327,768 | 532,090 / 1,199 | 725,340,160 | 102,432,768 / 151,912,094 |
| S100-F | attempt-1 | 432,879 | 649.99 / 25.38 s / 8:22.87 | 25,612,645 / 696,152 | 876,262 / 3,529 | 1,437,671,424 | 208,076,800 / 329,191,285 |
| S50-B | attempt-1 | 206,057 | 334.27 / 15.89 s / 4:26.79 | 19,346,052 / 326,312 | 510,997 / 1,061 | 665,329,664 | 101,646,336 / 142,245,624 |
| S100-B | attempt-1 | 459,467 | 675.30 / 24.61 s / 8:52.20 | 25,032,178 / 701,024 | 850,319 / 2,397 | 1,340,452,864 | 206,700,544 / 314,134,274 |
| D100-F | attempt-1 | 683,509 | 589.36 / 25.78 s / 12:44.11 | 25,612,645 / 696,640 | 875,944 / 3,372 | 1,398,718,464 | 208,076,800 / 329,191,285 |
| D100-B | attempt-1 | 694,590 | 605.38 / 25.27 s / 12:52.95 | 25,032,178 / 697,848 | 849,636 / 3,383 | 1,323,024,384 | 206,700,544 / 314,134,274 |

Every 100k peak RSS is below 2 GiB; every 100k SQLite main database is below the existing frozen 512 MiB bound; all terminal WAL files are zero.

### Particle, row, query and cap evidence

| Cell | target / seeded / final | inflow | state rows / output events | B; logical/executed/reuse; repeated | records / field sets |
|---|---|---:|---|---|---|
| S50-F | 50,000 / 53,297 / 53,297 | 3,297 | 347,754 / 8,994 | 3,297; 6,613/6,607/6; 0 | 45,005 / 9,001 |
| S100-F, D100-F | 100,000 / 108,875 / 108,875 | 8,875 | 706,177 / 20,342 | 8,875; 17,769/17,763/6; 0 | 101,745 / 20,349 |
| S50-B | 50,000 / 52,462 / 52,462 | 2,462 | 344,251 / 8,095 | 2,462; 4,943/4,937/6; 0 | 40,510 / 8,102 |
| S100-B, D100-B | 100,000 / 107,541 / 107,541 | 7,541 | 700,018 / 18,951 | 7,541; 15,101/15,095/6; 0 | 94,795 / 18,959 |

Each population audit is valid: initial count equals target, seeded/final/SQLite totals agree, and added particles are valid boundary inflow rather than silent fallback.

## 5. Aggregate scaling and determinism

Formal aggregate:

```text
target/m4-a4.6/post-stable-id-formal/summary/formal-20260727T093415Z.json
SHA-256: c946299179f6dc0ad56b1ffbd09f387ef7b7e25da0281d03cf7b5bbc3c71e868
aggregate.passed = true
```

| Direction | Formula | Result |
|---|---|---|
| forward | 432,879 / 202,401 = **2.13871967035736** | passed (`<=2.4`) |
| backward | 459,467 / 206,057 = **2.2298053451229514** | passed (`<=2.4`) |

This new backward formal result is calculated from this six-cell matrix, not copied from A’s directional pair. For comparison only: old post-sorted backward was `544,803 / 222,240 = 2.451417386609071`; A’s directional stable-ID evidence was `457,668 / 202,852 = 2.256167057756394`.

w1/w4 triplets are exactly equal:

```text
forward content / SQL / canonical:
36c731abcb930e2f3c410e065d254fce12ec6d49a70e03219235636a3baa9e2b
cd40a6a50145c84ebfc60f009ff3df34f0ff0e8e1f88a5b7d52f137b63e2ec7c
f44b3091d6b7c9fdebb11448d9aecb755bc270eb407f3732ab9244de87c551dc

backward content / SQL / canonical:
7c4136e87cd1e9e72b43a8b0d473ea107b0b495c7dc363b53b78dd1ee8414b87
729212161ed664abcd6791526de97a1d1e63f627578ada35107e79830ea9bc61
236a7fb3dea52cb6682692a0358dbc4b3f36916a27e6d1a381d066c2656b8419
```

All six new normalized digest triplets exactly match their same direction/size/worker counterparts in `post-sorted-sqlite-formal`; S50-B and S100-B additionally exactly match A’s `a-stable-id-fastpath-backward-pair`. Exact SQLite/bundle file SHA are intentionally not used for cross-run equality because run UUID and physical layout may differ.

## 6. Reader and backward high-boundary audit

All six formal reader JSON artifacts use `trajecta.m4-a4-concurrent-reader/v1` and `indexed_high_water_marks/v1; exact terminal row counts are postflight-only`, passed, and contain the four required high-water fields. Required/observed active snapshots:

```text
S50-F  1 / 1,732
S100-F 3 / 3,691
S50-B  1 / 1,780
S100-B 3 / 3,913
D100-F 3 / 5,912
D100-B 3 / 6,022
```

Read-only backward SQLite audit found `domain_boundary` exact-birth termination count=0 in S50-B, S100-B and D100-B. All have valid inflow origin/direction, abnormal=0 and valid lifecycle. Normal reasons remain in manifests without reclassification: B50 `population_outflow=5,626`; B100/D100 each `population_outflow=11,403`, `model_top=2`.

## 7. Cell identity evidence

| Cell | cell-summary SHA | manifest SHA |
|---|---|---|
| S50-F | `3bf4830295416f93ad26b864e31a2b810e80401f70101b095b1793919179b96e` | `80c1037f014ff3262d9109e3a1ee17ffe434e65e9fe6799be4f1ba93958deee5` |
| S100-F | `18e9f81cfa667d52fef6b8c81eab174ace339356209e3b7f3450b84d4e5a204f` | `8d7d1bb4bcf201443353c2d880c44d664b74bcd8ce7586a663c83485f2fe3f2a` |
| S50-B | `24579e5e1861013914d0da78215693a25290fcb9fc928d25bc163e53eefa183e` | `6ed937ba2694deb481685138220103cb07a1e0ea0b9f5777f6bcd8dae251f482` |
| S100-B | `32d2a14b234b9f63bf8d552e524b0c544b71c2ae6dbe435c5af3947625f1ce83` | `4905e028f97695d4b88be37f5db9d0f6f90f9be0610e1149cb84ffa29f69eb8b` |
| D100-F | `ba8ba04aa33b21348fb9ecb20b31b7bc3600ddfd1abfe8781e8f323f9fe68419` | `bf8c10c2f83f7f28e65663f3651893dd94ba9d5bac8c062e70cc193fa86c5d25` |
| D100-B | `68a5f9641d072e29f151c590eaaac58e32f3d4e1a9b9f986ff20b626b3b4627e` | `60921d8fa9fc0810f9ab786d309dbb1dae6e5d9dcc1f52f00d401402b6027d21` |

## 8. Handoff

No formal cell failed, no partial attempt occurred, and no external blocker was used. B did not alter the frozen execution conditions. The implementation and final acceptance decision remain A’s responsibility.

**未 commit / 未 push；不自行宣称 M4-A4.6、M4-A4 或 M4 完成。** Report creation is a post-run documentation change; B will not rerun the matrix after it changes the source identity.交 A 审阅 artifacts、质量账本、reader evidence 与 aggregate。
