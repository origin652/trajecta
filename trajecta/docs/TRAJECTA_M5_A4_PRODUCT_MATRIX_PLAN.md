# Trajecta M5-A4：跨平台产品矩阵与打包计划

状态：**A4 authority contract；Windows 与 Ubuntu 24.04 正式执行及 A 审阅已完成**

本阶段把已通过 M5-A3 的源码变成可从干净目录运行、可校验、可追溯的产品包，并用包内 release binary 完成正式 Windows/Linux 产品矩阵。A4 不改变数值核心，不进行 FLEXPART 对比，不修改 crate 版本，不创建 tag 或 GitHub Release。

## 1. 固定边界

- 平台：Windows x86_64 与 Linux x86_64；Linux 正式包必须在 Ubuntu 24.04 构建。
- 开发版本继续为 `0.0.0`；唯一计划内版本变更仍留在 M5-A5，一次改为 `0.1.0-alpha.1`。
- 每个平台只有一条正式打包路径，不建立 `v2`、`fix2`、legacy/new 等平行实现。
- 一个用户可执行文件同时包含 Rust reader 与 `native-eccodes`、`native-netcdf`；默认 reader 永远是 Rust，native 只能显式选择。
- native 动态库和 ecCodes definitions 属于产品 payload；ecCodes MEMFS 构建可用内嵌 definitions，但必须同时打包 `eccodes_memfs` runtime、写明 marker，并通过 native clean smoke。包不得依赖源码树、Cargo target 或构建机绝对路径才能启动。
- 输入气象资料不随软件包分发。
- M5 的 rerun/prune/forget 删除边界不变：只做 dry-run，产品包不得增加真实删除入口。
- 外层 `E:\flexpart\origo-validation-v1.json` 不属于仓库或产品包，禁止读取、修改、打包或提交。

## 2. 包名与布局

正式文件名固定为：

```text
trajecta-0.0.0-windows-x86_64.zip
trajecta-0.0.0-linux-x86_64.tar.gz
```

每个 archive 只有一个同名根目录（去掉扩展名），至少包含：

```text
trajecta[.exe]
LICENSE
README.md
BUILD-MANIFEST.json
SBOM.cdx.json
THIRD-PARTY-LICENSES.json
examples/minimal/...
native runtime libraries
share/eccodes/definitions/...  # real tree, or an MEMFS marker when embedded
```

archive 旁必须写 `<archive>.sha256`。ZIP/tar 成员必须排序、路径规范化、禁止绝对路径、`..`、设备文件和外部 symlink。`BUILD-MANIFEST.json` 不自我哈希；它枚举除此文件外的全部 payload，任何缺失、额外文件、大小或 SHA 不一致均 hard fail。

机器合同为：

- `testdata/M5_BUILD_MANIFEST.schema.json`；
- `testdata/M5_PRODUCT_CELL.schema.json`。

build manifest 必须记录 git HEAD、当前实际源码树 SHA、dirty paths、target triple、最低系统、Rust/Cargo、release profile、两项 native feature、默认 reader、native 组件、binary/SBOM/license identity 及完整 payload。

## 3. 供应链证据

- `SBOM.cdx.json` 使用 CycloneDX 1.5，来自 `cargo metadata --offline --locked` 的实际非 dev dependency 闭包，并加入 ecCodes、netCDF-C、HDF5 native 组件。
- `THIRD-PARTY-LICENSES.json` 列出相同闭包的名称、版本、来源和 SPDX/明确 license 表达式；缺 license 的组件禁止打包。
- native 组件至少为 ecCodes、netCDF-C、HDF5，版本与来源必须来自实际构建环境或显式冻结参数，不得猜测。
- archive、build manifest、binary、SBOM、license inventory 都必须保存 SHA-256。
- source identity 对仓库内 tracked 与 non-ignored untracked 文件的实际 bytes 计算，不能只用 git commit 代替 dirty working tree。

## 4. clean-extraction 硬门

每个平台把 archive 解压到源码树和 Cargo target 之外的全新目录，只通过包内 executable 运行：

1. 校验 archive `.sha256`、安全成员和单一根目录；
2. 重新计算 build manifest 的全部 payload identity；
3. `--help`；
4. `config init`、`config validate`；
5. `doctor --deep`；
6. 复制并验证 `examples/minimal`，确认“先配置、后放资料”为 configured/pending，不伪造 finalized；
7. 用冻结的三份 CFSR 文件完成小型默认前台真实 E2E；
8. 用同一包完成 `run --detach`、新 CLI 进程 `job wait/status/events`、daemon idle 后重启访问、`result inspect`、`result verify --full`、`result trajectory` 和自动 `run-report.md`；
9. Complete、abnormal=0、SQLite integrity、terminal WAL、provenance、digest、catalog/manifest/report identity 全部通过；
10. 测试后不得遗留 daemon/worker 进程。

使用 Python/Rust 验收程序不等于产品依赖 Python/Rust；被验收的 executable 及其运行时只能来自解压包。

## 5. 正式产品矩阵

所有 cell 使用同一平台 archive 内同一 binary SHA、同一 source tree SHA；每格使用独立 config、catalog、project 和 output root。

### 5.1 Rust reader 1k：每平台 18 格

```text
3 data families x 3 populations x 2 directions = 18
```

- data family：ERA5 pressure、ERA5 hybrid、CFSR pressure；
- population：release、dry-air domain-fill、stratospheric ozone；
- direction：forward、backward；
- 1,000 particles，1 worker，600 秒模拟，300 秒 maximum step；
- backend=`rust`。

两平台合计 36 格。

### 5.2 Rust reader 10k：每平台 6 格

- ERA5 pressure：release forward、air-mass backward；
- ERA5 hybrid：air-mass forward、ozone backward；
- CFSR pressure：ozone forward、release backward；
- 10,000 particles，4 workers，其余时间合同与 1k 相同；
- backend=`rust`。

两平台合计 12 格。

### 5.3 Native reader 1k：每平台 6 格

```text
3 data families x 2 directions x release = 6
```

1,000 particles，1 worker；ERA5 使用 native-netcdf，CFSR 使用 native-eccodes；不得静默回退 Rust。两平台合计 12 格。

正式总计 60 格。各 phase 默认按上列顺序串行执行；首个 hard fail 后停止该平台后续 cell，保留失败 attempt，不自动重试、不缩小粒子数、不改参数、不覆盖 artifact。

## 6. 每格硬门

每格至少同时满足：

- 进程退出 0、`RunOutcome::Complete` 对应 manifest `complete`；
- `terminations.abnormal_count == 0`；
- full verification 通过且 `run_success=true`；
- lifecycle 连续、finite/quality/population audit、mass ledger 全部合法；quality 汇总必须逐字段覆盖全部 state，仅允许 `ok` 与不超过正常终止数的同量 `missing`，provenance quality 只允许 `source | derived`，禁止 `estimated` 与未知值；
- SQLite `integrity_check=ok`、manifest row counts 一致、terminal WAL 不存在或 0 bytes；
- provenance bundle 语义、SQLite exact SHA、content SHA、SQL SHA、canonical-output SHA 从磁盘独立重算一致；
- job series/run/attempt、catalog、manifest、CLI envelope、自动 report 身份一致；
- report 可重复生成且不进入 scientific digest；
- package binary/build manifest/source identity 与 cell summary 一致；
- native phase 必须证明实际选择 native backend，缺 native runtime 是产品失败，不记 external_blocked。

跨平台 normalized digest 不预先强制 bitwise 相同；若不同，必须生成已有 M4 容差下的逐字段/集合差异证据，不能只写“平台浮点差异”。同平台同 case 的重复或 worker 对照若执行，则 normalized identity 必须遵守既有确定性合同。

## 7. A/B 执行边界

A 先完成：

- 本文件与两个机器 schema；
- 单一打包/校验实现；
- 一个 Windows 本地 package preflight 与一个小型 clean-extraction 真资料 smoke；
- 正式 cell harness、source identity 和 stop-on-fail 规则；
- 给 B 的完整冻结 Prompt。

B 之后只做机械工作：

- 在固定 Windows/Ubuntu 环境构建正式 package；
- 运行 clean-extraction 和 60 格矩阵；
- 首个 hard fail 停止并交 A；
- 汇总 hash、artifact index 和报告；
- 不修改合同、schema、数值核心、容差、异常分类、版本或删除语义；
- 不 commit、不 push、不自行宣称 A4/M5 完成。

A 读取实际 diff 和 artifacts 后给出 passed/failed/external_blocked 裁决。A4 通过仍不代表 M5 完成；M5-A5 的 FLEXPART 对比、单次 prerelease 版本变更和用户确认发布仍未进行。
