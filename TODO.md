# ctb-rs 实施 TODO

本文件由 `TECHNICAL_PLAN.md` 的实施状态派生；完成后更新状态，不改变既定技术决策。

## P0：领域边界与测试策略

- [x] 定义 crate 内模块边界和公开领域类型：`Crs`、`AffineTransform`、`Bounds`、`TileCoord`、`RasterMetadata`。
- [x] 定义 `RasterSource`、窗口读取和 NoData 错误契约；不引入具体 GeoTIFF I/O。
- [x] 实现并测试 TMS Global Geodetic 格网、zoom/resolution、坐标到瓦片及瓦片范围。
- [x] 定义并测试 CTB heightmap-1.0 的无压缩二进制 reader/writer、child bitfield 和 all-land mask。
- [x] 写出 fixture 清单、oracle 生成方法、语义兼容断言及误差规则。

## P0：依赖 spike

- [x] 通过 `cargo` 添加候选纯 Rust TIFF/GeoTIFF 依赖；确认依赖树不含 GDAL/PROJ/FFI GIS 库。
- [x] 用最小样本验证 GeoTIFF 元数据、第一波段、NoData、tiled/striped 和内部 overview 的实际可读性。
- [x] 将选型结论、已验证功能和缺口回写 `TECHNICAL_PLAN.md`。

## 当前：GeoTIFF 受限适配器

- [x] 通过 Cargo 添加纯 Rust GeoTIFF writer 作为开发期 fixture 工具，并复查依赖树。
- [x] 生成可复现的小型 EPSG:4326 fixture 及其元数据断言。
- [x] 实现 `GeoTiffRasterSource`：元数据校验、首波段窗口读取与 `f64` 样本转换。
- [x] 用 fixture 覆盖有效路径、错误 CRS、NoData 和窗口边界。

## P1：heightmap MVP

- [x] 实现 EPSG:4326 GeoTIFF `RasterSource`。
- [x] 实现窗口化采样与 nearest/bilinear/average。
- [x] 实现 `ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents`。
- [x] 与原 CTB golden fixtures 做语义兼容测试（最小 EPSG:4326 fixture 的裸 payload 逐字节一致）。

## 当前：CLI（tile 与 info）

- [x] 通过 Cargo 添加 CLI 解析依赖，并保留四个原 CTB 可执行文件名。
- [x] 实现 `ctb-tile` 的 GeoTIFF → heightmap tileset 调用、输出目录和 resume。
- [x] 实现 `ctb-info` 的 terrain 解码、child/type/height 输出。
- [x] 以进程级测试覆盖帮助、无效参数和最小成功路径。

## 当前：CLI（extents）

- [x] 从 `TilesetPlan` 生成每个 zoom 的 GeoJSON FeatureCollection。
- [x] 实现 `ctb-extents` 的 GeoTIFF 输入和输出目录 CLI。
- [x] 用 fixture 验证 GeoJSON 坐标、tile properties 与每层文件布局。

## 当前：CLI（export）

- [x] 通过 Cargo 将纯 Rust GeoTIFF writer 提升为生产依赖。
- [x] 实现 terrain `u16` bit pattern 到 signed GeoTIFF band 的导出器。
- [x] 实现 `ctb-export -i -z -x -y -o` CLI，并测试 transform、CRS 和 signed sample 行为。

## 当前：世界坐标采样器

- [x] 定义重采样算法与 world-coordinate sampling 接口。
- [x] 实现并测试 north-up EPSG:4326 下 nearest 采样。
- [x] 实现并测试边缘钳制的 bilinear 采样。

## 当前：terrain 目标瓦片采样规划

- [x] 定义并测试 heightmap 的 65×65 世界坐标样点与相邻瓦片边缘重合规则。
- [x] 为重采样接口加入目标 cell footprint。
- [x] 实现并测试确定性的 Average 样本聚合。
- [x] 生成一个 `TileCoord` 的 `Vec<f64>` terrain 高程栅格，暂不量化或写文件。

## 当前：heightmap 量化

- [x] 定义高程 `f64 -> i16` 的舍入、有限性与范围错误契约。
- [x] 实现由 sampled heights 构建 all-land `HeightmapTerrain`。
- [x] 测试负数半值、边界值、NaN、无穷大、溢出及高度数量错误。

## 当前：heightmap gzip 容器

- [x] 通过 Cargo 添加纯 Rust gzip 依赖，并检查 feature 与依赖树。
- [x] 实现并测试 compact/detailed heightmap 的 gzip 内存编解码。
- [x] 实现文件读写 API，并测试损坏或过大 gzip payload 的拒绝路径。

## 当前：tileset 规划与写入

- [x] 定义 max zoom、数据集 bounds 到 TMS tile range 的规划规则。
- [x] 生成每层被覆盖的 `TileCoord`，并测试层级与边界。
- [x] 在生成完成后按实际子 tile 回填 `ChildMask`。
- [x] 实现 `{z}/{x}/{y}.terrain` 原子 gzip 写入与 resume 行为。

## 当前：CTB 高程与空间外兼容性

- [x] 从 CTB 与本地 GDAL 源码确定 heightmap 高程的精确截断/溢出行为。
- [x] 从 GDAL warp 默认路径确定 destination 未被 source 覆盖时的初始化值。
- [x] 用最小 DEM fixture 运行 CTB，记录原始 terrain payload oracle。
- [x] 将量化器与空间外采样策略改为已验证的 CTB 行为。
- [x] 使 GeoTIFF adapter 按常见原生数值类型解码后转换到 `RasterSource` 的 `f64` 样本契约。
- [x] 为 CTB VRT 的像元中心与 Average 边缘覆盖行为建立最小 oracle 单元测试。
- [x] 将 Average 改为源/目标 PixelIsArea footprint 的面积加权，并使最小 fixture 的裸 payload 字节一致。
