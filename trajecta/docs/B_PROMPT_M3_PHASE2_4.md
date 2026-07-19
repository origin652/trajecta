# 给 B 模型的 M3 Phase 2--4 返工 Prompt

你在 `E:\flexpart\trajecta` 工作。当前工作区包含 A/B 未提交修改；禁止 reset、checkout、覆盖他人改动，完成后不要自行 commit，也不要宣称 M3 完成。

## 已冻结的 A 合同，禁止回退

1. 公共 `Dimension` 已是 SI 指数向量；必须使用 `FieldRegistry::canonical()`，不得恢复私有 placeholder registry 或把复合单位标成 Dimensionless。
2. canonical sensible/latent heat flux 固定为向上为正。CFSR downward-positive 源通过 Profile 显式负号派生，provenance 必须保留 expression。
3. FRICV 和 U/V stress 是替代输入。有 FRICV 时不得仍强制读取 stress；无 FRICV 时才要求两个 stress。
4. 禁止从 frame spacing 推断统计区间。任何 interval 必须来自可核验的产品/消息语义。
5. Explain 默认关闭；`--explain` 才编译 `ExplainMode::Full`。不得恢复 always-on 分配，也不得在 CLI 重算科学权重。
6. 不得使用 NaN、0、99999、空字符串等魔法哨兵表达缺失或“未给 time”。

## 第一组：先修 CLI 流式合同

- 修复混合业务目录发现：真实 CFSR 目录会同时含 `pgbl/flxl/spllnl`，当前 `met probe --profile cfsr-pgbl-pressure-v0 --data-root <raw-dir>` 会盲检全部文件，并因无关产品网格不匹配而让整个锁构建失败。应使用 Profile-aware 的候选筛选/分组，或提供明确的 include/glob/manifest 合同；不得要求用户手工复制出 `pgbl` 专用目录，也不得静默忽略“本应匹配但损坏”的文件。增加真实混合目录正例和匹配文件损坏负例。
- 修复 `replay --input -`：当前 coverage 预扫描会消费 stdin，正式读取时 EOF。stdin 必须单遍流式；若未显式给 coverage，采用有界 spool 文件或其它不会保存全部记录的可靠方案。
- summary 不得用 `BTreeMap<i64, ()>` 保存所有时刻。设计有界统计；若 schema 必须输出精确 unique time count，则用有界外部归并/spool，不能假装常量内存。
- `time_unix == 0` 是合法 Unix epoch，不得当缺省哨兵。把输入 time 改为显式 `Option<i64>` 或等价 typed representation。
- 重写 large JSONL 测试：必须实际越过多个 1024-point chunk 并验证读取/写出峰值有界；不能在读取输入前故意用不存在 data root 早退来冒充流式测试。
- 对 Explain JSON 加 golden/contract 测试，至少覆盖关闭时字段缺省、pressure 路径、surface-layer 路径和逐字段 quality/provenance。

## 第二组：真实资料链

### CFSR

- 从 NCEI 官方源获取并冻结 2009-01-01 12 UTC，与已有 00/06 组成三个连续时次。
- 下载器要求 `.part`、HTTP Range 校验、size/SHA、原子完成、重跑幂等。
- 完整运行 lock → inventory → frame → strict TransportPlan → prepare → execute。
- 覆盖 00/06/12 整帧与 03/09 中间时刻、AGL/ASL/Pa、海面/平原/山地/高空、边界状态。

### ERA5 hybrid

- 官方 CDS/MARS、2018-12-01 00/03/06、完整 1--137 模式层、规则经纬网。
- 同时取得 Transport 与 NearSurfaceTransport 所需 3D、surface、10 m、2 m、roughness、PBL、flux，以及直接或可核验推导的 u*/L 输入。
- 不得用现有 130--137 flex_extract 子集冒充正式全层锚点。

### ERA5 pressure

- 官方 CDS NetCDF4、2018-12-01 00/06/12、37 压力层。
- 必须包含逐层 geopotential、U/V/T/q/omega、surface pressure/geopotential 和完整近地输入。
- classic NetCDF3 只能作为由官方 NetCDF4 派生的格式等价夹具，不能冒充原始锚点。

每套资料均要求 `allow_estimated=false`，缺字段必须结构失败，不能静默 fallback。

## 第三组：差分、规模与 oracle

- pure-Rust/native ecCodes/native-netCDF：所有时次、所有字段、layout、mask、unit、temporal、quality、provenance、bounds、status、数值；同 index/worker 二次解码。
- 重复查询必须比较完整八列、有效位、quality、provenance、bounds、status 和 Explain（若启用），不能只抽 U/W/terrain。
- 百万点：显式小于 1 GiB 的预算，实际跨多个 chunk，1 线程/多线程、冷/热缓存、不同内部排列 bitwise identical；execute reader/provider 调用计数为零。
- 生成 FLEXPART oracle 原始结果和机器报告，但不要自行裁决或放宽容差；异常交给 A。
- Windows 与 Linux 都要保留实际命令、feature、环境和日志；“代码已就位”不算通过。

## 门禁与交付格式

必须实际运行并报告：

```text
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
TRAJECTA_REQUIRE_REAL_MET=1 cargo test --offline -p trajecta-met --tests
```

再按可用环境运行 native feature、百万点和跨平台矩阵。报告分为“已实现且实跑”“已实现但未跑”“未实现”“外部阻断”，列出准确命令和结果。不要修改 A 冻结公式、公共类型、字段符号、容差或许可边界；若发现矛盾，给 A 最小复现并停止该分支。
