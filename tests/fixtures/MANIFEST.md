# 测试 Fixture 清单

本目录只保存可再分发、可由仓库内说明复现的小型测试输入。输入的
GeoTIFF 派生文件在测试或 oracle 运行时写入临时目录，不提交二进制副本。

| ID | 文件 | 许可/来源 | SHA-256 | 生成与用途 | 预期 |
| --- | --- | --- | --- | --- | --- |
| `oracle-source-v1` | `oracle-source.asc` | 项目内人工构造数据；无第三方数据或许可限制 | `4da195971dd9635d38275a1180120a5c2b2ce76a42262bda5141cfc640ccbcaa` | 用 `gdal_translate -of GTiff oracle-source.asc oracle-source.tif` 生成临时 EPSG:4326 GeoTIFF；2×2、north-up、1° PixelIsArea，样本为 100/200/300/400 m | 原 CTB 与 ctb-rs 在 z=0–2、`nearest`、`bilinear`、`average` 下的解压 heightmap payload 必须一致；受限 `-s 1 -e 1` 的 child mask 也必须一致 |

## Runtime 生成的 fixture

以下 fixture 不提交二进制输入，故没有独立文件 checksum；其完整来源是受版本控制的 Rust 测试代码。它们仍遵守相同的空间元数据和预期行为记录要求。

| ID | 生成位置 | 样本/元数据 | 预期 |
| --- | --- | --- | --- |
| `runtime-geotiff-numeric-v1` | `src/geotiff.rs` 的单元测试 | EPSG:4326、north-up、2×2、0.5° PixelIsArea；`f64`、`f32`、`i16` 负高程与 `u16` | 每种类型读取后必须保持值并转换为 `f64` 公共契约 |
| `runtime-geotiff-failure-v1` | `src/geotiff.rs` 的单元测试 | 具有 `GDAL_NODATA` tag 的 GeoTIFF，以及三字节截断 TIFF | 分别返回 `NoDataEncountered` 和 `RasterRead`，不得 panic |

## 录入规则

新增 fixture 前必须在本表记录：稳定 ID、源文件、许可/来源、SHA-256、可执行的
生成步骤、空间与像元元数据，以及成功或失败的断言。含 NoData、损坏文件或不支持
feature 的 fixture 也必须列出，且明确其结构化错误预期。
