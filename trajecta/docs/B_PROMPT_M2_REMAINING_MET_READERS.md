# 给 B 模型的 M2 剩余气象读取实现 Prompt

你负责在 A 已冻结的 Trajecta M2 合同上完成剩余 NetCDF 读取链。不要重做已经通过的
ERA5/flex_extract hybrid GRIB 和 CFSR pgbl GRIB2，也不要只挑其中一类 NetCDF：下列
三类真实资料、Rust/native 双后端、正向链和差分测试全部完成后，才可以交给 A 终审。

工作区根目录是 `E:\flexpart`，Rust workspace 是 `E:\flexpart\trajecta`。当前工作树
包含用户、A 和此前 B 尚未提交的改动；禁止 `git reset`、`git checkout --`、清理未跟踪
文件或覆盖不相关改动。不要提交代码，除非用户另行明确要求。

## 开始前必须完整阅读

1. `trajecta/docs/TRAJECTA_M2_MET_INGESTION_PLAN.md`
2. `trajecta/docs/TRAJECTA_MODULE_ARCHITECTURE.md`
3. `trajecta/crates/trajecta-met/src/io/{reader,grib,netcdf,inventory,frame_loader,lock_builder}.rs`
4. `trajecta/crates/trajecta-met/src/profile/{document,catalog,expression,graph}.rs`
5. `trajecta/crates/trajecta-met/src/{field,frame,vertical}/mod.rs`
6. `trajecta/crates/trajecta-met/profiles/`
7. `trajecta/crates/trajecta-met/tests/{real_era5_pipeline,real_cfsr_pipeline}.rs`
8. `trajecta/testdata/REAL_MET_MANIFEST.json`
9. `trajecta/vendor/eccodes/TRAJECTA_PATCHES.md`

先运行现有默认门禁和 `native-eccodes` 测试。基线不绿时先查明是否由你的环境造成，
不要在失败基线上继续扩展。

## A 已冻结、不得擅自改变的合同

- `MetReader` 保持 `inspect -> build_index -> decode` 三阶段；`SourceIndex: Send + Sync`；
- backend 必须精确执行用户选择。缺库或功能未链接时返回 `BackendUnavailable`，禁止
  Native/Rust 双向静默回退；
- GRIB 跨后端逻辑字段键是
  `(physical_message_offset, field_index_in_message)`。不得恢复遍历序号对齐，也不得只用
  offset；
- 固定压力层统一为 Pa，严格递增，即模型顶到近地面；必须非空、有限、正值、无重复；
  values 与 mask 必须使用同一层排列；
- hybrid 拓扑保留完整接口 A/B 系数和实际 `active_full_levels`，不得把数据层子集误当成
  完整坐标定义；
- canonical 字段类型由内置合同固定。直接 namespaced extension 字段必须在 Profile 的
  `extension_fields` 中显式登记 target、unit、shape；不得恢复“所有 extension 都是
  `m + Horizontal2D`”的默认值；
- `geopotential_from_height(height)` 已冻结为 v0 操作：
  `geopotential = 9.80665 * height`；
- 纯 Rust classic NetCDF3 已冻结为单 reader worker：索引创建时打开文件一次；由于
  `netcdf3` 没有公开通用 hyperslab API，变量首次完整读取并缓存，之后在内存按时次和
  层切片。不要改回每字段重新打开，也不要伪称这是磁盘 hyperslab；
- `FrameLoader` 每个唯一文件只建一次索引，按 Capability 解码；发布后的
  `RawMetFrame` 不再触发 I/O；
- 二维布局是 `[y][x]`，三维布局是 `[level][y][x]`，值为有限 `f64`，缺测放独立
  validity mask；
- 时间支持、单位转换、Profile 主来源/有序 fallback、provenance、锁文件哈希和阶段门控
  沿用现有实现；
- 不生成 FLEXPART 传统气象文件或配置；不得复制 GPL FLEXPART/flex_extract 源码进入
  MIT workspace。可以参考公开格式说明、论文和外部 oracle 的行为结果。

如果真实资料证明冻结合同存在矛盾，保留最小复现、文件哈希和诊断后交给 A 裁决。
不要顺手重写 Profile DSL、锁文件、FrameLoader、压力顺序或 hybrid 语义。

## 当前已经完成的基线

- 真实 ERA5/flex_extract hybrid GRIB：完整 PV、活动层子集、Rust/ecCodes 7 字段差分、
  8 个三小时时次正向链；
- 真实 CFSR pgbl GRIB2：37 个压力层、JPEG2000、地形高度派生位势、00/06 UTC 正向链、
  Rust/ecCodes 7 类字段差分，包含同一物理消息里的 U/V；
- 纯 Rust classic NetCDF3：CF 轴、时间、规则经纬网、压力/hybrid 拓扑、packing/mask、
  精确时次选择、层重排、单 reader worker 和变量缓存的小型确定性夹具；
- `native-netcdf` 当前只会明确失败，不会借纯 Rust 路径伪装 native。

这些是回归基线，不是你的待实现项。

## 必须完成的三类真实资料

### 1. ERA5 等压 CF-NetCDF3

- 从 ECMWF/ERA5 真实业务资料取得至少两个物理时次。若官方交付不是 classic NetCDF3，
  允许用仓库外的固定版本工具做一次确定性离线转换，但数值必须来自真实资料；
- 记录原始来源 URL/产品页面、许可或数据使用条款、attribution、输入哈希、转换工具与
  版本、完整命令、输出哈希；运行时和 `cargo test` 不得临时转码；
- 完成内置 Profile `era5-cf-pressure-netcdf-v0`，至少覆盖 transport 所需的 U、V、
  omega、温度、比湿、表面气压和表面位势；物理上分文件时由组帧层处理，不能靠变量
  改名掩盖；
- 验证压力层 Pa 严格递增、时次切片、CF 维序、packing、fill/missing mask、单位、
  provenance 和时间支持；
- 贯通 `DatasetLockBuilder -> InventoryBuilder -> FrameLoader -> RawMetFrame`；
- 纯 Rust 与真正 netCDF-C native 后端做元数据、布局、层、mask、时间和数值差分。

### 2. ERA5 hybrid CF-NetCDF4/HDF5

- 实现 Rust NetCDF4/HDF5 reader。Rust backend 不得调用 netCDF-C/HDF5，也不得内部
  转用 Native；只需可靠支持内置真实 Profile 实际使用的 HDF5/NetCDF4 子集；
- 至少正确支持真实夹具使用到的 chunking、deflate/shuffle、dimension scale、常见
  原子数值类型和 fill/packing 属性。不支持的 filter、type 或布局必须结构化拒绝，
  不能读错；
- 完成内置 Profile `era5-cf-hybrid-netcdf4-v0`；
- 以已经冻结的真实 ERA5 hybrid GRIB 锚点为输入，用仓库外确定性工具离线生成 CF
  NetCDF4 夹具；保留输入、工具、命令和输出哈希；
- 完整读取 `formula_terms`、A/B 系数、表面气压、137 个完整模式层定义和实际数据层
  子集，并与 GRIB 锚点的相同物理字段交叉核对；
- 同时实现真正的 netCDF-C/HDF5 native worker，并做 Rust/native/GRIB 三方差分。

### 3. 多文件 CF-NetCDF（官方 PSL NCEP R1 + CFSR 派生夹具）

科学来源与格式测试拆分：

- **官方多文件兼容性**：从 NOAA PSL 取得 **NCEP/NCAR Reanalysis 1** 多文件 NetCDF
  （`downloads.psl.noaa.gov/Datasets/ncep.reanalysis/`），不是 CFSR。原件不得重写
  属性；Profile 直接匹配原始 `dataset_title`。垂直层异构（如 shum/omega）与静态
  `hgt.sfc` 不得静默插值或补层，需诚实负例 + A 裁决。
- **CFSR 装配夹具**：`cfsr-derived-cf-multifile-netcdf-v0` 由官方 CFSR GRIB2 经
  `grib_to_cf_netcdf` 再按变量拆分得到，仅用于多文件 role/装配合同测试，不得称为
  官方 PSL CFSR NetCDF。
- 文件 role 必须由文件内变量、坐标、时间和全局属性确定，不能靠文件名猜；
- `NetCdfAssembly` 必须严格比较网格、压力层、时间轴、坐标值/属性摘要、变量形状和
  role。缺文件、重复 role、同 role 两个候选、坐标漂移或时次不一致都必须失败；
- 覆盖资料实际使用的 NetCDF3 或 NetCDF4 容器，并完成 Rust/native 整帧差分；
- 贯通至少两个时次的锁、inventory、组帧、Capability 校验和 `RawMetFrame` 发布。

## native-netcdf 实现要求

- 保留 feature 名称 `native-netcdf`，使用真正的 netCDF-C/HDF5；不要改名，也不要影响
  默认纯 Rust 构建；
- native handle 若不是 `Send + Sync`，使用专用 worker thread + channel。Trajecta 自有
  crate 继续禁止 `unsafe`，不得伪造 `Send`/`Sync`；
- 一个 immutable index 持有一个打开的 native handle；decode 使用真正 variable/time/
  level hyperslab，不得每字段重开或重扫整个文件；
- native index 和 Rust index 必须产生同样的 normalized metadata、grid、vertical、
  valid_times 和 exact field identities；
- classic NetCDF3 和 NetCDF4/HDF5 都要走 native 实现，不得对其中一个静默调用 Rust；
- 原生库版本、构建/安装步骤和实际运行版本写入文档或测试诊断；新增 vendored 组件时
  保留许可证、NOTICE（若上游存在）、来源版本和修改声明。

## NetCDF 共通科学与格式要求

- CF 轴至少处理 `standard_name`、`axis`、`units`、`positive`、`coordinates`、
  `bounds` 和 `formula_terms`；Profile 字段选择仍是精确身份，不做模糊猜测；
- 只接受能精确映射 UTC 的 `standard`、`gregorian`、`proleptic_gregorian` 等已冻结
  日历；明确拒绝 `360_day`、`noleap`、`all_leap` 等 v0 不支持日历；
- 正确应用 `_FillValue`、`missing_value`、`valid_min/max/range`、`scale_factor` 和
  `add_offset`。先按源值判断缺测，再解包；values 与 mask 始终同步切片和重排；
- 只接受冻结的 CF 维序 `(time?, level?, y, x)`。表面 `(time?, y, x)` 必须发布 2D，
  不得因文件有 vertical axis 就误判成 3D；
- 规则经纬网允许纬度升序或降序；非等距、旋转、Gaussian 或投影网格只识别并明确
  拒绝，不在此任务偷偷插值；
- source unit、时间区间/累计语义和 provenance 必须与 GRIB 路径同等完整；
- 对不存在的请求时次、重复时次、非法维序、坐标/层重复、非正压力、损坏或截断容器
  提供稳定错误测试。

## 真实资料与测试资产

- 扩充 `trajecta/testdata/REAL_MET_MANIFEST.json`，每个载荷记录资料类、官方 URL、来源
  页面、许可/使用条款、attribution、大小、SHA-256 和本地相对路径；
- 大型二进制不提交 Git，下载到 `trajecta/target/test-data` 或环境变量指定目录；核心库
  运行时不联网；
- 可以增加独立下载/校验脚本，但必须先校验 SHA-256，再原子安装；
- 普通开发机缺真实资料时测试可以明确 skip；`TRAJECTA_REQUIRE_REAL_MET=1` 启用后，
  任一声明的真实资料缺失都必须令测试失败；
- 下载或离线转换失败不能用手写数组代替。小型人工夹具只用于错误边界和格式单测，
  不能充当真实正向验收。

## 必须通过的测试矩阵

对三类新增 NetCDF，以及两类现有 GRIB 回归，逐项覆盖：

1. 魔数识别与 `inspect`；
2. 唯一 Profile 精确匹配，以及无匹配/歧义匹配；
3. index 的网格、垂直、有效时次、变量身份和文件生命周期；
4. transport 所需每个直接字段的精确 decode；
5. `DatasetLockBuilder -> InventoryBuilder -> FrameLoader -> RawMetFrame`；
6. Rust/native 元数据严格相等，数值按源精度采用固定且有依据的容差；
7. mask、单位、时间支持、provenance、形状和常驻字节摘要；
8. 缺字段、重复身份、非法日历/维序、坐标变化、压力/PV 变化、哈希失配和损坏文件；
9. 多文件缺 role、重复 role、时次不一致和坐标漂移；
10. 默认并行测试通过，不能靠 `--test-threads=1` 隐藏 native 并发问题；
11. 相同输入在不同线程数下生成相同 lock、catalog 和 frame 摘要；
12. 强制真实资料开关下，不能出现“全部 skip 但门禁绿色”。

不得只测文件能打开，不得只测 `inspect`，不得让 native 输出与自己比较，也不得把
缺资料的提前 `return` 当作发布级验收。

## 最终门禁

至少运行并逐条报告：

```text
cargo fmt --all --check
cargo check --offline --workspace --all-targets
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
```

配置 ecCodes 与 netCDF-C/HDF5 后运行：

```text
cargo clippy --offline -p trajecta-met --all-targets \
  --features native-eccodes,native-netcdf -- -D warnings
cargo test --offline -p trajecta-met --features native-eccodes,native-netcdf
```

最后在资料齐全且启用 `TRAJECTA_REQUIRE_REAL_MET=1` 的环境重跑完整矩阵。

## 交付说明

完成后列出：

- 修改文件、新依赖和 feature；
- 三个新增/完成的 Profile 及字段、单位、层型、时次和容器；
- 每类资料的来源、许可/条款、哈希和获取/转换方式；
- Rust/native（以及 hybrid GRIB 交叉锚点）的最大数值差和容差依据；
- 每类正向链的 frame 数、字段数、形状、垂直层和 mask 摘要；
- 全部门禁结果；
- 仍不支持的 HDF5/NetCDF4 特性，必须具体列出，不能笼统声称“支持所有 NetCDF4”；
- 任何需要 A 裁决的问题，附最小复现和证据，不要自行改变冻结合同。
