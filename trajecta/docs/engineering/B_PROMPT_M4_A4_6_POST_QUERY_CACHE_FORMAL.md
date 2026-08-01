# 给 B：M4-A4.6 query cache / backward boundary 修复后的 WSL 正式矩阵

你是 B 模型。本轮只负责机械执行、artifact 保全、独立审计和报告整理。A 已完成故障定位与实现修复；
你不得修改代码来“修绿”，不得调用子代理或再次委派任务。

项目尚未发布。继续使用当前 M4 v1；不得增加 v2、兼容层、平行 schema、临时版本号或新算法名。
不得 commit、push、建 PR，也不得宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 本轮需要验证的 A 修复

上一轮 WSL `S100-F` 已经满足：

```text
RunOutcome / manifest = Complete / complete
abnormal_count = 0
invalid_meteorology = 0
reflection_limit = 0
lifecycle duplicate/nonmonotonic/state mismatch = 0
records / field sets = 101745 / 20349
peak RSS < 2 GiB
```

但仍有两个正式失败：

```text
particle-loop logical/executed/reuse = 17769 / 17769 / 0
unique/repeated = 17768 / 1

S50-F = 685771 ms
S100-F = 1752433 ms
ratio  = 2.5554201037955817 > 2.4
```

A 已完成三项最小修复：

1. **exact transport cache admission**
   - bulk cache 仍可被 `Unclassified | Integrator | Output` 填充；
   - `OutputLifecycle`、boundary 和 population helper 仍可命中已有 exact cache；
   - 这些小查询 miss 后不再替换整批 transport cache；
   - 没有增加多级缓存、近似 key、容差复用或数值路径分支。
2. **forward/backward timed cohort 查询顺序**
   - forward exact-time groups 按物理时间升序；
   - backward exact-time groups 按物理时间降序；
   - 同刻 particle indices 保持输入稳定顺序；
   - 同一 batch 混合积分方向仍 hard fail。
3. **有限域高索引精确边界**
   - East 以及负纬度步长下的 South safe boundary 原本被错误要求再取一个域外高索引邻点；
   - 现对精确 high safe boundary 使用最后一个域内插值 cell、fraction=1；
   - 只接受原合同定义的精确 safe boundary；边界外坐标仍 `OutOfDomain`；
   - 没有 inward nudge、epsilon 位移、容差放宽或异常重分类。

冻结回归包括：

```text
lifecycle_query_does_not_displace_bulk_transport_cache
exact_transport_cache_reuses_full_batch_and_ordered_subset_exactly
transport_time_groups_follow_forward_direction_and_preserve_row_order
transport_time_groups_follow_backward_direction_and_preserve_row_order
transport_time_groups_reject_mixed_directions
exact_high_safe_boundary_uses_the_interior_cell
```

A 的实现侧证据（Windows，只用于判断实现，不冒充 WSL 正式验收）：

```text
forward 50k: 221450 ms
forward 100k: 518000 ms
forward ratio: 2.33912847143825 <= 2.4
forward 100k query: 17769 logical / 17763 executed / 6 reuse / repeated=0

backward 50k: 240178 ms
interior boundary births: 2462
boundary births terminated at exact birth time: 0
integrator query: 4936 expected / 4936 observed
particle-loop query: 4943 logical / 4937 executed / 6 reuse
unique=4937 / repeated=0
abnormal=0 / lifecycle valid
```

backward Windows evidence：

```text
target/m4-a4.6/a-query-cache-boundary-fixed-backward-windows/
source_tree_sha256 = 16b2989a05768d58585aa88499b40306d8231a13d453bad67afb70c93bea7bb2
```

注意：本 Prompt 文件加入工作树后，B 开始时的 source-tree identity 应产生新的合法 SHA；不得要求它与
上述 Windows 诊断 SHA 相同。B 只需保证自己的 preflight 和全部正式 cells 使用同一个冻结身份。

## 2. 绝对边界

从 `E:\flexpart\trajecta` 的用户当前工作树继续。不得 reset、clean、checkout、stash、revert、覆盖或
删除既有未提交改动。不得读取、修改、移动、删除或加入 Git 外层用户文件：

```text
E:\flexpart\origo-validation-v1.json
```

本轮不得：

- 修改任何 Rust/Python 源码、Cargo 文件、测试、合同、schema、资料或既有 Prompt；
- 修改 RK2、boundary、vertical、domain-fill、runner、SQLite、provenance、query cache 或 metrics；
- 加入新的 cache、batch 合并、矩阵近似、GPU 路径或其他性能实现；
- 放宽 finite、质量、lifecycle、normal/abnormal、digest、I/O、exact-query 或 scaling 门；
- 把 `invalid_meteorology`、`reflection_limit`、domain exit 改为其他分类；
- 提高 provenance dictionary caps：records 仍为 `128000`，field sets 仍为 `64000`；
- 提高正式 100k peak RSS 上限：仍为 `2,147,483,648 bytes`；
- 改 50k/100k 粒子数、3600 秒时长、600 秒步长、600 秒 scheduled output、worker 数、随机种子或资料；
- 删除失败 attempt、复制旧 passed artifact、在相同 attempt 内重试或自动重试科学/验收失败；
- 运行 1,000,000 粒子测试；本轮正式上限为 100,000；
- commit、push、建 PR 或宣称完成。

若任何一步必须改实现才能继续，立即停止并交 A。

## 3. 冻结开始身份与新 artifact root

测试前记录：

```powershell
git status --short --branch
git rev-parse HEAD
git diff --stat
python -c "import importlib.util,pathlib,json; p=pathlib.Path('tools/run_m4_a4_real_matrix.py'); s=importlib.util.spec_from_file_location('m4a4',p); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(json.dumps(m.source_tree_identity(),sort_keys=True))"
```

记录 Git HEAD、`source_tree_sha256`、文件数和 dirty paths。从 preflight 开始到矩阵停止前不得编辑仓库
文件；否则 binary/artifact identity 失效，立即停止。执行报告只能在全部运行结束后再写，并明确报告
文件本身是 post-run 文档改动。

旧 roots 全部只读保留，不得复用：

```text
target/m4-a4.6/memory-formal/
target/m4-a4.6/post-invalid-met-formal/
target/m4-a4.6/post-boundary-lifecycle-formal/
target/m4-a4.6/a-query-cache-diagnostic-windows/
target/m4-a4.6/a-query-cache-formal-windows/
target/m4-a4.6/a-query-cache-backward-windows/
target/m4-a4.6/a-query-cache-boundary-fixed-backward-windows/
```

本轮唯一新 root：

```text
target/m4-a4.6/post-query-cache-formal/
```

如果该 root 在开始前已经存在，不得覆盖；使用下一个清晰的全新 root，并在报告中说明。

## 4. 本地冻结门禁

依次运行并保留完整 stdout/stderr：

```powershell
cargo fmt --all -- --check
cargo test --offline -p trajecta-core --lib
cargo test --offline -p trajecta-met --lib
cargo test --offline --release -p trajecta-core --test m4_a2_real_data real_hybrid_ -- --ignored --nocapture
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
git diff --check
```

预期至少包括：

```text
trajecta-core --lib: 135 passed
trajecta-met  --lib: 161 passed
real hybrid replays: 5 passed
```

workspace 中既有 ignored 测试保持 ignored；不得启用百万点测试。任何非 ignored 失败即停止，不得修代码。

## 5. 新源码 WSL preflight

```powershell
python tools/run_m4_a4_real_matrix.py preflight `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-query-cache-formal `
  --stop-on-fail
```

只有编排器审计 `passed` 才能继续。必须同时满足：

- direct release binary、binary SHA、host/source identity 完整；
- `RunOutcome::Complete`、manifest `complete`、abnormal=0；
- numerical steps=6，scheduled output times=7；
- lifecycle valid，无 duplicate/nonmonotonic/state mismatch；
- SQLite integrity `ok`，terminal WAL=0；
- execute-phase reader/provider I/O 五项 delta 全为 0；
- preflight particle-loop query：`19 logical / 13 executed / 6 reuse`；
- `unique=13`、`repeated=0`；
- bundle、streaming semantic validation、content/SQL/canonical digest 全过；
- mass、finite、population、caps 和 artifact identity 全过。

preflight 失败时不得开始 50k/100k。

## 6. 先单跑 S50-F 与 12 分钟门

```powershell
python tools/run_m4_a4_real_matrix.py single `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --direction forward `
  --particles 50000 `
  --workers 4 `
  --artifact-root target/m4-a4.6/post-query-cache-formal `
  --resume `
  --stop-on-fail
```

除普通 hard gates 外：

```text
runner_run_milliseconds <= 720000
```

只使用 runner run-only 时间，不混入编译、外层 Python、审计或报告时间。若超过 12 分钟，停止，不运行
S100-F。

## 7. 正式 6-cell 矩阵

S50-F 通过后：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-query-cache-formal `
  --resume `
  --stop-on-fail
```

冻结顺序：

```text
S50-F  -> S100-F -> S50-B -> S100-B -> D100-F -> D100-B
50k/w4   100k/w4   50k/w4   100k/w4   100k/w1   100k/w1
```

不得使用 `--continue-on-fail`。首个失败后保留 attempt 并停止。

## 8. Scale-aware exact-query 硬门

正式 domain-fill 会产生不同数量的 interior boundary birth times，因此 50k/100k 不能错误要求固定
`19/13/6`。每格使用 harness 已冻结的公式：

```text
B = distinct interior_birth_time_count
I_expected = 2 * (6 + B)       # RK2 start + midpoint
O_expected = 7                 # start + six scheduled endpoints

particle_loop_logical  = I_expected + O_expected
particle_loop_reuse    = 6
particle_loop_executed = particle_loop_logical - 6
unique_exact_keys      = particle_loop_executed
repeated_exact_executions = 0
```

并要求：

- observed integrator logical == `I_expected`；
- observed output logical == `O_expected`；
- logical == executed + reuse；
- 每次 execute/reuse 均被计入；
- forward 和 backward 都必须 `reuse=6`、`repeated=0`；
- boundary、boundary-corners、lifecycle helper 的总 query 数可很大，但不得混入 particle-loop exact-key
  上限或伪装成 reuse。

任何 repeated exact execution 都是失败，不得用“只重复 1 次”接受。

## 9. Backward 高边界专项审计

对 S50-B、S100-B、D100-B 单列：

```text
interior_birth_time_count
inflow particle count
invalid inflow origin/direction count
boundary-origin particles terminated at exact birth time
observed/expected integrator logical requests
reuse / repeated exact executions
abnormal termination counts/reasons
lifecycle validity
```

必须满足：

- `invalid_inflow_origins == 0`；
- `invalid_inflow_birth_direction == 0`；
- boundary-origin particles不是整批出生即 `population_outflow`；
- exact birth-time termination count 应为 0；
- observed integrator query 数满足两次 RK2 公式；
- abnormal=0，lifecycle valid。

可用只读 SQLite 查询补充 exact-birth termination 计数；不得修改数据库或编排器。

## 10. 正式 aggregate 门

| gate | 判据 |
|---|---|
| completion | 6/6 `Complete`；manifest complete；abnormal=0 |
| S50-F runtime | run-only `<= 720000 ms` |
| scaling forward | `S100-F / S50-F <= 2.4` |
| scaling backward | `S100-B / S50-B <= 2.4` |
| RSS | 四个 100k cells 均 `<= 2 GiB` |
| SQLite | 四个 100k 主 DB 均 `<= 512 MiB`，integrity ok，terminal WAL=0 |
| query | 6/6 满足 scale-aware 公式、reuse=6、repeated=0 |
| execution I/O | 6/6 execute-phase 五项 delta 全 0 |
| determinism | 同方向 100k w1/w4 normalized content、SQL、canonical-output digest 相同 |
| lifecycle | 6/6 valid；无 duplicate/nonmonotonic/state mismatch |
| science/output | mass、finite、population、bundle validator、caps 和 digest 全过 |

exact SQLite SHA、bundle SHA、run ID 可以不同；只比较冻结 normalized digests。

## 11. 100k 专项证据

每个 100k cell 单列：

```text
RunOutcome / manifest status
initial / inflow / total particles
numerical steps / scheduled output times / output_event rows
normal and abnormal terminations by reason
lifecycle expected/actual rows and all mismatch counters
record_count / field_set_count / sample_count
bundle size/SHA/semantic validation
runner_run_milliseconds / process wall
peak RSS
SQLite / WAL / attempt size
execute I/O delta
query logical/executed/reuse/unique/repeated
interior_birth_time_count and expected/observed integrator logical
mass-ledger maximum fraction
normalized content/SQL/canonical-output digests
```

records 必须 `< 128000`，field sets 必须 `< 64000`；不得提高 caps。

## 12. 首个失败处理

首个失败至少保留并引用：

- attempt 路径；
- `command.json`、binary/source/host identity；
- stdout、stderr、GNU time、concurrent-reader JSON；
- `M4_A4_CELL_SUMMARY.json`、`cell-result.json`、phase/formal summary；
- manifest SHA/status/termination 分类；
- SQLite、WAL、bundle 是否存在及大小；
- 首个明确错误、return code；
- 后续哪些 cells 因 stop-on-fail 未运行。

dictionary、RSS、超时、数值异常、lifecycle、SQLite、digest、I/O、query 或 scaling 失败都不是
`external_blocked`。只有 WSL、资料或必需系统工具确实不可用且与实现无关时才可写 external_blocked。

## 13. 报告

全部执行结束后才新增：

```text
docs/engineering/TRAJECTA_M4_A4_6_B_POST_QUERY_CACHE_EXECUTION_REPORT.md
```

报告必须明确：

- 本轮由 B 机械执行，未使用 C，也未调用子代理；
- 未改实现、合同、schema、caps、容差、异常分类或版本；
- 未 commit/push；
- 开始 source identity 与所有运行 cell identity 是否一致；
- preflight、S50-F、正式六格的逐格状态；
- scale-aware query、两方向 scaling、RSS、SQLite、lifecycle、digest 和 backward 高边界审计；
- 若失败，首个失败与未运行 cells；
- 不宣称 M4-A4.6、M4-A4 或 M4 完成，交 A 最终裁决。

报告写完后只运行轻量检查：

```powershell
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
cargo fmt --all -- --check
git diff --check
```

不得因报告写入后的 source-tree SHA 改变而重跑矩阵；报告中说明该变化发生在运行结束之后。
