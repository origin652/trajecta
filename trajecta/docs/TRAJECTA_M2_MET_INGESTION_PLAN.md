# Trajecta M2 原生气象读取链实施计划

当前状态（2026-07-13，A 核心终审已完成，M2 尚未全部完成）：

- GRIB 双后端逻辑字段身份冻结为
  **`(physical_message_offset, field_index_in_message)`**。真实 CFSR 含一个物理消息
  多个逻辑字段，单独使用 offset 或解码器序号都不成立；
- CFSR pgbl 的 Rust/native 7 类字段真实差分已通过，包含同消息 U/V；
- 压力层统一为 Pa 且严格递增，即模型顶到近地面；非空、有限、正值、无重复，
  values/mask 使用同一层排列；
- Profile 扩展字段不再默认假设为 `m + Horizontal2D`，直接扩展字段必须显式登记
  unit/shape；CFSR 地形高度已登记；
- `geopotential_from_height` 正式冻结为 v0 白名单操作，使用
  `geopotential = 9.80665 * height`；
- NetCDF3 索引持有单一专用 reader 线程：文件只打开一次，变量首次全量读取后缓存，
  时次和层切片在内存完成；native-netcdf 仍明确拒绝静默回退。

NetCDF4/HDF5、NOAA PSL 多文件和真实 NetCDF 锚点继续暂停在后续任务，不属于本轮。

## 1. 目标与完成边界

M2 交付完整的本地原生读取链：

```text
Case 时段
  -> RunProfile
  -> DatasetLockBuilder
  -> ProfileCatalog
  -> MetReader
  -> MetCatalog
  -> 单时次 RawMetFrame
```

M2 完成必须满足：

- 支持 ERA5/flex_extract hybrid GRIB、ERA5 等压 CF-NetCDF、ERA5 hybrid
  CF-NetCDF4 转码、CFSR pgbl GRIB2、NOAA PSL NCEP/NCAR Reanalysis 1 多文件 CF-NetCDF
  （官方多文件兼容性）以及 CFSR 派生多文件装配夹具；
- 五类资料都支持 native 与 Rust 两套读取后端；
- `RawMetFrame` 包含直接 canonical 字段和 Frame 阶段派生结果；
- 提供库级集成测试和非稳定 `examples/met_ingest`；
- 不实现前后帧窗口、逐点插值、局地垂直柱和粒子运行，这些属于 M3/M4。

## 2. 公共合同

### 2.1 DatasetProfile 与计算图

- Profile 使用同构 YAML/JSON 文档，按规范化语义 JSON 计算 SHA-256；
- 官方 Profile 作为仓库 YAML 文件接受审查，构建时验证并嵌入二进制；
- RunProfile 通过 `profile_sources` 显式列出私人 Profile 文件或非递归目录；
- LockBuilder 自动精确匹配 Profile，不使用名称相似度、搜索顺序覆盖或科学猜测；
- 无匹配返回结构化 `ProfileDraft`，不自动写文件；歧义匹配失败；
- DatasetLock 记录自动选出的 Profile 名称和内容哈希；名称缺失报错，同名内容变化只警告；
- 单一来源无需声明顺序；多个来源只有在 Profile 显式声明主来源和有序回退时才允许；
- canonical 字段使用内置稳定类型；直接 namespaced extension 字段必须通过
  `extension_fields` 显式声明目标、规范单位和形状，禁止全局默认类型；
- v0 提供类型化表达式 DSL，编译到白名单 `GraphOp`，禁止循环、递归、用户函数和外部调用；
- `geopotential_from_height(height)` 是冻结的 v0 白名单函数，按标准重力常数
  `g0 = 9.80665 m/s²` 输出位势；
- M2 完整解析、类型检查并编译所有 v0 操作，只执行 Frame 阶段；
  Column、Tile 和 Sample kernel 留到 M3；
- 完全通用的专用科学语言作为未来目标，不进入 M2。

### 2.2 DatasetLock、命名数据根与缓存

- LockBuilder 只锁定 Case 时段以及去累计、前后插值所需缓冲帧；
- 重叠算例复用机器侧索引和哈希缓存，但各自生成不可变 DatasetLock；
- 建锁时完整计算 SHA-256；后续按稳定文件身份、大小和高精度时间戳复用缓存，
  同时保留强制重哈希入口；
- 气象载荷哈希不符立即拒绝，Profile 哈希不符只警告；
- DatasetBinding 提供命名数据根，LockedFile 使用 `root_id + relative_path`；
- 符号链接只有在解析后落入某个显式授权的命名根时才允许；
- 多个模拟可以并发只读同一载荷；机器缓存使用内容寻址、原子安装和短文件锁；
- 文件类型通过魔数识别，README、MD5 和普通索引等非气象文件只进入扫描摘要。

### 2.3 Reader、Inventory 与 RawMetFrame

- native 正式后端使用 ecCodes、netCDF-C/HDF5；
- Rust 备用后端必须完整支持五类内置 Profile；
- RunProfile 提供全局默认后端，DatasetBinding 允许显式覆盖，禁止运行时静默回退；
- 官方未来发行包内置固定原生库；M2 只要求 Linux/Windows CI 成功链接和测试；
- 系统原生库达到最低 API 版本即可运行，未进入认证矩阵时警告并记录实际版本；
- MetCatalog 只保存不可变索引和标准化元数据；
- FrameLoader 按 CapabilitySet 一次性解码一个时次所需字段，发布后的 RawMetFrame 不再触发 I/O；
- 二维数组规范为 `[y][x]`，三维数组规范为 `[level][y][x]`，数值使用 f64；
- 缺测使用独立有效掩膜，不使用 NaN、无穷值或魔法数表示状态；
- 固定压力层统一使用 Pa 且严格递增（模型顶到近地面），并要求非空、有限、正值、
  无重复；任何层重排必须同步作用于 values 和 mask；
- 每个字段记录 instantaneous、interval 或 accumulation 时间支持；
- 累计量起点、重置和 warm-up 规则由 Profile 显式声明，不从数值跳变猜测；
- hybrid A/B 系数按 Profile 统一单位、顺序和声明精度后生成垂直签名；
- 一个锁内 Profile、网格和垂直坐标必须稳定，跨产品升级点要求拆分数据集和运行；
- 多文件逻辑帧按文件内有效时次、变量身份、层型和网格签名组装，不依赖文件名；
- CF 是默认便利层，私人 Profile 可以完整声明非 CF NetCDF 布局；
- v0 只接受能精确转换为 UTC 的日历，拒绝 360_day、noleap 等非公历时间；
- GRIB native/Rust 差分以物理消息 byte offset 与消息内逻辑字段序号组成稳定键，
  `message_index` 仅表示单个解码器的遍历顺序，禁止跨后端对齐；
- classic NetCDF3 的一个不可变索引拥有一个专用 reader 线程和变量缓存；由于当前
  `netcdf3` crate 没有公开通用 hyperslab API，首次读取完整变量，随后只做内存切片；
- 只强制算法所需结构和数学不变量，不评价资料科学品质；
- 规则经纬度是 v0 唯一可查询网格，其它网格可识别和报告，但拒绝形成可查询帧；
  `GridBackend` 保留后续扩展点。

## 3. 实施顺序与 A/B 分工

### A：核心启动

1. 冻结 M2 API、所有权、错误边界和文档合同；
2. 实现 Profile schema、语义哈希、自动匹配、ProfileDraft 和内置 Profile 加载；
3. 实现 DSL parser、单位/形状/阶段推导、GraphValidator、GraphCompiler 和 Frame executor；
4. 实现命名数据根、DatasetLockBuilder、验证缓存和阶段门控诊断；
5. 实现标准化网格/垂直签名、多文件组帧、RawMetFrame 生命周期和 NetCDFAssembly；
6. 冻结 native/Rust 后端接口与差分验收合同；
7. 亲自打通真实 ERA5 hybrid GRIB 的 native/Rust 双后端链路。

### B：冻结接口上的扩展

- 完成 CFSR GRIB2 适配；
- 完成 native/Rust NetCDF3/NetCDF4 格式适配；
- 录入并测试另外三类 NetCDF Profile；
- 扩充 codec、普通诊断、小型夹具和数据清单；
- 不自行改变 hybrid、组帧、DSL、锁文件和后端一致性语义。

### A：最终终审

- 审计五类 Profile 的字段、单位、层型、时间语义和回退链；
- 检查双后端差分、跨平台结果、确定性、许可边界和真实数据矩阵；
- 全部门禁通过后才进入 M3。

## 4. 测试与验收

- 每次提交在 Linux/Windows 跑双后端快速夹具、fmt、Clippy、测试和 Rustdoc；
- 定时、发布前和手动任务从独立冻结数据发布下载五类真实资料并核对 SHA-256；
- ERA5 hybrid NetCDF4 由独立确定性工具离线生成，运行时不转码；
- native/Rust 差分要求字段身份、网格、层、形状、掩膜和时间支持严格一致，
  数值按源打包精度使用固定容差；
- 覆盖无 Profile、歧义来源、缺帧、缺字段、非法时间轴、损坏文件、网格/PV变化、
  哈希失配、外部根越界和缓存并发；
- 年度扫描使用 `inspect -> profile_match -> index -> assemble -> capability_validate`
  阶段门控，聚合独立根因并抑制继发错误；
- 不同线程数必须生成相同 DatasetLock、MetCatalog 和帧摘要；
- 性能验收要求流式扫描、同路径单次打开、按能力解码、内存有界和跨文件有界并行；
  记录吞吐和峰值内存，但不设置跨机器绝对速度线；
- `examples/met_ingest` 输出稳定 JSON 摘要，不输出巨大字段数组。

## 5. 当前实施状态（2026-07-13）

### 5.1 已完成的 A 模型锚点

- 真实链路已经贯通
  `DatasetLockBuilder -> InventoryBuilder -> FrameLoader -> RawMetFrame`；
- 已用 flex_extract 7.1 的真实 ERA5 资料 `EA18120100` 至 `EA18120121`
  验证 8 个三小时时次，而不是只测人工小夹具；
- 纯 Rust GRIB reader 已实现 `inspect`、`build_index` 和按字段 `decode`，并正确
  处理同一文件前部 GRIB2、后部 GRIB1 的混合 edition；文件匹配仍以首个容器
  edition 为 GRIB2，逐消息 edition 身份完整保留；
- hybrid PV 保留完整 138 个接口 A/B 系数，即 137 个完整模式层；资料数组只携带
  130--137 层时，拓扑保留完整坐标定义，同时把 8 个实际层作为
  `active_full_levels` 发布；
- transport capability 已发布 U、V、温度、比湿、表面气压、表面位势，以及原生
  hybrid 垂直速度共 7 个字段；一个 6x6、8 层真实帧的常驻载荷为 13,608 bytes；
- flex_extract 中 GRIB2 `discipline=0, parameterCategory=2,
  parameterNumber=32` 已确认是 eta/hybrid 坐标垂直速度，单位 `s-1`；不得把它
  伪装成压力垂直速度 `Pa/s`；
- `FrameLoader` 对每个唯一文件只建一次索引，按 Capability 解码，执行显式主来源/
  回退、单位转换、时间支持、mask、provenance 和 Frame 阶段计算图，发布后不再
  保留隐藏 I/O；
- 可选 feature `native-eccodes` 已接入 ecCodes 2.47.0。真实 7 字段的 reader 级和
  完整 Frame 级 Rust/native 差分均通过，最大允许误差为 `1e-12`；
- Windows ecCodes 的定义文件延迟解析存在并发竞争，已只在 Windows 上串行保护
  ecCodes FFI 调用；Rust 侧工作和其它平台不因此被全局串行；
- vendored `eccodes 0.13.4` 保留 Apache-2.0 许可证、修改声明和补丁清单；上游
  发布包没有 `NOTICE` 文件。

### 5.2 当前门禁基线

以下命令已经在当前工作区通过：

```text
cargo fmt --all --check
cargo check --offline --workspace --all-targets
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
cargo clippy --offline -p trajecta-met --all-targets --features native-eccodes -- -D warnings
cargo test --offline -p trajecta-met --features native-eccodes
```

native 测试使用项目隔离的 `.native/eccodes` 工具链；该目录是机器侧依赖并已忽略，
不属于仓库交付物。

### 5.3 B 落地后由 A 完成的终审与核心收尾

- `cfsr-pgbl-pressure-v0`：真实 `pgbl00` 00/06 UTC，37 层 Pa 拓扑，JPEG2000
  Rust 解码（`grib-reader/jpeg2000`），transport 7 字段 + 地形高度→位势派生；
- native ecCodes 开启官方 multi-field 枚举，以 `(offset, field_index_in_message)`
  对齐结构索引；真实 CFSR Rust/native 压力层、表面场与同消息 U/V 差分通过；
- GRIB 索引跳过分数 sigma/无 PV hybrid，避免污染压力层产品；
- 纯 Rust classic NetCDF3 reader（`inspect/build_index/decode`、CF 轴、日历拒绝、
  packing/mask、规则经纬网、压力/hybrid 拓扑、单 reader worker、首次变量缓存、
  精确时次选择和 values/mask 同步层重排）；
- feature `native-netcdf` 已接入 netCDF-C worker（Windows GNU + MSYS2 UCRT64 已差分通过）；未链接 netCDF-C/HDF5 时不会借用
  纯 Rust NetCDF3 路径伪装成 native；
- 压力层公共合同、显式 extension descriptor 和 `geopotential_from_height` 标准重力
  合同已经由 A 冻结并进入测试；
- `testdata/REAL_MET_MANIFEST.json` + `TRAJECTA_REQUIRE_REAL_MET` 开关约定；
- 非稳定 `examples/met_ingest`。

### 5.4 B 续作（2026-07-13 晚）

- 离线 `examples/grib_to_cf_netcdf`：真实 GRIB → classic CF-NetCDF3；
- 压力 CF-NetCDF3 锚点（CFSR pgbl 00/06 真值）+ `era5-cf-pressure-netcdf-v0` 正向链；
- hybrid CF-NetCDF4 锚点（EA18120100/06，nccopy 自 classic）+ 纯 Rust
  `netcdf-reader`/`hdf5-reader` 路径 + `era5-cf-hybrid-netcdf4-v0`；
- `native-netcdf` 真实 netCDF-C worker（专用线程，无静默 Rust 回退）；
- `NetCdfAssembly::assemble` 与 `cfsr-derived-cf-multifile-netcdf-v0` Profile；
- `REAL_MET_MANIFEST.json` 已登记压力/hybrid 输出哈希与转换命令。

### 5.5 进入 M3 前建议补强

- NOAA PSL 官方多文件整帧 inventory/FrameLoader 贯通与双后端差分；
- native-netcdf 与 pure-Rust NetCDF4 数值差分矩阵写入 CI；
- 跨平台 `TRAJECTA_REQUIRE_REAL_MET=1` 终审。

## 6. 明确延期

- 正式 `trajecta met probe`、PreparedWindow、域选择、局地柱和插值进入 M3；
- M3 开始前实现独立 GPL FLEXPART 数值 oracle，MIT 项目只读取其 JSON 基准；
- 运行时远程下载不进入 M2，只保留 DataProvider/未来获取模块接口；
- Gaussian、投影和旋转网格按独立 GridBackend 后续增加；
- 正式原生库发行包、签名和安装流程留到 M5；
- 当查询、粒子闭环和主要物理功能接近完成时，再设计 Trajecta/FLEXPART
  对比结果、差异分类和论文图表。

### 5.6 科学来源与多文件格式拆分（2026-07-14）

- **CFSR 科学真值**：NCEI/NOAA 官方 CFSR GRIB2（及由其确定性转换的 CF-NetCDF 锚点）。
- **多文件 NetCDF 官方兼容性验证**：NOAA PSL NCEP/NCAR Reanalysis 1 多文件 NetCDF
  （downloads.psl.noaa.gov/Datasets/ncep.reanalysis/），不是 CFSR。
- **CFSR 派生多文件夹具**：`cfsr-derived-cf-multifile-netcdf-v0` 明确标注为
  GRIB2 to CF-NetCDF 再按变量拆分的装配测试夹具，不得称为官方 PSL CFSR NetCDF。
- 原 PSL CFSR NetCDF 目录 Datasets/cfsr/ 已下线（HTTP 404），不再作为阻断项。
- Windows native-netcdf Rust/native 数值差分已在本机 GNU 工具链通过；详见
  docs/TRAJECTA_NATIVE_NETCDF_WINDOWS.md。
- PSL NCEP R1 官方成员垂直层异构：air/uwnd/vwnd/hgt=17 层，omega=12 层，
  shum=8 层；hgt.sfc 为静态单时次。当前不宣称完整 transport capability；
  仅对 17 层相容子集 + pres.sfc 声明 diagnostics。异构垂直/静态地形支持待 A 裁决。
- 不宣称 M2 完成。
