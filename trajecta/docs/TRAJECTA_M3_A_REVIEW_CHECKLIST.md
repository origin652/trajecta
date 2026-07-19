# Trajecta M3 A 级科学评审清单

## 1. A0/A1 合并前

- canonical 字段的单位、shape、stagger、插值类别和 capability 与代码一致；
- QueryPlan、TransportPlan、QueryOutput、TransportOutput、状态和 VerticalBounds 无隐式哨兵；
- 输出 provenance 表自包含，跨帧和派生字段 ID 均可解析；
- `allow_estimated=false` 检查所有实际依赖，而不只检查表面输出字段；
- 规则经纬网覆盖周期经度、日期线、近极点、精确极点和球面 U/V；
- pressure/hybrid 柱按四角构造，同 cell 不同点不会复用点专属结果；
- pressure 地下 mask、有效三角、层序和无外推规则有解析测试；
- hybrid 半层压力、ECMWF alpha 位势、几何高度和 density 公式使用冻结常量；
- W 包含层面时间导数、水平坡度和 native-coordinate crossing，不使用 `-omega/(rho*g)`；
- exact-frame W 使用相邻两段斜率平均，端点缺帧时结构失败；
- surface layer 的 u*、T*、q*、L、Businger-Dyer、桥接和地面无穿透 W 有独立测试；
- 直接 u* 会重算所有依赖尺度；Estimated 输入会传播到输出 quality；
- 公共 SI 指数维度覆盖复合单位，canonical registry 拒绝符号正确但维度错误的描述符；
- 热通量符号转换进入 Profile provenance；FRICV 存在时不再要求 stress，stress 回退要求两个分量；
- Explain 默认关闭且零逐点记录分配，Full 模式计入预算并含 cell/水平权重/垂直括号/surface model/逐字段 quality+provenance；
- prepare 可改变 LRU，execute 不读取文件、不调用 provider、不改变缓存指标；
- cache key、Pin、预算失败、chunk 和 caller-order scatter 具有确定性测试；
- 空批、地下、surface undefined、available top、model top、极点和域外均为局部状态；
- 通用查询和 M4 两次查询接口测试通过；
- fmt、clippy `-D warnings`、workspace test 和 doc 通过。

## 2. B 交付后 A2

- ERA5 hybrid、ERA5 pressure、CFSR pressure 的来源、许可、日期、区域、变量、层数和哈希与计划一致；
- 三套正式锚点均在 `allow_estimated=false` 下发布 Transport 与 NearSurfaceTransport；
- 三个连续时次、ASL/AGL/Pa、海面/平原/山地/近地/高空和所有边界均实际运行；
- GRIB、NetCDF3、NetCDF4、pure-Rust、native 和跨平台比较覆盖所有时次、字段、mask、quality、provenance；
- native feature 没有静默回退，hyperslab 没有整变量伪切片；
- 百万点测试在显式预算下完成，execute I/O 计数为零，1/多线程和不同 chunk/cache 排列确定；
- FLEXPART oracle 身份完整，MIT/GPL 边界未被破坏；
- FLEXPART raw oracle、MIT comparison report 和 tolerance registry 分离，三者 SHA 相互锁定；
- 容差注册表每条规则都有 measured/preregistered/diagnostic 依据，展开 `all` 后匹配唯一，阈值未自动放宽；
- native-anchor/interpolated-common hard gate 与 modern/surface-layer report-only 没有混用；
- 异常差异已修复或有版本化、带问题编号的科学 allowlist；
- Windows 与 Linux 的真实门禁有日志和机器报告；
- 机器 JSON 与 Markdown 摘要一致；
- 所有未运行项明确标记，不能以“代码已就位”替代实际通过。

## 3. 完成裁决

A0/A1 通过只表示科学核心和公共合同可以交给 B/C 接线，不表示 M3 完成。只有第二节全部通过并签署 A2 结论后，才可把 M3 状态改为完成。
