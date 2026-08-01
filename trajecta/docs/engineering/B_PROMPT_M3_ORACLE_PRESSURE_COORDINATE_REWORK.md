# 给 B：M3 Oracle pressure-coordinate P0/P1 返工

你在 `E:\flexpart\trajecta` 工作。当前工作树含大量 A/B 未提交修改，全部视为用户
资产：禁止 reset、checkout、清理或覆盖无关修改。本轮不要 commit，不宣称
M3/A2 完成，不修改 frozen query、registry、Trajecta 算法或容差。

先完整阅读：

- `docs/engineering/TRAJECTA_M3_ORACLE_PRESSURE_COORDINATE_A_REVIEW.md`
- `docs/engineering/TRAJECTA_M3_TOLERANCE_AND_ORACLE_CONTRACT.md`
- `docs/engineering/TRAJECTA_M3_ORACLE_COMPLETION_WSL_B_REPORT.md`

## 一、P0：修正 pressure-coordinate 科学路径

### 1. 垂直变换

- ERA5 pressure 与 CFSR pressure 必须调用冻结 FLEXPART
  `verttransform_gfs`。
- ERA5 hybrid 保持 `verttransform_ecmwf`。
- pressure runner、raw oracle provenance、README、build/nm 证据必须写实际路径，
  不得再硬编码 `verttransform_ecmwf`。

### 2. 拆开身份维度

不要再用单个 `TRAJECTA_ORACLE_METDATA_FORMAT` 同时表达资料族、坐标和 PBL 模式。
实现等价的显式配置，至少能审计：

- `source_family=era5|cfsr`；
- `vertical_coordinate=pressure|hybrid_eta`；
- `pbl_height_mode=richardson_diagnosed|official_prescribed`。

冻结组合：

- ERA5 hybrid：ERA5 + hybrid_eta + Richardson diagnosed；
- CFSR：CFSR + pressure + official HPBL prescribed；
- ERA5 pressure：ERA5 + pressure + official ERA5 BLH prescribed。

允许在 GPL adapter 内把冻结 FLEXPART 的 NCEP pressure-coordinate 分支机械拆成
显式参数，但不能改变公式、常量、循环边界或失败语义。ERA5 pressure 不得在输出
身份上冒充 NCEP，也不得冒充 native ECMWF ETA reader。

pressure-coordinate calcpar 必须：

- 按 local surface pressure 找第一个地上 pressure level；
- Obukhov/Richardson 从该层开始；
- prescribed BLH/HPBL 保留为 `hmix`；
- Richardson 仍只负责冻结实现原有的 `wstar/hmixplus` 等路径；
- 任何 `ierr < 0` 仍 hard fail，禁止 hmixmin/旧结果/零值回退。

### 3. ERA5 pressure adapter 身份

- raw artifact 明确记录 `build_mode=pressure_meter_adapter`（或等价、无歧义的冻结
  名称）及上述三维身份。
- `native_anchor`、`interpolated_common` 生成真实值；
- `surface_layer`、`modern_difference` 继续是 report-only reference。
- comparison 使用新的 provisional adapter variant；由于 registry 尚未由 A 发布，
  总体必须诚实为 `unvalidated`，不得复用旧 variant 写成 passed。

## 二、P0：数据与诊断完整性

### 4. finite/shape/存在性

- 两个 Fortran driver 使用 `ieee_arithmetic::ieee_is_finite`。
- 必需坐标、pressure/hybrid fields、surface PBL fields 和实际输出都做 shape + finite
  校验。
- 真实零合法；变量缺失、NaN、Inf、shape 错误均不得 complete。
- ERA5 pressure 必须有官方 `blh`；CFSR 必须有官方 HPBL 映射的 `blh`。

### 5. CFSR frozen cell diagnostics

- 保留 sea/plain/mountain 四角。
- 新增真正的 bilinear cell center：terrain、surface pressure、BLH、ishf、fricv。
- `center_underground` 使用 bilinear center surface pressure，不得使用最近角点。
- mountain 预期地形四角约
  `5509.447/5167.124/771.060/2686.144 m`，半格中心约 `3533.444 m`。
- query 文件及 SHA 不得改变。

## 三、P1：binary 证据链

### 6. build identity / nm

- pressure 与 ETA build 都生成 `compile_flags.txt`、`compile_identity.txt`、
  `nm_symbols.txt`。
- pressure nm 必须包含 `verttransform_gfs`、`interpol_wind`、
  `interpol_partoutput_val`、`interpol_pbl`、`oracle_calcpar`。
- ETA nm 必须包含 `verttransform_ecmwf` 及对应插值/calcpar 符号。
- runner 必须从实际 `--binary` 的 build 目录读取这些证据；缺任何一项 exit 2。
- harness SHA 覆盖 driver、adapter、runner、build script、`oracle_calcpar_mod.f90`。

### 7. 旧产物隔离

- 主入口运行前清理本轮 canonical 输出文件；不得读到旧 pressure/CFSR artifact、
  token 或 comparison report。
- CFSR 旧的 13 hard fails 作废，不得引用为本轮结果。

## 四、WSL 重建与验收输出

在 Ubuntu-24.04 WSL 中使用新的独立 build/work 目录：

1. 重建 pressure + ETA binaries；
2. 三族 raw oracle 各 75 条；
3. Richardson token 只允许 `0` 或诚实 `hard_fail`，silent fallback 必须 0；
4. canonical 与 audit 独立构建：比较 compile identity、nm、ELF `.text` SHA、raw
   records；
5. schema Draft 2020-12 零错误；
6. 运行 Windows 侧 MIT 消费，但 adapter variant 未注册时必须 `unvalidated`；
7. 暂不运行 WSL full+million，等 A 复核本轮 P0 后再跑。

最低门禁：

```text
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo test --offline --workspace
cargo doc --offline --workspace --no-deps
python -m py_compile tools/flexpart_oracle/*.py tools/run_m3_oracle_comparison.py
```

交付报告必须逐条列出：三族身份、实际 transform、75/75 状态、Richardson token、
query SHA、binary/harness/text SHA、CFSR bilinear center、schema/gates，以及明确未改
registry/query/算法/容差、未 commit、不宣称 M3 完成。
