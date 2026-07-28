# TRAJECTA M5-A2E A 交付报告

日期：2026-07-29

## 1. 结论

M5-A2E 的 Windows 侧结构收口、去重、死代码和兜底审计已完成，完整 workspace、真实 CFSR runtime contract、clippy、doc、A0 validators、fmt 与 diff-check 均通过。

当前不能宣称 M5-A2E 正式完成：本机 wsl --list --quiet 无任何发行版，冻结要求中的 WSL local IPC、job、CLI 和真实 runtime contract 无法复跑。该项记为 external_blocked，不以 Windows 结果替代。

本轮不宣称 M5 完成、发布完成或跨平台验收完成。

## 2. 范围与约束

基线提交：

    819eb41 feat: establish M5 control plane through A2

本轮严格限制为：

- 拆分过大的 runtime/catalog 职责；
- 删除重复实现、无价值 wrapper、一次性抽象、死 plumbing 和可证明不可达的兜底；
- 复用既有 helper 和 typed contract；
- 不改变公开 CLI、Rust/job/schema 合同、状态机、错误时序、磁盘产物或数值行为；
- 不创建 v2、fix2 或平行实现；
- 不修改 crate 版本，全部保持 0.0.0；
- 不修改 E:\flexpart\origo-validation-v1.json；
- 不使用子代理，也未把 A2E 交给 B。

## 3. 量化结果

从 819eb41 到当前 A2E 代码头：

    12 个独立 cleanup commits
    38 个已关闭 pattern instances
    14 个 crates 下的文件发生变化
    774 insertions / 974 deletions
    生产代码净删 200 行

高优先级大文件的 physical line 变化：

| 模块 | 基线 | 当前 | 说明 |
|---|---:|---:|---|
| catalog.rs | 2474 | 2113 | SQLite row codec/schema 已移出 |
| catalog/storage.rs | 0 | 383 | 私有持久化边界；catalog family 合计 2496 |
| runtime.rs | 1947 | 1722 | OS worker process 实现已移出 |
| runtime/process.rs | 0 | 147 | 私有宿主进程边界；runtime family 合计 1869 |
| project.rs | 2391 | 2313 | 净删 78 行 |

catalog family 因明确私有可见性和模块 import 合计增加 22 行，但主生命周期文件减少 361 行；runtime family 总体净删 78 行。三个高优先级 family 合计从 6812 行降至 6678 行。

## 4. 已关闭的 code-humanizer pattern

对应提交：

    f5235ff  remove no-value output wrapper
    650f437  remove single-use invocation trait
    993e97d  remove dead CLI plumbing
    e14d04b  consolidate duplicate command results
    9d796ce  reuse existing helpers and types
    4138dbf  consolidate worker setup failures
    a3b0469  separate catalog storage responsibility
    005f3a1  remove one-shot invocation carriers
    185a62d  remove impossible fallback branches
    d5a08d6  remove discarded serialization errors
    6fe9c30  reuse typed data-plan validation
    6ff3c4f  separate worker process responsibility

主要效果：

- 删除纯转调 wrapper、单实现 trait、一次性 invocation struct 和未使用参数；
- 合并 CLI outcome/error 转换及 5 段相同 worker setup failure 处理；
- 复用已有 timestamp、路径验证、reader backend 和 Timestamp 反序列化合同；
- 将 SQLite schema/row codec 与生命周期逻辑分开；
- 将 OS worker 启动、探测、终止与 daemon/worker 生命周期决策分开；
- 删除 is_err() 后仍假设 error 不存在、固定父目录回退到 .、合法 JSON 值静默变 null 等不可达兜底；
- 删除 9 个调用点构造后立即丢弃的 serialization error plumbing，同时保留原机器输出行为。

没有新增注册表、factory、未来扩展接口、配置旋钮或公共 API。

## 5. 保留的 fallback 审计

以下不是可安全删除的无意义兜底，或删除会改变冻结行为，因此本轮保留。

### 5.1 明确的产品/信任边界

- config set / project set：输入不是合法 JSON 时按字符串处理，是冻结的人类渐进配置语义；
- 未指定 event cursor 时从 0 开始、可选 event message 显示为空，是公开 CLI 语义；
- reader backend 的 binding override 缺失时使用 profile backend，是 RunProfile 继承语义；
- 物理内存发现失败时使用保守配置默认值，运行时可用内存探测失败时暂停新调度；
- worker process probe 失败不能证明进程死亡，因此恢复路径保守视为仍存活；
- 原子写、force-stop、shutdown 和临时文件清理中的 best-effort 分支用于保留主错误或 forensic。

### 5.2 行为风险项：只记录，不在 A2E 静默修改

- runtime::serialize_or_null：异常时输出 null 是现有可观察行为；改为诊断会改变 machine output；
- project::same_path：两次 canonicalize 都失败时的相等语义可能值得后续修正，但属于错误行为变更；
- selector 读取路径中的连续 .ok()? 会折叠 path/I/O/YAML 错误，修正会改变错误类型和时序；
- worker child try_wait() 错误当前不移除 child，改为上抛会改变 daemon resilience；
- config/project/data-lock 的原子替换和 cleanup 错误聚合语义并不相同，未强行抽成一个 helper；
- ProjectState 当前没有仓内调用，但属于公开 Rust 类型，A2E 不删除公开合同。

这些项不能仅凭“看起来多余”删除；如需改变，应先冻结新的错误合同并用独立功能/修复提交处理。

## 6. Windows 最终门禁

最终代码头通过：

    cargo test --offline --workspace
      521 passed / 11 ignored
      包含 m5_a2_runtime_contracts：2 passed
      包含真实 CFSR daemon → worker → SQLite/provenance/manifest 链

    cargo clippy --offline --workspace --all-targets -- -D warnings
    cargo doc --offline --no-deps
    python tools/validate_m5_a0_contracts.py
    python tools/validate_m4_a0_contracts.py
    cargo fmt --all -- --check
    git diff --check

    全部 passed

最终一次真实 runtime contract 中，长测试约 81.6 秒完成。

## 7. WSL 状态

    wsl --list --quiet
    # 无输出

因此尚缺：

- WSL local IPC tests；
- WSL trajecta-job tests；
- WSL trajecta-cli lib tests；
- WSL m5_a2_runtime_contracts 真实 CFSR 进程链。

WSL 恢复后应在相同 source identity 上复跑以上四组；通过前不关闭 M5-A2E。

## 8. 工作树与交付状态

代码 cleanup 已按 pattern class 分成独立提交。报告写入后，工作树预期只剩用户既存且禁止触碰的：

    ?? origo-validation-v1.json

本报告不宣称 M5-A2E、M5 或发布完成；交后续跨平台复验与最终裁决。
