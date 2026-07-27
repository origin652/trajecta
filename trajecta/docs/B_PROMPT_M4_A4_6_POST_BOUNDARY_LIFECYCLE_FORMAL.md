# 给 B：M4-A4.6 boundary / lifecycle 修复后的 WSL 正式矩阵

你是 B 模型。本轮只负责机械执行、artifact 保全、独立审计和报告整理。A 已完成中高难度故障定位与
源码修复；你不得修改任何实现来“修绿”，也不得调用子代理或把任务再委派出去。

项目尚未发布。继续使用当前 M4 v1；不得创建 v2、兼容层、平行 schema、临时版本号或新算法名来绕过
验收。不得 commit、push、建 PR，也不得宣称 M4-A4.6、M4-A4 或 M4 完成。

## 1. 本轮需要验证的 A 修复

上一轮 `S100-F` 的正式失败为：

```text
invalid_meteorology = 1
reflection_limit    = 2
duplicate scheduled output event = 1
nonmonotonic sample-event particle = 1
scheduled state mismatch = 2
```

A 已完成以下修复：

1. **surface reflection**
   - 旧实现围绕碰撞点 terrain 反射剩余终点；上升地形上，终点仍可能低于终点当地 terrain，随后只获得
     近乎零进度并耗尽四次反射上限。
   - 现实现按终点当地 terrain 与 clearance 计算反射终点；不改变四次反射上限、不改异常分类。
2. **restrictive transport top**
   - boundary 与完整 transport query 使用 before/after 两时次共同支持的 restrictive top。
   - RK2 start typed top-exit 保留 alive stationary proposal，交连续 boundary path 处理 `fraction=0`。
   - restrictive endpoint top 与 physical top 的浮点插值曾出现约 `2e-12 m` 反向交叉；现取两者的物理
     交集 `min(restrictive_top, physical_top)`，没有放宽容差。
3. **SQLite lifecycle identity**
   - 同一物理时刻只保留一个 output event；event kind 按
     `birth > termination > start > end > interval` 升级。
   - 同粒子同刻由 alive 变 terminated 时 UPDATE 既有 state 行，并写 termination；不新增重复 sample/event。

A 的冻结回归已通过：

```text
production_builder_hybrid_domain_fill_is_complete
surface_reflection_uses_endpoint_clearance_over_rising_terrain
typed_start_boundary_bridge_preserves_an_alive_stationary_proposal
temporal_bounds_use_the_restrictive_complete_transport_top
coincident_interval_and_termination_upgrade_one_event_and_one_state
coincident_birth_retains_priority_when_state_becomes_terminal
real_hybrid_restrictive_transport_top_start_replay
real_hybrid_surface_reflection_limit_replays
```

A 最终门禁记录：

```text
trajecta-core --lib: 132 passed
trajecta-met  --lib: 159 passed
real hybrid boundary replays: 5 passed
clippy -D warnings: passed
workspace tests: passed，保留 2 个既有 M3 million-point ignored
doc / py_compile / fmt --check / git diff --check: passed
```

这些是实现回归证据，不代替本轮新源码身份下的 WSL 正式矩阵。

## 2. 绝对边界

从 `E:\flexpart\trajecta` 的用户当前工作树继续。不得 reset、clean、checkout、stash、revert 或覆盖既有
未提交改动。不得读取、修改、移动、删除或加入 Git 外层用户文件
`E:\flexpart\origo-validation-v1.json`。

本轮不得：

- 修改 Rust/Python 源码、Cargo 文件、测试、合同、schema、资料或现有 Prompt；
- 修改 RK2、boundary、vertical、domain-fill、runner、SQLite、provenance 或 query 路径；
- 放宽 finite、质量、lifecycle、normal/abnormal、digest、I/O 或 exact-query 门禁；
- 改四次反射上限，或把 `reflection_limit` / `invalid_meteorology` 改成 normal；
- 提高 provenance dictionary caps：records 仍为 `128000`，field sets 仍为 `64000`；
- 提高正式 100k peak RSS 上限：仍为 `2,147,483,648 bytes`；
- 改粒子数、3600 秒时长、600 秒步长、600 秒 scheduled output、worker 数、随机种子或 ERA5-hybrid 资料；
- 删除失败 attempt、复用旧源码 identity 的 passed artifact、自动重试科学/数值/验收失败；
- 运行 1,000,000 粒子门禁；本轮正式上限就是 100,000 粒子；
- commit、push、建 PR或宣称完成。

若任何一步需要改实现才能继续，立即停止并交 A，不要自行修复。

## 3. 冻结开始身份

在任何测试前记录：

```powershell
git status --short --branch
git rev-parse HEAD
git diff --stat
python -c "import importlib.util,pathlib,json; p=pathlib.Path('tools/run_m4_a4_real_matrix.py'); s=importlib.util.spec_from_file_location('m4a4',p); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(json.dumps(m.source_tree_identity(),sort_keys=True))"
```

把开始时的 `source_tree_sha256`、Git HEAD、dirty paths 和文件数写入执行日志。此后直到正式矩阵停止前，
不得编辑仓库文件；否则 binary 与 artifact 身份失效，立即停止。报告只在全部执行结束后写。

旧 artifact root 全部只读保留，不得复用：

```text
target/m4-a4.6/memory-formal/
target/m4-a4.6/post-invalid-met-formal/
```

本轮使用唯一新 root：

```text
target/m4-a4.6/post-boundary-lifecycle-formal/
```

## 4. 本地冻结门禁

依次运行并保存完整 stdout/stderr：

```powershell
cargo fmt --all -- --check
cargo test --offline -p trajecta-core --test m4_a2_domain_fill production_builder_hybrid_domain_fill_is_complete -- --exact
cargo test --offline -p trajecta-core --lib
cargo test --offline -p trajecta-met --lib
cargo test --offline --release -p trajecta-core --test m4_a2_real_data real_hybrid_ -- --ignored --nocapture
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
git diff --check
```

`cargo test --offline --workspace` 中两个已冻结的 M3 百万点测试保持 ignored 是正常状态；不得手工启用。
任一其他失败即停止，保存日志并写 `needs_a_adjudication`；不得改代码。

## 5. 新源码 WSL preflight

本轮源码身份与历史 attempt 不同，必须重新运行：

```powershell
python tools/run_m4_a4_real_matrix.py preflight `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-boundary-lifecycle-formal `
  --resume `
  --stop-on-fail
```

只有编排器审计 `passed` 才能继续。必须同时满足：

- 直接 release binary、binary SHA 与 source-tree identity 完整记录；
- `RunOutcome::Complete`、manifest `complete`、`abnormal_count == 0`；
- 6 个 numerical steps、7 个 scheduled output times；
- lifecycle audit `valid == true`，无 duplicate、nonmonotonic、state mismatch 或无关 lifecycle row；
- 正常 `population_outflow` 可以存在，其终止粒子不要求在之后 scheduled time 重复写 state；
- SQLite integrity `ok`，终态 WAL 为 0；
- execute-phase reader/provider I/O 五项 delta 全为 0；
- exact particle-loop query：19 logical / 13 executed / 6 reuse，unique=13、repeated=0；
- provenance bundle 生成，streaming semantic validator、content/SQL/canonical-output digest 均通过；
- mass、finite、population 和 artifact identity hard gates 全部通过。

preflight failed 时不得开始 50k/100k；保留完整 attempt 后交 A。

## 6. 先单跑 S50-F，执行 12 分钟门

为避免编排器在 S50-F 性能未达标时自动进入 100k，先运行冻结单格：

```powershell
python tools/run_m4_a4_real_matrix.py single `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --direction forward `
  --particles 50000 `
  --workers 4 `
  --artifact-root target/m4-a4.6/post-boundary-lifecycle-formal `
  --resume `
  --stop-on-fail
```

除所有普通 hard checks 外，读取 cell summary 中的 `runner_run_milliseconds`。冻结的 A4.6 run-only 性能门为：

```text
runner_run_milliseconds <= 720000
```

不要把编译、外层 Python、artifact 审计或报告时间混入 run-only 门。若超过 12 分钟，即使 cell 其余审计
通过，也停止并诚实报告 performance failed；不得继续 S100-F。

## 7. 正式 6-cell 矩阵

S50-F 及 12 分钟门通过后，运行：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-boundary-lifecycle-formal `
  --resume `
  --stop-on-fail
```

`single` 使用 formal mode，因此 `formal --resume` 应只复用同一新 root、相同 source identity、完整审计
passed 的 S50-F；不得重新跑或从旧 root 复制它。

冻结顺序：

```text
S50-F  -> S100-F -> S50-B -> S100-B -> D100-F -> D100-B
50k/w4   100k/w4   50k/w4   100k/w4   100k/w1   100k/w1
```

默认首个失败立即停止。不得使用 `--continue-on-fail`，除非 A 后续书面授权。

正式 aggregate 门：

| gate | 判据 |
|---|---|
| completion | 6/6 `Complete`；manifest complete；abnormal=0 |
| S50-F runtime | run-only `<= 720000 ms` |
| scaling forward | `S100-F / S50-F <= 2.4` |
| scaling backward | `S100-B / S50-B <= 2.4` |
| RSS | 四个 100k cell 均 `<= 2 GiB` |
| SQLite | 四个 100k 主 DB 均 `<= 512 MiB`，integrity ok，terminal WAL=0 |
| query | 6/6 为 19 logical / 13 executed / 6 reuse；unique=13；repeated=0 |
| execution I/O | 6/6 execute-phase 五项 delta 全 0 |
| determinism | 同方向 100k w1/w4 normalized content、SQL、canonical-output digest 相同 |
| lifecycle | 6/6 audit valid；无 duplicate/nonmonotonic/state mismatch |
| science/output | mass、finite、population、bundle validator 和 digest 全过 |

exact SQLite SHA、exact bundle SHA 和 run ID 可以不同；不得错误要求它们跨 run 相同。只比较冻结的 normalized
digests。

## 8. S100-F 专项证据

无论通过还是首个失败，单列：

```text
RunOutcome / manifest status
initial + dynamically born + total particle count
numerical steps / scheduled output times / output_event rows
normal_count 及各 reason
abnormal_count 及各 reason
invalid_meteorology / reflection_limit
lifecycle expected rows / actual rows / valid
duplicate scheduled events
nonmonotonic sample-event particles
scheduled state mismatches
record_count / field_set_count / sample_count
provenance bundle 是否存在、大小、SHA、semantic validation
runner_run_milliseconds / process wall
peak RSS bytes
SQLite / WAL / attempt 总大小
execute I/O delta
query logical / executed / reuse / unique / repeated
mass-ledger maximum fraction
normalized content / SQL / canonical-output digests
```

通过要求包括：

- `invalid_meteorology == 0`；
- `reflection_limit == 0`；
- `abnormal_count == 0`；
- lifecycle audit 完全有效；
- records `< 128000` 且 field sets `< 64000`；
- peak RSS `<= 2,147,483,648 bytes`；
- 其余 runner hard checks 全部通过。

测试进程 exit 0 但审计失败仍写 `failed`。dictionary、RSS、超时、数值异常、lifecycle、SQLite、digest、
I/O 或 query 失败都不是 `external_blocked`。

## 9. 失败 artifact

首个失败至少保留并引用：

- attempt 路径；
- `command.json`、`binary-identity.json`、source/host identity；
- stdout、stderr、GNU time 与 concurrent-reader JSON；
- `M4_A4_CELL_SUMMARY.json`、`cell-result.json`、phase summary；
- manifest（若有）的 SHA、status、termination 分类、output error；
- SQLite、WAL、bundle 是否存在及大小；
- 首个明确错误字符串、return code；
- 后续哪些格因 stop-on-fail 未运行。

不得删除失败 attempt 或仅保留后来成功的结果。只有 WSL/资料/必需工具确实不可用且与实现无关时才写
`external_blocked`。

## 10. 报告与收尾

全部执行停止后才新增：

```text
docs/TRAJECTA_M4_A4_6_B_POST_BOUNDARY_LIFECYCLE_EXECUTION_REPORT.md
```

报告必须包含：

1. 开始 source-tree / Git / binary identity；
2. 本地门禁和 WSL preflight；
3. S50-F 12 分钟门；
4. 六格 passed / failed / not-run 表；
5. S100-F 专项证据；
6. scaling、RSS、SQLite、query、I/O、determinism、lifecycle aggregate；
7. 每个 attempt、summary、manifest、binary 的 SHA；
8. 若失败，明确停止点和交 A 裁决项；
9. 明确写：`未 commit / 未 push / 不宣称 M4-A4.6、M4-A4 或 M4 完成`。

最后只运行只读/轻量收尾：

```powershell
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
cargo fmt --all -- --check
git diff --check
git status --short --branch
```

交 A 后停止。B 不修复下一项故障，不 commit，不 push。
