# 给 B：M4-A4.6 invalid-meteorology 修复后的 WSL 正式矩阵执行

你是 B 模型。本轮只做冻结后的机械验证、WSL 执行、artifact 审计和报告整理。A 已完成数值问题定位与
修复；你不得继续修改数值核心、输出实现、provenance 字典、合同、容差、版本或验收语义。

本项目当前没有发布版本。本轮继续使用现有 M4 v1，不得新增 v2、兼容层、临时版本号或平行 schema。
不得调用或委派其他代理。

## 0. A 已完成的修复与证据

原 `S100-F` 的两个 `invalid_meteorology` 来自两条同属模型顶边界的错误路径：

1. domain-fill 先在控制体列内抽样高度，再把粒子水平移动到列内实际经纬度；当地模型顶可能略低，
   旧实现没有按当地完整查询支持压缩粒子高度；
2. RK2 中点可能高于某个 bracketing endpoint 的模型顶、但仍低于目标时刻插值后的模型顶。查询正确
   返回 typed `AboveAvailableTop` / `AboveModelTop`，旧 integrator 又用另一套目标时刻 bounds 二次比较，
   错误地把它转成 `invalid_meteorology`，而不是交给连续边界路径。

A 已实现并保留：

- `AirMassSnapshot` 的完整水平 available-top 网格；
- 按粒子实际经纬度双线性采样 terrain、transport floor 和 available top；
- 当当地顶较低时，在保持 RNG、粒子 ID、粒子数和质量不变的前提下按原层内比例压缩几何高度；
- RK2 对带 bounds 的 typed top-exit 状态使用 continuous boundary bridge；
- 合成斜顶回归、bracketing endpoint 回归和 ignored 真实 midpoint 回放。

A 已删除无效的 before/after 顶高整格重算旁路，避免每次 domain-fill 重复静力积分。

已通过：

```text
trajecta-core --lib: 128 passed
trajecta-met derive::domain_fill: 7 passed
clippy -D warnings: passed
real_hybrid_model_top_midpoint_replay: passed
fmt / git diff --check: passed
```

A 的 100,000 粒子、ERA5-hybrid、forward、1 秒真实资料预检：

```text
status: complete
seeded/final rows: 100000 / 100000
numerical steps: 1
abnormal_count: 0
normal_count: 1 (population_outflow，合法正常终止)
```

证据：

```text
target/m4-a4.6/invalid-met-fix-preflight-5/
  m4-a2-real-era5-hybrid-forward/
    019f9c29-d314-7350-9171-a3a971392cd7/run-manifest.json

manifest SHA-256:
  ce64a52c8ef2422841bb0a2f20f3ac790f0dcf824ba951ff09353dda1e3e4784
```

这只是针对两个异常的短预检，不是 3,600 秒正式 `S100-F` 通过证据。

## 1. 工作树与绝对边界

从用户当前工作树继续：不得 reset、clean、checkout、stash 或覆盖既有未提交改动。开始先记录：

```powershell
git status --short --branch
git rev-parse HEAD
git diff --stat
python -c "import importlib.util,pathlib,json; p=pathlib.Path('tools/run_m4_a4_real_matrix.py'); s=importlib.util.spec_from_file_location('m4a4',p); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(json.dumps(m.source_tree_identity(),sort_keys=True))"
```

将开始时的 `source_tree_sha256` 冻结到报告。运行结束前不得编辑任何源码、Cargo 文件、工具、合同或
测试资料；否则当前 binary/attempt 证据失效，立即停止交 A。报告只能在全部执行结束后补写。

外层 `E:\flexpart\origo-validation-v1.json` 是用户文件：不得读取、修改、移动、删除、加入 Git，
也不得把其内容写入报告。

本轮不得：

- 修改 RK2、boundary、vertical、domain-fill、query、runner、SQLite 或 provenance 实现；
- 再提高 `MAX_UNIQUE_RECORDS=128000` 或 `MAX_UNIQUE_FIELD_SETS=64000`；
- 放宽 `invalid_meteorology`、normal/abnormal、lifecycle、finite、质量或 digest 门禁；
- 改粒子数、时长、步长、输出间隔、worker 数、资料或随机种子；
- 删除失败 attempt、覆盖日志、自动重试失败格；
- commit、push、建 PR，或宣称 M4-A4.6、M4-A4、M4 完成。

正式 100k 进程级 peak RSS 门仍是 `<= 2 GiB`。这是内存验收上限，不授权按失败结果继续扩大字典或
内存预算。

## 2. 开始前本地门禁

从 `E:\flexpart\trajecta` 运行：

```powershell
cargo fmt --all -- --check
cargo test --offline -p trajecta-core --lib
cargo test --offline -p trajecta-met derive::domain_fill
cargo clippy --offline -p trajecta-met -p trajecta-core --all-targets -- -D warnings
cargo test --offline --release -p trajecta-core --test m4_a2_real_data real_hybrid_model_top_midpoint_replay -- --ignored --exact
git diff --check
```

任一失败即停止，不修代码；保存完整 stdout/stderr，并报告 `needs_a_adjudication`。

再只读核对 A 的短预检 manifest：

- 文件 SHA 与上文一致；
- `status == complete`；
- `terminations.abnormal_count == 0`；
- 唯一 normal reason 为 `population_outflow`。

## 3. 新源码必须重跑 WSL preflight

invalid-meteorology 修复改变了执行源码，旧 `S50-F attempt-2` 的 binary/source identity 不得复用。使用
新的 artifact root，不混入旧 `target/m4-a4.6/memory-formal`：

```powershell
python tools/run_m4_a4_real_matrix.py preflight `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-invalid-met-formal `
  --resume
```

preflight 必须满足编排器全部 hard checks，尤其：

- direct release binary 与 source identity 完整记录；
- `RunOutcome::Complete`、manifest `complete`；
- `abnormal_count == 0`；正常 `population_outflow` 允许存在，但 lifecycle coverage 必须精确；
- 6 个 numerical steps、7 个 output events；
- SQLite integrity `ok`、终态 WAL 为 0；
- execute-phase reader/provider I/O delta 五项为 0；
- frozen exact-query gate 通过；
- provenance bundle、manifest、SQLite 与 normalized digests 可独立重算。

preflight failed 时不得开始正式矩阵，也不得修绿；保存 attempt 后停止交 A。

## 4. 正式 6-cell 矩阵

preflight passed 后运行唯一入口：

```powershell
python tools/run_m4_a4_real_matrix.py formal `
  --platform wsl `
  --wsl-distro Ubuntu-24.04 `
  --artifact-root target/m4-a4.6/post-invalid-met-formal `
  --resume `
  --stop-on-fail
```

冻结顺序必须保持：

```text
S50-F  -> S100-F -> S50-B -> S100-B -> D100-F -> D100-B
50k/w4   100k/w4   50k/w4   100k/w4   100k/w1   100k/w1
```

不得手工跳过 `S50-F`，因为旧通过证据的源码身份已失效。`--resume` 只允许复用本次新 root 内、相同
`source_tree_sha256` 且审计 passed 的格；不能复用旧 root 的 attempt。

默认首个失败立即停止。不要使用 `--continue-on-fail`，除非 A 之后另行书面授权。

## 5. S100-F 的重点裁决证据

本轮首先要证明旧两个异常确实消失，同时确认上一轮 dictionary 失败没有被隐藏。对 `S100-F` 单独列出：

```text
RunOutcome / manifest status
seeded particle count / final particle rows
numerical steps / output events
normal_count + 按 reason 分类
abnormal_count + 按 reason 分类
invalid_meteorology count
record_count / field_set_count / sample_count
provenance bundle 是否存在及其 SHA
peak RSS bytes 与采集方法
SQLite/WAL/整个 attempt 大小
reader I/O delta
query logical/executed/reuse/unique
mass-ledger maximum fraction
normalized content / SQL / canonical-output digests
```

通过要求：

- `RunOutcome::Complete`、manifest `complete`；
- `abnormal_count == 0`，因此 `invalid_meteorology == 0`；
- normal `population_outflow` 可以非零，不得把合法正常出域改成异常或反过来；
- 不再出现 `record dictionary exceeded hard cap 128000`；
- 不出现 `field-set dictionary exceeded hard cap 64000`；
- provenance bundle 完整生成并通过流式语义校验；
- peak RSS `<= 2,147,483,648 bytes`；
- 其余 A4 hard gates 全部通过。

如果 dictionary 再次超限、出现任意 invalid meteorology、RSS 超 2 GiB，或测试进程 exit 0 但审计失败，
该 cell 都必须写 `failed`，保留完整 attempt 后立即停止。不得提高 cap、改分类、删粒子或只写成功摘要。

## 6. 失败证据要求

首个失败至少保留并在报告引用：

- attempt 绝对/仓库相对路径；
- `command.json`、`binary-identity.json`、host/source identity；
- stdout、stderr、GNU time、concurrent-reader JSON；
- `M4_A4_CELL_SUMMARY.json` 与 phase summary；
- 若有 manifest，记录其 SHA、终态状态、termination 分类和 output error；
- SQLite 主库、WAL、bundle 是否存在及大小；
- 首个明确错误字符串和 return code；
- 哪些后续格因 stop-on-fail 未运行。

失败不是 `external_blocked`，除非是 WSL/资料/工具确实不可用且与实现结果无关。数值异常、dictionary cap、
RSS、超时、SQLite、digest 或 hard-gate 失败都写真实 `failed`。

## 7. 报告

全部执行停止后才可新增或更新：

```text
docs/TRAJECTA_M4_A4_6_B_POST_INVALID_MET_EXECUTION_REPORT.md
```

报告必须包含：

1. 开始与 binary 的 source-tree identity；
2. 本地门禁及 WSL preflight 结果；
3. 六格 passed/failed/not-run 表；
4. S100-F 的 invalid-meteorology、dictionary、RSS 和 provenance 专项表；
5. 每格 attempt、summary、manifest、binary 的 SHA；
6. 失败后的诚实停止点；
7. 明确写：`未 commit / 未 push / 不宣称 M4-A4.6、M4-A4 或 M4 完成`。

最后运行：

```powershell
python -m py_compile tools/run_m4_a4_real_matrix.py tools/monitor_m4_a4_wsl_cell.py
cargo fmt --all -- --check
git diff --check
git status --short --branch
```

报告完成后交 A 裁决；B 不进行下一轮修复。
