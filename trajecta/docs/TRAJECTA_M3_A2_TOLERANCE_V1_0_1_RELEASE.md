# M3 A-tolerance v1.0.1 发布记录

## A 裁决

`m3-a-tolerance/v1.0.1` 已发布到工作树。本次是有界的 backend 容差修订：

- `grib:0.1.0:isobaric`（CFSR specific humidity）冻结为最多 3 binary64 ULP；
- `grib:0.2.8:isobaric`（CFSR pressure vertical velocity）继续冻结为最多 2 ULP；
- ERA5、其它 CFSR 字段、FLEXPART oracle 规则和算法版本均未放宽。

## 冻结身份

- registry version：`m3-a-tolerance/v1.0.1`
- registry SHA-256：`9f72c1b5aabead53b2de08a856b24aff27e0bc61f98282ce6b8dd4427a72b080`
- expected matrix SHA-256：`b45225cf543e4833f460571a324bc1258233c90256b108e6ab8b07d8aa806bb8`
- calibration SHA-256：`c097417f96cde49751372bb56cac141766f1f660830b74b879adbf7514ffb520`
- raw CFSR ULP calibration SHA-256：`2583243f564d4e5b8f8a584fb684a5020ec1a4e74d8f1ad91c7b4914ac0bf8fe`
- CFSR packing evidence SHA-256：`e1ff20e378ce35563e957d03c79a74d01f6e7e4311233748ed4f915a5c2f2a85`

Packing 证据只引用不可变 raw calibration，不再反向改写它。校准文件使用扩盒后的
ERA5 输入 SHA 和全场统计；q/omega 使用独立规则，禁止共享 3 ULP 门槛。

## 发布复验

使用 `tools/run_m3_backend_comparison.py` 对 v1.0.1 重跑全部冻结 backend 矩阵：

| family | compared | status | blockers |
|---|---:|---|---:|
| ERA5 pressure | 945747 | passed | 0 |
| ERA5 hybrid | 2829123 | passed | 0 |
| CFSR pressure | 5897232 | passed | 0 |

CFSR 三个时次的 q 均以最大 3 ULP 通过；omega 均以最大 2 ULP 通过。比较报告和
registry 均通过 Draft 2020-12 Schema。

门禁：

- `cargo fmt --all -- --check`：通过；
- 默认 workspace Clippy `-D warnings`：通过；
- `native-eccodes,native-netcdf` 全 target Clippy `-D warnings`：通过；
- workspace tests：236 passed，两个百万点长测使用此前独立通过的冻结证据；
- `cargo doc --offline --workspace --no-deps`：通过。

## 不宣称

本次只关闭 Windows backend 容差发布，不代表 M3/A2 完成。真实 FLEXPART 数值
oracle、Linux 全矩阵与跨平台裁决仍是 M3 硬门槛。
