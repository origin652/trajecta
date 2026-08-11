# Trajecta M6-A2 交付报告

日期：2026-08-11

结论：**M6-A2 的代码、正式资料锁、本地生产矩阵和全部源码门禁已经通过。GMTED2010 的公共 GitHub Release 尚未创建，因此普通用户下载闭环仍待发布授权；本报告不把计划中的下载地址写成已上线服务。当前停在 A2，不进入 A3。**

## 1. 本轮范围

M6-A2 完成以下纵向切片：

- 实现三维中尺度 Markov 速度及其局地时空方差查询；
- 将中尺度状态接入共同子步、中点运动、粒子 SoA、SQLite v2 和 full verify；
- 实现 GMTED2010 mean/std 的只读准备格式、全局 DatasetLock 和生产加载；
- 将次网格地形异常接入出生高度、边界入流、连续地面求交和边界层混合高度；
- 从 pressure 与 hybrid 温压高度柱计算局地干静力稳定度；
- 将 GMTED2010 纳入 project data-plan、finalize preflight、data lock 和资料获取助手；
- 建立三类真实气象资料、正反向、w1/w4 的生产运行矩阵；
- 保持 A1 边界层切片、M5 纯平流路径和现有 M4/M5 合同通过。

深对流、水汽交换、沉降、化学、排放和检查点恢复仍由后续阶段实现。Runner 遇到这些模块时继续在执行前返回稳定的阶段不可用诊断。

## 2. 三维中尺度 Markov

### 2.1 局地方差

气象查询在粒子周围使用以下支持点：

```text
2 个原生时间面
× 4 个水平角点
× 2 个原生垂直层
```

三个风分量分别计算非负加权方差。权重来自现有时间、水平和垂直插值几何，缺层、越界和无效角点保留 typed status。查询不会用零值补齐缺失资料。pressure level 与 hybrid level 使用各自完整的柱几何；hybrid 垂直速度沿已有原生坐标链计算。

稳定算法标识为：

```text
three_dimensional_local_variance_ou/v1
```

查询采用 prepare/execute 两阶段。prepare 固定时间窗、水平支持、垂直模板和 caller-order permutation；execute 不再读取源文件，并按冻结 chunk 边界并行计算。

### 2.2 OU 状态

相关时间由原生气象时间间隔确定：

```text
tau = correlation_interval_fraction × delta_t_met
default correlation_interval_fraction = 0.5
```

生产更新使用精确 OU 转移：

```text
r = exp(-dt / tau)
u_next = r u_previous + sqrt(1-r²) sigma xi
```

东西、南北和垂直三个分量使用固定且互不重叠的随机维度。随机键包含 seed、粒子稳定 ID、模块、宏步、子步和 draw index。局地方差为零时，对应状态精确回到零。

中尺度速度与解析风、边界层随机速度进入同一个中点运动。出生、midpoint 和 endpoint 状态使用已有固定 draw 分区；线程调度不进入随机键。

### 2.3 统计门

冻结 OU 测试使用 100,000 个独立粒子样本，并同时检查三个分量：

```text
|mean| <= 0.02 sigma
relative variance error <= 0.03
absolute lag-one correlation error <= 0.02
```

均值、方差和相关系数全部通过。常风支持另有精确零方差回归。

## 3. GMTED2010 准备格式与锁

### 3.1 来源身份

原始产品采用 USGS GMTED2010 30 arc-second mean elevation 与 standard deviation。来源清单保留 USGS 产品页、DOI、引用文本、原始归档成员、CRC32、归档大小和 SHA-256。

| 原始归档 | 字节 | SHA-256 |
|---|---:|---|
| `mn30_grd.zip` | 277,229,566 | `dfd0d6c6486f4da22109be6107c93a70ef9917a5b6f954cd82d87cf8b920149d` |
| `sd30_grd.zip` | 126,192,743 | `953b05060a44a35adc2e7834a2498f5c1283b9243cdbbce6aa5b2e8f20375bd4` |

USGS EROS 旧下载端点已经退役。本轮从其归档捕获取得冻结字节，并以 USGS 元数据和归档内部身份校验来源。

### 3.2 运行时文件

生产运行只需要三份准备文件：

| 文件 | 字节 | SHA-256 |
|---|---:|---|
| `gmted2010-30arcsec-mean.tgrid` | 274,915,129 | `54070d724ab6aee76944b6fc10a029bd865efbf4c90747088c8f6d9404dabe3e` |
| `gmted2010-30arcsec-standard-deviation.tgrid` | 143,929,399 | `f2359a29c5790fbd8bf0298470269453a45b329d1312cab88666c92355fadb97` |
| `gmted2010-source-manifest.json` | 8,792 | `52e81a30ec3cdd44c7202325896bf40f8b6b4a9d0f5f8dd8fd29c4b0d2c97011` |

格式标识为：

```text
trajecta.gmted2010-grid/v1
```

`.tgrid` 使用 256 × 256 tile、zlib 压缩、固定小端头和显式 tile 索引。读取器按需解压 tile，并使用有界只读缓存。普通运行不依赖 GDAL、Rasterio 或原始 Arc/Info 目录。

### 3.3 DatasetLock

GMTED2010 使用固定 dataset identity：

```text
dataset: gmted2010-30arcsec-mean-std/v1
profile: gmted2010_30arcsec_mean_std
```

锁包含三份文件身份、配对网格摘要、USGS attribution 和准备格式身份。生产 Runner 会验证本地文件、锁形状和配对几何，然后以共享只读句柄同时服务出生、边界和物理管线。Case 需要 `subgrid_orography` 时，RunProfile 必须精确绑定该锁；缺失、额外 binding、reader override 或身份不符均在运行前失败。

## 4. 次网格地形

### 4.1 局地地面

局地修正采用 A0 冻结定义：

```text
z_surface = z_met + z_dem(point) - mean_dem(meteorology_cell)
```

点高与标准差使用源像元中心的双线性插值。气象单元均值按球面面积计算，并以气象网格几何和单元索引缓存。周期经度、日期变更线、反向纬度顺序以及官方 Arc/Info 外边界的小数登记偏差均有显式处理。

修正地面已经接入：

- release 的 AGL/ASL 出生解析；
- domain-fill 初始粒子；
- forward/backward 边界入流；
- 地面反射与终止策略的连续路径求交；
- 边界层局地 AGL 和混合层高度。

粒子跨越修正地面时继续使用冻结的连续求交算法。实现没有直接把高度钳到地表。

### 4.2 稳定度限制

pressure 与 hybrid 两类资料均从完整局地温度、压力和几何高度柱计算干位温梯度及 `N²`。算法标识为：

```text
dry_potential_temperature_column_gradient/v1
```

地形标准差按冻结关系增加混合高度：

```text
delta_h = sigma_z                         when N² <= 0
delta_h = min(sigma_z, 2 |V_h| / N)      when N² > 0
```

缺少正式热力层时返回无效查询状态，不以中性层或零稳定度替代。

## 5. 项目与资料助手

`project data-plan` 会在 Case 解析后的物理列表包含 `subgrid_orography` 时加入 GMTED2010 全局需求。该需求没有 Case 裁剪范围，也没有时间 anchors。

资料助手支持以下流程：

```text
python tools/fetch_trajecta_data.py \
  --project PROJECT \
  --plan DATA_PLAN

python tools/fetch_trajecta_data.py \
  --project PROJECT \
  --plan DATA_PLAN \
  --execute
```

默认模式只列出三份请求、目标路径和本地文件状态。`--execute` 才下载。每份文件具有固定大小和 SHA；已有正确文件会复用，冲突文件会拒绝覆盖。下载器支持 HTTPS、临时文件和 HTTP Range 续传。助手只写项目声明的数据目录，并将下载清单原子发布；DatasetLock 仍由用户随后显式运行 `project finalize` 创建。

助手目前指向计划中的：

```text
https://github.com/origin652/trajecta/releases/download/m6-gmted2010-v1/
```

该 Release 尚未创建。本地实现和下载合同已经冻结，公共下载尚不可用。

## 6. 真实资料验证

### 6.1 Rasterio 独立交叉检查

原始 Arc/Info 栅格由 Rasterio 1.4.3 独立抽取九个冻结点，覆盖日期变更线两侧、赤道海洋、珠峰、阿尔卑斯、安第斯、圣海伦火山、开普敦、日本和高纬区域。

Rust 准备格式以相同像元中心双线性定义复验。九点 mean/std 最大差约：

```text
4.1e-10 m
```

完整 build lock → verify files → reopen → sample 测试通过：

```text
real_prepared_pair_matches_frozen_source_identity_and_rasterio_samples
passed in 87.80 s
```

### 6.2 珠峰连续边界

真实地形路径从 `86.84°E` 到 `87.10°E`，纬度为 `27.99°N`，粒子高度为 4,500 m。双网格切分得到超过十段，算法选择首个由正到负的 clearance crossing，并将根定位在：

```text
86.85°E <= longitude < 86.87°E
|clearance| <= 1e-8 m
```

测试通过：

```text
corrected_everest_ridge_path_locates_the_first_downcrossing
passed in 16.41 s
```

### 6.3 三资料家族生产矩阵

正式矩阵覆盖：

| 资料 | 方向 | worker | 物理模块 |
|---|---|---|---|
| ERA5 pressure | forward / backward | w1 / w4 | BL Langevin + mesoscale Markov + subgrid orography |
| ERA5 hybrid | forward / backward | w1 / w4 | BL Langevin + mesoscale Markov + subgrid orography |
| CFSR pressure | forward / backward | w1 / w4 | BL Langevin + mesoscale Markov + subgrid orography |

共 12 个 production run，每格 256 粒子。全部结果满足：

- `RunOutcome::Complete`；
- abnormal termination 为零；
- SQLite v2 与完整过程状态有效；
- full verify 通过；
- w1/w4 的 normalized content、normalized SQL 和 canonical output 三项 SHA 完全一致。

矩阵显式要求正式 fixture，未使用缺资料跳过分支：

```text
TRAJECTA_REQUIRE_M6_A2_FORMAL=1
cargo test --offline -p trajecta-core \
  --test m6_a2_real_families -- --nocapture

passed in 672.80 s
```

## 7. 最终门禁

```text
TRAJECTA_REQUIRE_M6_A2_GMTED=1 \
cargo test --offline -p trajecta-met \
  --test real_gmted2010 -- --nocapture
passed in 87.80 s

TRAJECTA_REQUIRE_M6_A2_GMTED=1 \
cargo test --offline -p trajecta-core \
  --test m6_a1_physics \
  --test m6_a1_cfsr \
  --test m6_a2_gmted_boundary -- --nocapture
passed

cargo test --offline --workspace
passed in 1,385.2 s

cargo clippy --offline --workspace --all-targets -- -D warnings
passed

cargo doc --offline --workspace --no-deps
passed

cargo fmt --all -- --check
passed

python tools/test_fetch_trajecta_data.py
8 passed

python tools/validate_m6_a0_contracts.py
passed: production_stage=m6_a2, SQLite v2, 12 modules, 4 presets

python tools/validate_m5_a0_contracts.py
passed

python tools/validate_m4_a0_contracts.py
passed

python -m py_compile \
  tools/fetch_trajecta_data.py \
  tools/test_fetch_trajecta_data.py \
  tools/prepare_gmted2010.py \
  tools/validate_m6_a0_contracts.py
passed

git diff --check
passed
```

## 8. 边界遵守

- 软件版本保持 `0.1.0-alpha.1`；
- Case `schema_version` 保持 `0`；
- SQLite `user_version` 保持 `2`；
- 没有加入旧算法、静默降级或快/准双路径；
- 运行期不自动下载 GMTED2010；
- 没有实现 A3 深对流；
- 没有读取或修改 `E:\flexpart\origo-validation-v1.json`；
- 没有使用子代理；
- GitHub Release 尚未创建；源码提交与推送按后续用户指令处理。

## 9. 停止点

M6-A2 的本地工程和科学验收已经通过。公共 GMTED2010 下载仍需创建 `m6-gmted2010-v1` Release 并上传三份冻结文件。该动作会修改远程仓库状态，等待明确授权。A3 未开始。
