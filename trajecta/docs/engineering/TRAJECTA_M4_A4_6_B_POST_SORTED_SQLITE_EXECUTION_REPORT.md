# Trajecta M4-A4.6：post-sorted-SQLite WSL 正式复验报告（B）

日期：2026-07-27
状态：**六个 individual formal cells 均通过；formal aggregate 因 backward 50k→100k scaling ratio 超过 2.4 而失败。**
本报告不宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 执行边界与冻结身份

本轮由 B 仅做机械执行、artifact 保全、只读审计与证据汇总。A 独立完成 sorted SoA SQLite sink 性能实现与数值/性能裁决；**无 C、无子代理**。B 未修改任何实现、合同、schema、caps、容差、异常分类或版本；未 commit/push；未运行百万点；未触碰外层 `origo-validation-v1.json`。

- Git HEAD：`91caefe115e29ee4153caaa975e518773ba97b9e`
- 开始 source-tree SHA-256：`df4e97c5fc527b08da43d0c53111dd771050d3c5200f9b826fbee9b99f61f1d7`
- source identity file count：321
- 所有 preflight/formal cell recorded source identity：与上述 SHA 一致
- WSL release binary：`/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae`
- binary SHA-256：`4f2ba166b5cfc27aa46910a37f795aea7895577f150b4fd5ee3c3abf65a513ac`
- binary identity JSON SHA-256：`91b543b80ef1fba6082055e7572ad96621ab2fc7e067707f1cef347d9f3eb459`
- platform：WSL Ubuntu-24.04; kernel `6.18.33.2-microsoft-standard-WSL2`
- 独立 roots：
  - `target/m4-a4.6/post-sorted-sqlite-preflight/`
  - `target/m4-a4.6/post-sorted-sqlite-formal/`

旧 post-query-cache root 没有被 resume 或复制。

## 2. 本地门禁

完整日志：`target/m4-a4.6/post-sorted-sqlite-preflight/gates/local-gates.log`

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | passed |
| workspace clippy `-D warnings` | passed |
| `trajecta-core --lib` | 138 passed |
| `trajecta-met --lib` | 161 passed |
| production digest matrix: two-runs content/SQL | 1 passed |
| production digest matrix: workers/order/chunk | 1 passed |
| workspace tests | 454 passed / 11 ignored |
| workspace docs | passed |
| Python compile | passed |
| `git diff --check` | passed |

## 3. 新 root WSL preflight 与 indexed reader

preflight passed：`wsl__era5-hybrid__forward__p1000__w4/attempt-1`，130.0 seconds。

| Evidence | Value |
|---|---|
| cell-summary SHA | `21ee36bcfc31c416d43224bda6251585cce9cb9309d25bf92a1d41580a5ca289` |
| outcome / manifest / abnormal | `Complete` / `complete` / 0 |
| query gate | 19 logical / 13 executed / 6 reuse; unique=13; repeated=0 |
| lifecycle | 7 scheduled output times; 6,701 expected / actual rows; valid |
| execute I/O | all five deltas=0 |
| SQLite | integrity `ok`; terminal WAL=0 |
| bundle, semantic validator, normalized digests, mass/finite/population | all passed |

Reader artifact:

```text
cells/wsl__era5-hybrid__forward__p1000__w4/attempt-1/
  M4_A4_CONCURRENT_READER.json
SHA-256: e6dba5dfde683b8bebb88a71b87954bb7e41f346a31a91343f8182ef04e2e339
```

它满足新 contract：

```text
schema_version   = trajecta.m4-a4-concurrent-reader/v1
snapshot_contract= indexed_high_water_marks/v1; exact terminal row counts are postflight-only
successful active snapshots = 42 >= 1
```

每个 recorded sample 都含 `database_page_count`、`output_event_max_sequence`、`particle_max_id` 和 `particle_state_high_water`，且使用 running-before/running-after 的短只读事务。没有恢复 runtime `COUNT(*)` 全表轮询；终态 row count 由 postflight 全量审计验证。

## 4. Formal 六格结果

所有 six cell 的 direct binary SHA 证据、`Complete`/`manifest=complete`、abnormal=0、lifecycle valid、mass ledger、execute I/O five delta=0、scale-aware query repeated=0、SQLite integrity、terminal WAL、bundle semantic validation、dictionary caps、100k RSS/SQLite gate 和 backward high-boundary audit 均通过。

| Cell | Attempt | Status | runner ms | RSS bytes | SQLite bytes / WAL |
|---|---|---|---:|---:|---:|
| S50-F forward 50k w4 | attempt-1 | passed | 221,769 | 723,795,968 | 102,432,768 / 0 |
| S100-F forward 100k w4 | attempt-1 | passed | 521,386 | 1,417,822,208 | 208,076,800 / 0 |
| S50-B backward 50k w4 | attempt-1 | passed | 222,240 | 667,295,744 | 101,646,336 / 0 |
| S100-B backward 100k w4 | attempt-1 | passed | 544,803 | 1,343,500,288 | 206,700,544 / 0 |
| D100-F forward 100k w1 | attempt-1 | passed | 773,145 | 1,404,809,216 | 208,076,800 / 0 |
| D100-B backward 100k w1 | attempt-1 | passed | 768,342 | 1,337,085,952 | 206,700,544 / 0 |

Summary / manifest SHA pairs:

| Cell | summary SHA | manifest SHA |
|---|---|---|
| S50-F | `629b0a4ed0b9cf7c18d488a384418117b11c6ecd4d80fd14ee48d0e63de25004` | `85a0a16cffdf2042e1cdd7d4cde4b6a7d7e9b3742db961f39b73b04879800644` |
| S100-F | `4269fea338181d06b0d33c3de41eeed5d9304e4e2a856a0e0a05438c85a8ae1d` | `226c9f8c7a5792fe782a519a64672860da2792b61b5168a84f530fa34e1933d6` |
| S50-B | `f4b2ffa0ca3821d0129763e95bf9ed27d3e606f820d7b191bae023672774087c` | `7b11a62bd3006573a1fd9c3480fa823b5faa6a1ed2af8e69c4a90ae07b1562c2` |
| S100-B | `bedd93308fa7b8c3a50133c699fccbd62fdd32da2bf2de67b3b9b8588f36dc14` | `09eac95fb066ae743cde4ce6c0bd5954306b0ab25993df61519c54e67831f44b` |
| D100-F | `b45798e67ec63e76fb854bdcabba0b76accc01ee995f781e4e598e6252d074ef` | `4ea22bed4c1b39346dc32b8f4994ba0d0320f73d2e5fb6ed4a524e23e5dc795e` |
| D100-B | `4fd6ea18dbeb1b2594a0b886ee2e087c3695cd48b9891264ccd26d496b8cabd0` | `11a338903e4e116e8355da763f7d0d339438d75226b982b88d8f4b06b23afaf0` |

## 5. Aggregate scaling: stop point

Formal aggregate:

```text
target/m4-a4.6/post-sorted-sqlite-formal/summary/formal-20260727T063601Z.json
SHA-256: eea1309b01c978ca4d7a1b1999a75e583680e5cf193d2ca15e610a6f061237d8
```

| Direction | Formula | Result |
|---|---|---|
| forward | 521,386 / 221,769 = **2.351031929620461** | passed (`<=2.4`) |
| backward | 544,803 / 222,240 = **2.451417386609071** | **failed** (`>2.4`) |

Thus aggregate `passed=false` for the backward scaling gate only. This is a real performance failure, not `external_blocked`; no retry or implementation change was made.

## 6. Performance evidence: old post-query-cache vs sorted sink

| Direction / scale | previous user/sys/wall / FS input | sorted user/sys/wall / FS input |
|---|---|---|
| F50 w4 | 350.25 / 69.46 s / 11:48.91 / 81,432,860 | 337.30 / 13.99 s / 4:42.31 / 19,960,685 |
| F100 w4 | 755.11 / 172.80 s / 28:45.70 / 201,471,852 | 732.09 / 22.35 s / 9:51.49 / 25,612,645 |
| B50 w4 | 359.19 / 70.28 s / 12:00.46 / 80,220,605 | 337.15 / 14.19 s / 4:43.05 / 19,346,052 |
| B100 w4 | 788.81 / 172.93 s / 29:18.92 / 198,760,476 | 766.78 / 23.48 s / 10:16.65 / 25,032,178 |

Sorted sink removes the prior superlinear system-time/filesystem-input growth: forward new system ratio `22.35/13.99=1.598` and FS-input ratio `25,612,645/19,960,685=1.283`; backward `23.48/14.19=1.655` and `25,032,178/19,346,052=1.294`. Nevertheless, measured runner run-only backward ratio remains 2.4514 and fails the frozen aggregate threshold.

For reproducibility, the same four new cells have output rows / particle-state rows respectively:

```text
F50: 8,994 / 347,754
F100: 20,342 / 706,177
B50: 8,095 / 344,251
B100: 18,951 / 700,018
```

All terminal WALs are zero; 100k SQLite main DBs are under 512 MiB and peak RSS under 2 GiB.

## 7. Query, lifecycle, backward boundary

Scale-aware particle-loop gates all valid:

| Cell | B | logical / executed / reuse | unique / repeated |
|---|---:|---|---|
| S50-F | 3,297 | 6,613 / 6,607 / 6 | 6,607 / 0 |
| S100-F, D100-F | 8,875 | 17,769 / 17,763 / 6 | 17,763 / 0 |
| S50-B | 2,462 | 4,943 / 4,937 / 6 | 4,937 / 0 |
| S100-B, D100-B | 7,541 | 15,101 / 15,095 / 6 | 15,095 / 0 |

Read-only backward SQLite audit finds `domain_boundary` exact-birth termination count=0 in S50-B, S100-B and D100-B; invalid inflow origin/direction counts=0, abnormal=0 and lifecycle remains valid. Normal termination reason details are preserved in manifests, without reclassification.

All six formal reader JSONs passed the indexed high-water contract. Required/observed active snapshots were: S50-F 1/1,909; S100-F 3/4,468; S50-B 1/1,923; S100-B 3/4,674; D100-F 3/6,692; D100-B 3/6,679.

## 8. Digest determinism and cross-generation comparison

- Forward 100k w1/w4 triplet equal:
  `36c731ab…` / `cd40a6a5…` / `f44b3091…`.
- Backward 100k w1/w4 triplet equal:
  `7c4136e8…` / `72921216…` / `236a7fb3…`.
- Every new 100k cell’s normalized content / SQL / canonical triplet exactly equals the corresponding direction/worker cell in old `post-query-cache-formal/summary/formal-20260727T001031Z.json` evidence.

Exact bundle and SQLite file SHA are intentionally not compared cross-run because UUID and physical insertion layout may differ.

## 9. Handoff

The sorted SQLite production implementation fixes forward formal scaling and materially reduces system time/filesystem input, while retaining numerical output/digests and all non-scaling hard gates. It does **not** meet the frozen backward formal scaling threshold.

No partial attempt or external blocker occurred in this new root. **未 commit / 未 push；不宣称 M4-A4.6、M4-A4 或 M4 完成。** The report is a post-run documentation change; no matrix rerun will follow its source-identity change.交 A 最终裁决。
