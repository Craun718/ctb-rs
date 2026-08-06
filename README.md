# ctb-rs

`ctb-rs` 是 [cesium-terrain-builder](https://github.com/geo-data/cesium-terrain-builder)
（C++ CTB 0.4.1）的纯 Rust 重写，用于为 [Cesium](http://cesiumjs.org) 的
`CesiumTerrainProvider` 生成 [heightmap-1.0](http://cesiumjs.org/data-and-assets/terrain/formats/heightmap-1.0.html)
地形切片。它逐模块对齐原版 `libctb` 及四个命令行工具 `ctb-tile`、`ctb-info`、`ctb-export`、
`ctb-extents` 的接口、输出和错误路径。

本工具只负责生成切片，不负责把切片发布给浏览器。发布可使用
[Cesium Terrain Server](https://github.com/geo-data/cesium-terrain-server)。

## 特性

- **纯 Rust，无 GDAL/PROJ FFI**：不链接 GDAL、PROJ 或任何 C/C++ GIS FFI；GeoTIFF/VRT
  栅格读写由 OxiGeo 0.2.3 承担，通用 EPSG 坐标变换使用纯 Rust proj4rs。
- **行为基准对齐**：数值公式、迭代顺序、边界包含规则、默认参数、数据类型转换和错误条件均以
  C++ CTB 为唯一基准，不新增原版没有的算法、接口或命令行语义。
- **TMS 双 profile**：Global Geodetic（EPSG:4326）与 Global Mercator（EPSG:3857），
  y 轴自南向北。Geodetic 根层为 `2x1`，Mercator 根层为 `1x1`。
- **12 种重采样算法**：nearest、bilinear、cubic、cubicspline、lanczos、average、mode、
  max、min、med、q1、q3。
- **`#![forbid(unsafe_code)]`**：库代码不含 unsafe。

## 命令行工具

四个工具均与原版同名，`--version` 输出当前项目版本 `0.0.1`；C++ CTB oracle 仍固定为
`0.4.1`，版本号不再作为 Rust 与 C++ 的兼容性断言。

### `ctb-tile`

从 proj4rs 可解析 EPSG 的 GeoTIFF 或 VRT DEM 生成 `{z}/{x}/{y}.terrain` 切片，计算与源分辨率
匹配的最大 zoom，并自顶向下生成所有重叠切片；也支持以 GeoTIFF（GTiff）作为输出格式。
EPSG:4326 与 EPSG:3857 使用内建公式，其它 EPSG 输入通过 proj4rs 重投影到目标 CTB
profile；任意 WKT 输入不在当前支持范围内。

```sh
ctb-tile --output-dir ./terrain-tiles dem.tif
```

主要选项：

```
-o, --output-dir <dir>          输出目录（默认当前目录）
-f, --output-format <format>    Terrain（默认）或 GTiff
-p, --profile <profile>         geodetic（默认）或 mercator
-t, --tile-size <size>          像素边长，Terrain 固定 65，栅格默认 256
-s, --start-zoom <zoom>         起始（最高）zoom，默认源推导最大值
-e, --end-zoom <zoom>           结束（最低）zoom，默认 0
-r, --resampling-method <name>  重采样算法，默认 average
-n, --creation-option <opt>     GDAL 创建选项，形式 NAME=VALUE，可多次指定
-z, --error-threshold <px>      近似变换误差阈值，默认 0.125
-m, --warp-memory <bytes>       warp 内存上限，0 用默认值
-c, --thread-count <count>      工作线程数，非正值按 CPU 数
-R, --resume                    不覆盖已写出的最终切片
-q, --quiet                     仅输出错误
-v, --verbose                   输出每个完成的切片
```

### `ctb-info`

打印一个 `.terrain` 文件的信息（child 可用性、水/陆分类、可选的 ASCII 高程矩阵）。

```sh
ctb-info --show-heights ./terrain-tiles/0/0/0.terrain
```

### `ctb-export`

把 `.terrain` 切片导出为 GeoTIFF。terrain 文件不含自身位置，需通过参数指定 zoom 与 TMS 坐标。

```sh
ctb-export -i ./0/0/0.terrain -z 0 -x 0 -y 0 -o tile.tif
```

### `ctb-extents`

把源 DEM 会覆盖到的切片范围按 zoom 输出为 GeoJSON 文件。

```sh
ctb-extents --output-dir ./extents dem.tif
```

## 栅格格式

输入支持 OxiGeo 0.2.3 的 GeoTIFF（含 BigTIFF）与 VRT；输出支持 CTB Terrain 和 GeoTIFF。
OxiGeo 0.2.3 可以探测 NetCDF、HDF5、JPEG2000 等格式，但当前版本没有这些格式的像素读取
实现，所以 `.nc`、`.h5`、`.jp2` 等输入会在写出任何切片前返回不支持错误。

## 构建

本项目使用 Rust 2024 edition，需要较新的 Rust 工具链（建议 1.85 及以上）。

```sh
cargo build --release
```

构建产物为四个二进制：`ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents`，位于
`target/release/`。安装到用户 bin 目录：

```sh
cargo install --path .
```

## 测试

集成测试覆盖四个 CLI 的核心路径，并生成临时 GeoTIFF/VRT 作为输入：

```sh
cargo test
```

`scripts/` 下提供与 C++ oracle 对比的验证脚本：

- `verify-ctb-oracle.zsh`：对照 C++ CTB 输出做切片差分。
- `benchmark-ctb-tile.zsh`：`ctb-tile` 性能基准。

## 工作原理

terrain 高程以 C++ 的 `uint16_t((Float32_height + 1000) * 5)` 编码（有效范围内向零截断）；
未被源覆盖的区域初始值为 `0.0` m。`ctb-tile` 的 Terrain 路径忽略 CLI 的 resample 算法，
固定使用原版 Average 路径；GTiff 路径则使用所选的 GDAL 算法。关于切片大小、bounds、
最大 zoom、child flags 与遍历边界的精确规则，详见 [TECHNICAL_PLAN.md](TECHNICAL_PLAN.md)。

## 状态

本项目为进行中的行为对齐移植。已实现原版的库模块与四条 CLI 路径，并持续以 C++ CTB（固定于
commit `d9c29b2`，配合 GDAL 3.11.4）为 oracle 做差分验证。各模块的“已由 oracle 证明”与
“仅实现、尚未证明”状态记录在 [TECHNICAL_PLAN.md](TECHNICAL_PLAN.md) 与 [TODO.md](TODO.md) 中，
不应把后者视为已完成。完整规划与对齐依据见上述两份文档及 [TEST_STRATEGY.md](TEST_STRATEGY.md)。

## 致谢

本项目以 geo-data/cesium-terrain-builder 的 C++ 实现为唯一行为基准进行重写。
