# B Prompt：M4-A4.5 重复矩阵、无观测 baseline 与 WSL profiler

你是 B。本轮只执行 A 已冻结的 M4-A4.5 机械采样与 artifact 整理，不做科学、架构或优化裁决。

仓库：`E:\flexpart\trajecta`
基线 HEAD：执行开始时自行记录，工作区预期为 dirty。现有改动属于 A/用户，必须完整保留。

## 0. 不可违反的边界

1. 不调用子代理。
2. 不修改任何 Rust、Python、schema、registry、Case、Profile、数值核心或输出格式。
3. 不修改 `E:\flexpart\origo-validation-v1.json`。
4. 不安装 `perf`、FlameGraph、Rust crate、Python 包或系统依赖；工具不可用时诚实记为 `external_blocked`。
5. 不增加缓存、不加 `BufWriter`、不改 JSON pretty 格式、不改 provenance、SQLite、boundary、query 或线程逻辑。
6. 不运行 50k/100k A4 formal matrix。
7. 不删除或覆盖旧 artifact；只使用下文三个全新 root，工具会创建 `attempt-N`。
8. 不 commit、不 push，不宣称 M4-A4.5、M4-A4 或 M4 完成。
9. 任一命令失败时保留 stdout/stderr/artifact；不得“修绿”。若是工具自身 bug，停止相关分支并写最小复现交 A。
10. 你只负责 executed/passed/blocked 的事实整理；热点解释、优化授权和完成裁决归 A。

## 1. 先记录身份并跑轻量门禁

在 PowerShell、`E:\flexpart\trajecta` 下执行：

```powershell
git rev-parse HEAD
git status --short
wsl.exe -l -v
cargo fmt --all -- --check
cargo test --offline -p trajecta-met performance
cargo test --offline -p trajecta-core --test m4_a4_real_perf --no-run
python -m py_compile tools/validate_m4_a4_5_performance.py tools/summarize_m4_a4_5_attribution.py tools/run_m4_a4_5_perf_profile.py tools/run_m4_a4_real_matrix.py
git diff --check
```

不要因为已有 dirty worktree 停止；只需把 `git status --short` 原样写入报告。

## 2. Instrumentation-on：3×(w1/w4)

执行唯一命令：

```powershell
python tools/run_m4_a4_real_matrix.py attribution --platform wsl --repetitions 3 --continue-on-fail --artifact-root target/m4-a4.5/b-matrix
```

预期生成六个 attempt：

```text
target/m4-a4.5/b-matrix/cells/wsl__era5-hybrid__forward__p1000__w1/attempt-1..3/
target/m4-a4.5/b-matrix/cells/wsl__era5-hybrid__forward__p1000__w4/attempt-1..3/
```

即使一个 attempt 失败，也必须让其余 attempt 继续，并保留每个 `cell-result.json`、stdout、stderr、summary、manifest 和 `A4_5_STAGE_TIMINGS.json`。

## 3. 逐个验证 timing artifact

```powershell
$failed = $false
Get-ChildItem -LiteralPath target/m4-a4.5/b-matrix -Recurse -Filter A4_5_STAGE_TIMINGS.json | ForEach-Object {
    python tools/validate_m4_a4_5_performance.py $_.FullName
    if ($LASTEXITCODE -ne 0) { $failed = $true }
}
if ($failed) { Write-Error 'one or more A4.5 timing artifacts are invalid' }
```

同时确认 timing artifact 数量恰好为 6。少于 6 必须写 blocker，不得把已有部分写成完整矩阵。

## 4. Instrumentation-off：w4 baseline ×3

以下同一命令连续执行三次；不要改成 attribution mode：

```powershell
python tools/run_m4_a4_real_matrix.py preflight --platform wsl --continue-on-fail --artifact-root target/m4-a4.5/b-baseline
python tools/run_m4_a4_real_matrix.py preflight --platform wsl --continue-on-fail --artifact-root target/m4-a4.5/b-baseline
python tools/run_m4_a4_real_matrix.py preflight --platform wsl --continue-on-fail --artifact-root target/m4-a4.5/b-baseline
```

这三次必须满足：

- mode=`preflight`；
- workers=4；
- `TRAJECTA_M4_A4_5_OBSERVE=0`，summary 内 `performance_attribution` 为 null；
- 每次 `Complete`、abnormal=0、execute I/O delta=0。

## 5. 聚合 scaling 与观测开销

```powershell
python tools/summarize_m4_a4_5_attribution.py --artifact-root target/m4-a4.5/b-matrix --baseline-root target/m4-a4.5/b-baseline --required-repetitions 3
```

必须保留：

```text
target/m4-a4.5/b-matrix/summary/M4_A4_5_PERFORMANCE_ATTRIBUTION.json
```

不要自行改变 3% 目标。若 `instrumentation_overhead.within_goal` 为 false，只报告真实值；不得修改计时器。

## 6. Digest 与正确性矩阵

从六个 instrumentation-on 和三个 baseline attempt 的 `run-manifest.json` 提取：

- status；
- provenance.content_sha256；
- provenance.sqlite_sql_sha256；
- provenance.canonical_output_sha256；
- provenance.sample_count；
- terminations.abnormal_count。

要求：

- 所有成功 attempt 的三类 normalized digest 分别完全一致；
- bundle SHA、SQLite file SHA 和 run_id 允许不同，不要误判；
- 当前冻结场景允许正常 `population_outflow`，不得恢复“必须 7,000 rows”的旧门；
- abnormal 必须为 0，manifest 必须为 complete。

将提取表写入报告即可，不写新的分析程序。如果 digest 不一致，标 `hard_failed` 并把首个不同的两个 manifest 路径列出，停止性能解释但继续保留 profiler 证据。

## 7. WSL CPU sampling profile

执行：

```powershell
python tools/run_m4_a4_5_perf_profile.py --artifact-root target/m4-a4.5/b-profiles
```

预期核心产物：

```text
perf.data
perf-report-inclusive.txt
perf-report-self.txt
perf-script.txt
profile-binary-sha256.txt
M4_A4_5_PERF_PROFILE_RESULT.json
```

若系统已有 `stackcollapse-perf.pl` 和 `flamegraph.pl`，还应有：

```text
perf-folded.txt
flamegraph.svg
```

若 `perf` 权限、WSL kernel、符号化或 FlameGraph 工具不可用：

- 不安装任何东西；
- 保留 `stdout.log`、`stderr.log`、已有 `perf.data`/text report；
- 报告精确错误、`perf_event_paranoid` 和失败命令；
- 状态写 `external_blocked`，不是 passed，也不是算法失败。

不要根据 profile 自行修改代码。只摘录 inclusive/self top 20；对 `write`、`pwrite64`、`std::fs::File::write_all`、WSL/9p/drvfs、serde、BTreeMap、SQLite、boundary 和 query 相关栈保留原始百分比与符号名。

## 8. B 交付报告

只新增/更新：

```text
docs/engineering/TRAJECTA_M4_A4_5_B_EXECUTION_REPORT.md
```

报告必须包含：

1. 明确责任：A 不调用子代理；B 由用户另行启动；B 只执行机械矩阵与 profiler；解释和裁决归 A。
2. HEAD、完整 dirty status、WSL distro/kernel、Rust/Cargo、CPU、`perf` 版本与 `perf_event_paranoid`。
3. 六个 instrumentation-on attempt 表：worker、attempt、run ms、runner ns、complete、rows、normal/abnormal、I/O、artifact 路径。
4. 三个 instrumentation-off baseline 表。
5. aggregator 的 1-worker median、4-worker median、speedup、top-level accounted fraction、观测开销和是否 ≤3%。
6. 三类 normalized digest 矩阵。
7. profiler inclusive/self top 20，或完整 `external_blocked` 证据。
8. 现有 primary hypothesis 的交叉证据，但不得代 A 裁决：

```text
ProvenanceBundleBuilder::finalize_after_sqlite
  -> stream_write_bundle
  -> lockstep_write_samples
  -> unbuffered DigestingFile::write_all
  -> /mnt/e artifact
```

9. implemented / executed / passed / blocked 四列状态。
10. 明确未运行 50k/100k、未优化、未 commit、未 push、未改 `origo-validation-v1.json`、不宣称任何阶段完成。

完成后停止，交 A 复验。不要继续尝试 buffering 或其他优化。
