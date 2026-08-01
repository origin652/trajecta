# Trajecta M4-A4.6 A：stable-ID 状态更新缩放修复报告

日期：2026-07-27
状态：**A implementation + WSL backward 50k/100k pair passed；final six-cell formal matrix pending**

本轮由 A 独立调查、实现与验证，未使用子代理。后续 B 只按书面 Prompt 做机械正式复验；无 C 参与。

本报告不宣称 M4-A4.6、M4-A4 或 M4 完成；未 commit、未 push；未修改数值算法、科学容差、异常分类、SQLite schema、dictionary caps、验收门或任何版本号；未运行百万点；未触碰 `E:\flexpart\origo-validation-v1.json`。

## 1. 输入失败

`post-sorted-SQLite` 正式六格的 individual hard gates 全部通过，forward scaling 也已通过，仅剩：

```text
backward = 544,803 / 222,240 = 2.451417386609071 > 2.4
```

相对冻结上限只超出 11,427 ms。正式证据同时表明：

- backward 100k 的 particle states、queries、SQLite 与 bundle 均没有异常膨胀；
- sorted sink 已消除旧的 system-time 与 filesystem-input 超线性；
- backward 100k w1 与 forward 100k w1 墙钟近似，但 backward w4 的 CPU 消耗和并行效率较差。

因此本轮没有继续修改 SQLite，也没有放宽 scaling 门，而是检查 runner/integrator 的粒子状态热路径。

## 2. 根因

`ParticleBatch::set_state` 在每次替换一行状态时都会执行：

```text
遍历完整 id 列，确认 replacement id 没有与其他行重复
```

积分器和边界路径的正常更新都由原粒子状态派生，稳定 `ParticleId` 不会改变，但旧实现仍为每一行、每一步重复扫描完整 ID 列。结果是本应线性的状态更新带入近似 `O(updated_rows × total_rows)` 的隐藏工作。

该成本在 10k 时 ID 列仍较小，容易被边界查询与运行噪声掩盖；到 50k/100k 后，重复扫描的 CPU 和缓存成本迅速放大。证据与该机制一致：修复不改变 query 数、输出行或文件输入量，却显著降低 50k/100k 的 user CPU 和 runner time，并且 100k 的收益远大于 50k。

## 3. 最小实现

文件：`crates/trajecta-core/src/particle.rs`

`set_state` 现在遵守等价规则：

```text
replacement id == 当前行 id
    → 稳定身份未变化，不做全批 duplicate scan

replacement id != 当前行 id
    → 保留原有完整 duplicate scan；发现冲突仍 hard fail
```

其余验证和写入语义不变：

- replacement `ParticleState::validate()` 仍执行；
- 越界、坐标、质量、termination 与 substance columns 的验证不变；
- 真正改变 ID 时仍拒绝 `DuplicateParticleId`；
- integrator、boundary、population、output、schema、manifest 与 digest 公式均未改。

新增单测：

```text
particle::tests::set_state_preserves_stable_id_and_rejects_changed_duplicate
```

它同时冻结正常稳定-ID 更新和 changed-ID duplicate 负例。

## 4. WSL 真实资料证据

### 4.1 10k 正确性对照

真实 ERA5 hybrid、forward、10,000 particles、4 workers：

| 项 | sorted-sink 基线 | stable-ID fast path |
|---|---:|---:|
| runner ms | 34,154 | 34,461 |
| particle_state rows | 67,127 | 67,127 |
| output events | 1,128 | 1,128 |
| abnormal | 0 | 0 |

10k 性能差异在运行噪声内，但所有 hard gates 通过，normalized digest triplet 完全相同：

```text
content:   3c5af25acac17cf0af2891fed2cdfa47719b590bccaf420f32edc885cde7a272
SQL:       2adfadfd58cefd7117deed0eb73dddc77b8ba6d2c88be24dd47c9ac69afbe73d
canonical: 9329886b743be86c0d978f36cc6fb721a0b746beb810af809997197d313988f0
```

Artifact：

```text
target/m4-a4.6/a-stable-id-fastpath-diagnostic-10k/
```

### 4.2 Backward 50k/100k pair

两格使用相同源码、真实 ERA5 hybrid、backward、4 workers 和冻结 formal 参数：

| Cell | 旧 runner ms | 新 runner ms | 变化 |
|---|---:|---:|---:|
| backward 50k w4 | 222,240 | 202,852 | -19,388 (-8.72%) |
| backward 100k w4 | 544,803 | 457,668 | -87,135 (-15.99%) |

新 scaling：

```text
457,668 / 202,852 = 2.256167057756394 <= 2.4
```

GNU time 的 user CPU 同样下降：

```text
50k: 337.15 s → 319.29 s
100k: 766.78 s → 670.40 s
```

system time、filesystem inputs、query counts、state rows、event rows 和文件尺寸保持同一量级；这说明收益来自删除重复 CPU 扫描，而不是减少科学计算、输出覆盖或审计证据。

两格均满足：

- `RunOutcome::Complete`、manifest `complete`、abnormal=0；
- lifecycle valid、mass ledger valid；
- execute I/O 五项均为 0；
- particle-loop repeated exact executions = 0；
- SQLite integrity `ok`、terminal WAL=0；
- bundle、caps、RSS 与 backward boundary audit 均通过。

100k：

```text
peak RSS: 1,380,704,256 bytes < 2 GiB
SQLite:   206,700,544 bytes
records:  94,795 < 128,000
field sets: 18,959 < 64,000
```

50k 与 100k 的 normalized digest triplet 分别与旧 `post-sorted-SQLite` 正式对应 cell 完全一致。100k：

```text
content:   7c4136e87cd1e9e72b43a8b0d473ea107b0b495c7dc363b53b78dd1ee8414b87
SQL:       729212161ed664abcd6791526de97a1d1e63f627578ada35107e79830ea9bc61
canonical: 236a7fb3dea52cb6682692a0358dbc4b3f36916a27e6d1a381d066c2656b8419
```

Artifact：

```text
target/m4-a4.6/a-stable-id-fastpath-backward-pair/
```

执行 source identity：

```text
0d1ff6258cf04c69d7c60a9877535d91649bc6a6208916a190c41fefb79d9364
```

WSL release binary：

```text
/tmp/trajecta-m4-a4-target/release/deps/m4_a4_real_perf-913744333e73fcae
SHA-256 a44ab7fc942cf8ff286ca9f5e1578c6c35254451e7878644ad10fb4d65875dce
```

## 5. 本地门禁

```text
cargo fmt --all -- --check                                      passed
cargo clippy --offline --workspace --all-targets -- -D warnings passed
cargo test --offline -p trajecta-core --lib                     139 passed
cargo test --offline -p trajecta-met --lib                      161 passed
两项 production digest matrix                                  passed
cargo test --offline --workspace                                455 passed / 11 ignored
cargo doc --offline --workspace --no-deps                       passed
python py_compile                                               passed
python tools/validate_m4_a0_contracts.py                        passed
git diff --check                                                passed
```

## 6. 剩余退出条件

A 的 backward pair 已证明修复有足够余量，但它不是最终 six-cell aggregate。加入本报告与 B Prompt 后 source identity 会变化，因此仍必须在最终冻结源码上使用全新 artifact roots 运行：

1. WSL preflight；
2. S50-F、S100-F、S50-B、S100-B、D100-F、D100-B 六格；
3. forward/backward scaling 均 `<=2.4`；
4. w1/w4 与 cross-generation normalized digest 不退化。

在 B 交回完整正式证据并由 A 复验以前，不宣称 M4-A4.6、M4-A4 或 M4 完成。
