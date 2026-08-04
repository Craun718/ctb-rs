# ctb-rs 技术方案

## 1. 目标、范围与不可变约束

本项目以 `/Users/sander/coding/cesium-terrain-builder` 中的 C++ CTB 为唯一行为
基准，将其逐模块翻译为 Rust。目标是完整对齐原版公开库与四个工具
`ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents` 的接口、输出和错误路径。

实现必须同时满足：

- 不链接 GDAL、PROJ 或其他 C/C++ GIS FFI；GDAL 职责以纯 Rust/GeoRust 依赖实现。
- 不新增 C++ 原版没有的算法、接口、命令行语义或架构层次；Rust 模块、类型和调用顺序
  应一一映射原版类与函数。仅为所有权、错误传播和内存安全所必需的差异可以存在。
- 不以 Rust 语法糖改变算法或默认值：数值公式、迭代顺序、边界包含规则、默认参数、
  数据类型转换和错误条件均先从 C++ 固定，再编码。
- 现有 Rust 实现若与 C++ 冲突，以 C++ 为准，并在本文、测试策略和 TODO 中同步更正。
- 依赖变更仅通过 Cargo CLI 完成；生产代码不使用 `unwrap`。已证明的不变量可用带说明的
  `expect`。

“完整对齐”分为可验证的三层：

1. CLI：选项、默认值、stdout/stderr、退出状态、目录及 resume 行为一致；
2. 库：Grid、Tiler、Iterator、Tile 及异常条件的可观察行为一致；
3. 结果：同一输入和固定 CTB 版本下，路径集合、元数据、未压缩 terrain payload、栅格
   样本和 child flags 一致。TIFF 等容器允许仅比较语义，除非 C++ oracle 证明可稳定逐字节。

## 2. C++ 模块到 Rust 模块的映射

| C++ CTB | 当前 Rust 对应 | 当前状态与后续规则 |
| --- | --- | --- |
| `Bounds`、`Coordinate`、`TileCoordinate`、`Grid` | `grid` | 基础类型、Global Geodetic、Global Mercator 和 `TileGrid` 已存在；所有 tiler 必须转为通过同一 Grid 契约运行。 |
| `GlobalGeodetic`、`GlobalMercator` | `grid` | 公式与两个 profile 的 Raster/Terrain CLI 接入已落地；C++ tile 差分待补。 |
| `GDALTiler`、`GDALTile`、`gdaloverviewdataset` | `raster`、`geotiff`、`sampling`、`cache` | 用纯 Rust RasterSource、坐标变换、窗口/overview 选择和采样顺序逐项等价；不能以新的“优化型”数据流替换 C++ 行为。 |
| `RasterTiler`、`RasterIterator` | `raster_sampling`、`raster_tileset` | 已接入通用 Grid、4326↔3857、12 个采样分支和 GeoTIFF creation options；driver/过程差分待补。 |
| `TerrainTiler`、`TerrainTile`、`TerrainIterator` | `terrain`、`terrain_sampling`、`tileset` | heightmap-1.0 路径已存在；继续以 `terrainTileBounds`、Float32 读回、`uint16_t((h+1000)*5)` 和 child 逻辑逐项复核。 |
| `ctb-tile` | `src/bin/ctb-tile.rs` | Terrain/GTiff 内建路径、12 个采样名和主要 GeoTIFF options 已实现；任意 GDAL driver、ApproxTransformer 和完整错误矩阵待补。 |
| `ctb-info`、`ctb-export`、`ctb-extents` | 同名 `src/bin` | 基础路径存在；以逐命令 oracle 补齐文本、错误和文件语义。 |

Rust 的 `RasterSource`、缓存和 writer 只能作为 GDAL dataset/VRT 的内部实现替身；它们不得
变成对外可见的新产品架构。每扩展一个接口必须在表中指明它替代的 C++ 位置。

## 3. GDAL 职责的纯 Rust 替换

| GDAL 职责 | 纯 Rust 落点 | 对齐依据 |
| --- | --- | --- |
| Dataset 打开、波段、GeoTransform、NoData、TIFF tag | 纯 Rust TIFF/GeoTIFF reader/writer（当前 `geotiff-reader`/`geotiff-writer`） | `GDALTiler` 和工具实际读取的 metadata；每种样本类型、压缩、strip/tile、BigTIFF、overview 都须有 fixture。 |
| `GDALCreateWarpedVRT` / RasterIO | 显式的同顺序坐标映射、核函数、destination 初始化和样本转换 | `GDALTiler::createRasterTile`、`TerrainTiler::createRasterTile` 与 GDAL oracle。 |
| SRS 比较和 `OGRCoordinateTransformation` | 纯 Rust CRS 解析与登记式 EPSG 变换 | 先 EPSG:4326 与 3857；之后只按原版实测输入 CRS 增量实现，不把未知 CRS 当作 4326。 |
| overview dataset | 纯 Rust TIFF 内部/外部 overview 表示和 C++ 选择公式 | `getOverviewDataset` 及 SuggestedWarp oracle。 |
| `CreateCopy` driver | 每个原版实际支持且需要兼容的格式分别实现 writer/creation-option 映射 | 先 GTiff；其余 driver 以 C++ oracle 建立清单后按优先级翻译，不能静默降级。 |

任何候选 crate 必须确认无 GIS FFI、许可证可用、维护可用，且先通过独立 spike。若纯 Rust
生态缺失某一原版功能，先在本项目的对应模块实现最小 parser/codec；不得改用 GDAL 或缩小
兼容性承诺。

## 4. 固定的原版行为

- 两种 profile 均为 TMS（y 自南向北）。Geodetic 根层为 `2×1`，Mercator 根层为 `1×1`；
  Terrain 默认 tile size 为 65，RasterTiler 默认 256。
- `TerrainTiler` 以 `terrainTileBounds` 生成带东、北重叠的 65×65 VRT，再将 VRT 的
  GeoTransform 改回普通 tile bounds。
- terrain 高程写入为 C++ `uint16_t((Float32_height + 1000) * 5)`：有效范围内向零截断。
  未被 source 覆盖的 VRT destination 初始值为 `0.0` m。Rust 对非有限值和无法安全表示的
  值返回结构化错误，不复刻 C++ 未定义转换。
- `TerrainTiler` 忽略 CLI 传入的 resample algorithm，并固定使用其原版 Average 路径；
  `RasterTiler` 则使用选定的 GDAL 算法。
- bounds、最大 zoom、child flags 和 traversal 必须由 C++ `Grid`、`TilerIterator` 和
  `TerrainIterator` 的实际边界规则确定，尤其要覆盖右/上界及仅部分覆盖的 tile。

## 5. 设计和测试门禁

每个实施单元必须按以下顺序进行：

1. 更新本文的 C++ 责任映射、接口形状和边界条件；
2. 更新 `TODO.md`，将最高优先级单元拆为可验证任务；
3. 在 `TEST_STRATEGY.md` 登记 oracle、fixture 和断言；
4. 仅随后修改生产代码；完成后回写证据和状态。

没有 C++ 源码或 oracle 足以决定的边界，参考原版仓库使用的 GDAL 默认行为；若仍无法正确
落地，停止并向用户询问，不能自行改变上述设计。

## 6. 实施顺序

### P0：重新建立规格基线（进行中）

固定 C++ CTB 版本、构建命令和 oracle 输入；建立完整 CLI/库模块兼容矩阵。对当前 Rust
已实现功能逐项标记“已由 oracle 证明”或“仅实现、尚未证明”，不得把后者视为完成。

完成标准：三个规划文档重建；每个 C++ 模块、CLI 参数和输出 driver 有责任人（Rust 模块）
及测试状态。

#### P0 实施记录 1：oracle 基线（已记录，C++ 构建待兼容）

固定 oracle 源码为 `/Users/sander/coding/cesium-terrain-builder` commit
`d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`（2025-09-12，CTB 0.4.1）；本机 GDAL 为
`3.13.2`（2026-07-20），CMake 为 Release，编译器 `/usr/bin/c++`。CMake cache 指向
`/opt/homebrew/Cellar/gdal/3.13.2_1/lib/libgdal.dylib`。该组合构建失败于旧 CTB 对
`GDALDataset::GetGeoTransform`、`GetMetadata` 虚函数签名的 override/返回类型不兼容；
因此 oracle 输入、版本和失败原因已固定，但 C++ 可执行文件尚未生成。

#### P0 实施记录 2：恢复 C++ oracle 后的阻塞项收敛（进行中）

用户已修复 `/Users/sander/coding/cesium-terrain-builder/build-with-gdal.sh`。脚本已成功生成
`build-gdal-v3.11.4/tools/ctb-tile`、`ctb-info`、`ctb-export` 和 `ctb-extents`；使用 GDAL
`3.11.4`，C++ commit 为 `d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`。C++ 工程仍保留用户的
已暂存 `.gitignore` 与 `build-with-gdal.sh`，本项目不修改它们。当前以该 binary 为唯一 oracle，
按本方案中标记为“C++ 差分待补”的顺序执行 Grid 右上边界、Raster/Terrain payload、12 种
resampling、destination/NoData/整数转换、overview、EPSG:4326↔3857、GeoTIFF tags 及四个
CLI 契约。每个差异先登记为可复现证据，再修改 Rust；未获得 oracle 证据的实现继续保持未完成标记。

#### P2 实施记录 7：RasterTiler overview 目标比例修正（已实现，继续扩展 oracle）

恢复 oracle 后，`high-resolution-overview / nearest / automatic / 0/0/0.terrain` 首次差分
可稳定复现：C++ 与 Rust payload 长度均为 8452，但 overview 选择区域的 Rust 高程编码为
`6500`、C++ 为 `5500`；同一高分辨率输入去掉 overview 后的全部 12 算法均通过。根因是
Rust `RasterTileSamplePlan` 用固定 `1 / source_pixel_width` 估算比例，忽略当前目标 tile 的
像元分辨率。现改为 `destination_resolution / source_pixel_width`，使 overview 选择与
`GDALTiler` 的目标 warp resolution 对齐；仍需完成完整 overview、边界和不同 zoom 的差分。

#### P1 实施记录 2：TerrainTiler 固定 Average 路径（已实现，oracle 回归中）

C++ `TerrainTiler::createRasterTile` 沿用未设置 `TilerOptions` 的 GDAL 默认
`GRA_Average`，CLI 的 `-r` 只影响 RasterTiler。已将两个 Terrain tileset 写入入口固定为
`Average`，即使调用方传入其它 resampling 也不改变 heightmap payload；CLI 仍保留完整参数
解析和 RasterTiler 的 12 个分支。恢复 oracle 后的首个 overview 差异正是该遗漏暴露的证据，
修复后须重跑 5 输入 × 12 算法 × 2 range 矩阵。

#### P4 实施记录 5：四个 CLI 版本入口（已实现，帮助文本仍待收敛）

C++ oracle 的四个工具均以 `--version` 输出 `0.4.1`；Rust 工具此前将该参数误解析为输入或
未知选项。四个 clap 入口已固定公开版本字符串 `0.4.1`，与 CTB 0.4.1 oracle 对齐；帮助文本
的格式、可执行文件路径和参数描述仍作为独立 CLI golden 差分保留。

#### P0 实施记录 3：Terrain resampling oracle 矩阵（部分通过）

使用 C++ CTB 0.4.1/GDAL 3.11.4 与 Rust binary 的现有脚本，plain、float-negative、
tiled-overview 和无 overview 的 high-resolution 输入共 4 组均完成 12 算法 × automatic/limited
范围的裸 terrain payload 逐字节比对。剩余首个差异是带 overview 的 high-resolution 输入
`0/0/0.terrain`；C++ debug 明确显示 Terrain 使用 `GWKAverageOrMode`，差异收敛到 GDAL
overview warp 的 source window/坐标语义，未将该部分标记为完成。

Mercator direct-source z0 也已建立最小 oracle：C++/Rust 均生成 `0/0/0.terrain`，但 raw
payload 首差异在 byte 4225，边缘高度编码为 C++ `5500/6000`、Rust `6500/7000`。输入为
EPSG:3857、无重投影的窄范围 fixture；该差异归入 Mercator upper-edge/source coverage 与
destination 初始化规则，尚未修改实现。

RasterTiler GTiff z0、tile size 16 的 plain fixture 差分显示：nearest、bilinear、cubic、
cubicspline、lanczos 已逐值通过；average、mode、max、min、med、q1、q3 只在 source 外边缘
失败。C++ 对目标中心位于 source bounds 外、但目标 footprint 与 source 边界相交的像元保留
destination 初值 `0`；Rust 原先先按 footprint 相交面积统计，违反了 GDAL warp 的中心有效性
门禁。现按 C++ 执行顺序仅在 RasterTiler 统计核前加入中心 bounds 检查，Terrain overlap 路径
继续保留原有行为；plain GTiff z0/tile-size-16 的全部 12 算法已逐值通过。


### P1：收敛既有 Geodetic 路径

先对 Terrain heightmap 和 `-f GTiff` 的 EPSG:4326 direct-source 路径逐项比对：tile range、
terrain overlap、全部 12 个 resampling 名称的分支、样本类型、NoData、creation options、
quiet/verbose/resume 和错误文本。先修正任意与 C++ oracle 不符的既有实现，再扩展功能。

完成标准：已支持路径的裸 terrain payload 逐字节相同；GTiff 的路径、栅格、CRS、transform、
样本类型和 NoData 相同；全部差异有明确 C++ 证据。

#### P1 实施记录 1：RasterTiler 的通用 Grid 写入边界（已实现，尚待 C++ 差分）

`RasterTileSamplePlan::from_grid` 已按 `GDALTiler::createRasterTile` 所持有的 `const Grid &`
计算目标像素中心和 footprint；`RasterTileset` 通过 `TilesetPlan::from_raster_with_tile_grid`
生成范围，并在 EPSG:4326↔3857 内建 CRS 间执行纯 Rust 反向采样。Tile 队列继续严格映射
`GridIterator`：每层 x 递增、每个 x 内 y 递增，且调用端按最高 zoom 到最低 zoom 消费。
`ctb-tile -f GTiff -p mercator` 据此构造 `GlobalMercatorGrid`；未知 CRS 仍拒绝，C++ 差分待补。

Rust 层证据为 `tests/cli.rs::ctb_tile_writes_mercator_direct_source_gtiff` 对 EPSG:3857 z0 的
path、GeoTransform、CRS 和样本值断言，以及 `ctb_tile_writes_geotiff_rastertiler_tiles` 对
EPSG:4326 输入在无输出前返回错误的断言；`cargo test`（69 passed）和
`cargo clippy -- -D warnings` 已通过。z1 和 C++ 差分 fixture 尚未建立，因此该记录不能作为
P1 全部完成的证据。C++ 依据为
`src/RasterIterator.hpp`（继承 `TilerIterator`）和 `src/GridIterator.hpp`（x 外层、y 内层、由
startZoom 递减至 endZoom）。

### P2：补齐 GDAL VRT 等价层

按 `GDALTiler` 的执行顺序完成 source/grid CRS 比较、四角 bounds 变换、目标 GeoTransform、
destination 初始化、采样核、整数转换、内部 overview 选择与缓存。先为每一步构建 GDAL
中间结果 oracle，再写 Rust 实现。RasterTile 与 TerrainTile 分别保留 C++ 的像素布局差异。

完成标准：高分辨率、overview、边缘、NoData、所有支持样本类型的 oracle 可重复通过；
单线程和多线程输出相同。

#### P2 实施记录 1：离散统计采样核（Rust 实现完成，C++ 差分待补）

本阶段先实现 `GDALResampleAlg` 的 `GRA_Mode`、`GRA_Med`、`GRA_Q1` 和 `GRA_Q3`。
采样窗口沿用当前 `indices_overlapping_footprint` 的闭开像素交集规则，按 row-major 顺序
读取 source window；空覆盖返回 VRT destination 的初始值 `0.0`。统计值只使用窗口内实际
覆盖的样本，不引入新的 nodata 语义；并列 mode 选择首次出现的值，以保持稳定的输入顺序。
分位数采用 nearest-rank 离散定义：排序后 `ceil(p*n)`（最小为第一个）对应
`p=0.5/0.25/0.75`。该记录对应 `GDALTiler.cpp` 中 `psWarpOptions->eResampleAlg` 的
算法选择，连续核仍待后续记录和 C++ 差分。

Rust 证据：`sampling.rs` 的 `sample_with_footprint(_level)` 已接入四个分支，并以非平坦
2×2 fixture 验证 nearest-rank、row-major tie-break 和完全越界时的 `0.0`；`cargo test`
通过 71 项，`cargo clippy -- -D warnings` 通过。尚未完成的证据是同一 fixture 经 C++
`ctb-tile -r mode|med|q1|q3` 的输出差分，因此不能据此关闭 P2 的兼容性任务。

#### P2 实施记录 2：连续 4×4/6×6 核（Rust 实现完成，C++ 差分待补）

下一单元依据 GDAL `alg/gdalresamplingkernels.h` 与 `gdalwarpkernel.cpp`：`cubic` 使用
Catmull-Rom（半径 2），`cubicspline` 使用三次 B-spline（半径 2），`lanczos` 使用半径
3 的 windowed sinc（6×6）。目标像素坐标继续采用当前 pixel-corner 到 pixel-centre 的
`-0.5` 变换；越过 source 边界的 kernel tap 被跳过，最终按实际权重归一化。该范围先限于
当前 north-up、无 NoData 的 RasterSource 契约，NoData/density 语义仍须单独 oracle。

Rust 证据：`sampling.rs` 已实现三种核及边界 tap 丢弃/权重归一化，72 项测试和 clippy
通过；测试覆盖中心点与边缘有限值。GDAL 版本差异、缩放因子小于 1 和 NoData/density
仍未由 C++ oracle 证明，因此本记录不关闭 P2 总体任务。

#### P2 实施记录 3：resampling oracle 矩阵扩展（脚本完成，oracle 待执行）

现有 `scripts/verify-ctb-oracle.zsh` 只运行 nearest、bilinear、average、max、min，不能
证明新接入的七个算法。先将脚本矩阵扩展为 CTB CLI 声明的 12 个名称；执行仍要求外部
`CTB_ORACLE_BIN`，没有该 binary 时只记录为未验证，不改变 Rust 行为或兼容性结论。

证据：脚本已覆盖 12 个 CLI 名称并通过 `zsh -n`；当前环境只有 Rust `target/debug/ctb-tile`
和 GDAL utilities，没有 C++ `ctb-tile`，故差分执行仍是待办。

#### P4 实施记录 1：info/export/extents CLI 收敛（Rust 实现完成，C++ 差分待补）

依据 C++ `tools/ctb-info.cpp`、`tools/ctb-export.cpp` 和 `tools/ctb-extents.cpp`，本单元
先修正 `ctb-info -e` 的 ASCII raster 输出为 `Heights:` 后逐行输出、每个样本带尾部空格，
并让 extents 复用 `TileGrid` 接口覆盖 geodetic 与 mercator。GeoJSON 的属性和科学计数法
保持现有 C++ 对齐实现；export 的缺失输入 fallback/error 行为保持已有契约。

Rust 证据：`ctb-info -e` CLI golden test 检查 66 行 ASCII 输出、尾部换行和行内尾空格；
`ctb-extents` 已通过 `&dyn TileGrid` 生成两种 profile 的 GeoJSON，仍在 source CRS 与
target grid 不一致时返回结构化错误；`cargo test` 72 项及 clippy 通过。C++ 逐字节 CLI
差分和 EPSG:4326→3857 extents 仍待 P3 oracle。

#### P3 实施记录 1：EPSG:4326↔3857 坐标变换（RasterTiler Rust 实现完成，差分待补）

实现范围固定为 CTB 当前两种内建 Grid 的 EPSG:4326 与 EPSG:3857。正向 Web Mercator
使用半长轴 6378137、纬度裁剪到 Web Mercator 有效范围；反向变换使用
`atan(sinh(y/R))`。RasterTiler 将目标像素中心和 footprint 的四角转换到 source CRS，
再沿 source north-up transform 采样；Tile metadata、GeoTransform 和 CRS 保持 target grid。
未知 CRS 或旋转 transform 仍返回结构化错误，不静默当作 4326；目标边界外的投影坐标交给
destination 初始值和 source bounds 规则处理，以保留 C++ upper-edge tile 行为。

Rust 证据：`raster::transform_coordinate/transform_bounds` 已覆盖双向控制点，
`RasterTileSamplePlan` 已把目标中心和 footprint 转入 source CRS，tileset 规划和 GTiff writer 已保留 target CRS；CLI 覆盖 EPSG:4326 source→EPSG:3857 target，`cargo test` 74 项、clippy 通过。TerrainTiler 接入和 upper-edge 行为已覆盖 Rust CLI；缩放/overview、 NoData 和 C++ z0/z1 payload 差分仍未完成。

#### P3 实施记录 2：TerrainTiler Grid/重投影接入（Rust 实现完成，差分待补）

`TerrainSamplePlan` 与 factory tileset writer 已改为接受 `TileGrid`，保留 65×65、东/北
overlap 和 child-mask 计算；Terrain 采样中心与 footprint 会从目标 Grid CRS 转换到 source
CRS。`ctb-tile -f Terrain -p mercator` 已覆盖 EPSG:4326 source 的 z0 过程测试。C++
terrain payload、overview 和 NoData 行为仍待 oracle。

#### P4 实施记录 2：GeoTIFF LZW creation option（Rust 实现完成，C++ 差分待补）

依赖审计确认 `geotiff-writer` 0.8.0 的纯 Rust writer 提供 `Compression::Lzw`，因此将
CTB `-n COMPRESS=LZW` 映射到该实现；未知 creation option 继续在写目录前拒绝。验证包含
压缩输出可由当前 Rust reader 读回，并检查 CLI 错误/成功路径。

C++ oracle 构建记录：现有 CTB 0.4.1 源码在系统 GDAL 3.x 头文件下因
`GDALDataset::GetGeoTransform` 与 `GetMetadata` 虚函数签名变化而无法编译；该环境差异
暂不改变 Rust 兼容矩阵结论。

Rust 证据：`RasterGeoTiffCompression::Lzw` 与 CLI `COMPRESS=LZW` 已接入，CLI 生成文件
可由 `GeoTiffFile` 读回；74 项测试和 clippy 通过。

底层 TIFF 常量还列出 PackBits，但 `GeoTiffBuilder` 当前明确拒绝该压缩方式，因此不能直接映射；JPEG/LERC/ZSTD 等需要单独确认 C++ driver 清单、feature 和有损/无损语义，暂不静默映射。

#### P4 实施记录 3：CLI profile 默认值和 Terrain creation-option 校正（Rust 实现完成，差分待补）

依据 `tools/ctb-tile.cpp` 与 `tools/ctb-extents.cpp`：Terrain 默认 tile size 为 65，非
Terrain raster profile 默认 256；`-n/--creation-option` 对 Terrain 无效且应被拒绝。Rust
当前 GTiff 和 extents 路径仍有默认值/忽略 option 差异，本单元先修正这些不涉及 GDAL
driver 的 CLI 行为。

Rust 证据：GTiff 默认 tile size 为 256，Terrain 默认 65；extents 根据 profile 选择 65/256；
Terrain creation option 在输出前返回错误。CLI 集成测试与 74 项 Rust 测试、clippy 均通过。

#### P4 实施记录 4：ctb-tile warp 执行参数（解析与边界已实现，数值差分待补）

`tools/ctb-tile.cpp` 的 `-z/--error-threshold` 默认值为 `0.125`，`-m/--warp-memory`
默认值为 `0.0`（交由 GDAL 内部决定）。这两个选项属于 GDAL warp 的近似变换与内存
预算控制，不改变 CTB 命令行中声明的目标范围；当前 Rust 路径使用显式、精确的纯 Rust
坐标变换，没有可等价映射的 GDAL ApproxTransformer 或 warp 内存池。

因此 Rust CLI 必须解析并验证这两个参数，默认值可进入现有精确路径；非默认值暂不静默
忽略，而是在开始写出前以结构化错误报告“该执行控制项尚未实现”。后续获得可运行 C++
oracle 后，先测量非默认 `-z` 对投影 tile 样本的影响，再决定是否翻译 ApproxTransformer；
`-m` 仅在确认其没有可观察输出影响后再映射为 Rust worker/cache 控制。

Rust 证据：`ctb-tile` 已解析两个选项；默认值通过校验，负数/非有限值和非默认值均在
创建 source 与写出 tile 前失败；相关测试和 clippy 均通过（76 项测试）。

#### P4 实施记录 5：GeoTIFF 内部 overview 读取（Rust 验证补齐，C++ 选择差分待补）

`GeoTiffRasterSource` 已通过 `GeoTiffFile` 暴露内部 overview 数量，依据目标/源分辨率比
选择 overview，并为所选 IFD 派生像元尺寸；`read_sampling_window` 已按所选 level 读取
overview 数据。此单元补充真实多级 COG fixture，验证 top-level overview IFD 的发现、级别
边界、缩放后的 GeoTransform 和窗口样本，不改变当前 `GDALSuggestedWarpOutput2` 公式的
实现。C++ oracle 恢复后仍需用相同输入确认 tie/boundary 选择和 Terrain/Raster 两条调用链。

Rust 证据：使用 `CogBuilder` 生成 2/4 倍 top-level overview 的真实 GeoTIFF fixture，已
验证 1.5、2.0、4.0 ratio 的级别选择、overview GeoTransform 和 2×2 窗口样本读回；全套
测试 77 项、clippy 通过。C++ SuggestedWarp/getOverviewDataset 差分仍待补。

#### P3 实施记录 3：Web Mercator 有效纬度边界（Rust 修正，C++ 数值差分待补）

Global Mercator 的 grid 范围是 `±originShift`，对应 EPSG:3857 的有效纬度约
`±85.0511287798066°`。纯 Rust 正向变换在输入超出该范围时必须先裁剪纬度，避免把
EPSG:4326 的极点映射到 grid 外；反向变换保持其数学结果。该修正只收敛既有内建 CRS
边界，不新增 CRS 或投影接口。

Rust 证据：正向变换已对 ±90° 和超出有效范围的输入裁剪到有效纬度边界，新增边界测试
通过；全套测试 78 项、clippy 通过。与 GDAL 的精确数值差分仍待可运行 C++ oracle。

#### P2 实施记录 4：RasterTiler 全 resampling 分支（Rust 路径补齐，GDAL 数值差分待补）

RasterTiler 的 footprint 采样必须覆盖 CLI 声明的 12 个 `ResamplingMethod`。已有连续核
和离散统计函数不能只在通用 terrain/footprint helper 中可用；`sample_with_footprint_raster_tiler`
也必须将 cubic、cubicspline、lanczos 连接到有限核，将 mode、med、q1、q3 连接到离散
统计分支。算法公式、窗口顺序和边界规则沿用现有实现，不新增 resampling 名称；C++
GDAL 数值差分仍作为独立验证门禁。

Rust 证据：RasterTiler 现在已将 12 个 CLI resampling 名称全部连接到可执行分支，新增
非平坦 source fixture 覆盖每个名称且全套测试 79 项、clippy 通过；各算法与 C++ GDAL 的
具体数值差异仍待 oracle。

#### P2 实施记录 5：NoData density 传播（Rust 最小等价实现，GDAL 差分待补）

`GDALTiler` 将 source band 的 NoData 交给 GDAL warp，destination VRT 初始值为 `0.0`；
NoData 不应使整个 RasterIO window 失败，而应按像元参与 density/权重计算。Rust
`RasterWindow.samples` 保持现有公开形状，以 `f64::NAN` 表示从 GeoTIFF NoData 转换来的
无效样本；采样核统一跳过非有限 tap，在没有有效贡献时返回 `0.0`。该设计不新增公开
mask 接口，先覆盖当前单 band north-up 契约；真实 GDAL source/destination NoData、NaN
和整数样本差分仍待 oracle。

Rust 证据：GeoTIFF 混合 NoData window 现在返回 NaN 标记与有效样本；12 个 RasterTiler
分支均过滤无效值，全 NoData footprint 返回 `0.0`，并保留 Terrain 的后续高度编码路径。
专项测试与全套测试 80 项、clippy 均通过；GDAL density/NaN 数值差分仍待补。

#### P4 实施记录 7：GeoTIFF ZSTD creation option（Rust 接入，C++ 字节差分待补）

当前纯 Rust `geotiff-writer`/`tiff-reader` 已提供无损 ZSTD codec，且不需要新增 GIS FFI
或依赖。将 C++/GDAL 的 `COMPRESS=ZSTD` 映射为 writer 的 ZSTD 压缩；未知选项继续拒绝，
JPEG、LERC、PackBits 仍分别受类型/参数或 writer API 约束，不在本单元静默映射。

Rust 证据：`COMPRESS=ZSTD` 已接入 `RasterGeoTiffCompression`，CLI 生成文件可由
`GeoTiffFile` 打开并读取，未知 creation option 仍拒绝；全套测试 80 项、clippy 通过。
C++ driver metadata 与压缩字节差分仍待补。

BigTIFF/Predictor 证据：CLI 已将 `BIGTIFF=NO/YES/IF_NEEDED` 和 `PREDICTOR=1/2/3` 解析并
传递到 writer；测试覆盖 BigTIFF header、浮点 Predictor=3、浮点使用 Predictor=2 的失败
路径和 parser 错误路径；全套测试 82 项、clippy 通过。

#### P4 实施记录 8：GeoTIFF BigTIFF 与 Predictor creation options（Rust 接入，driver 差分待补）

`geotiff-writer` 暴露 `TiffVariant::{Classic, BigTiff, Auto}` 和 TIFF `Predictor::{None,
Horizontal, FloatingPoint}`。将 `BIGTIFF=NO/YES/IF_NEEDED` 分别映射为 Classic/BigTiff/Auto，
将 `PREDICTOR=1/2/3` 映射为对应 predictor，并把选项沿 `RasterTileset` 写出路径传递到
builder。只接受 C++ GTiff 语义中可确定且与源样本类型相容的值；冲突或未知选项在写出
前拒绝，`TILED/BLOCKXSIZE/BLOCKYSIZE` 另行处理。

#### P4 实施记录 9：GeoTIFF tiled block options（Rust 接入，GDAL layout 差分待补）

将 `TILED=YES` 映射为 writer 的 tiled layout，默认 block 为 256×256；`BLOCKXSIZE` 和
`BLOCKYSIZE` 可覆盖对应边长。writer 要求 block 边长为正的 16 倍，因此 Rust 在创建
任何 tile 前拒绝不满足该约束的值；`TILED=NO` 保持 strip layout。该选项只影响输出
容器布局，不改变 CTB tile size、样本或 georeferencing。

Rust 证据：`TILED=YES` 默认 256×256、显式 `BLOCKXSIZE=32/BLOCKYSIZE=16` 和 strip
路径均可写出；非正数或非 16 倍 block 在 parser 阶段失败；全套测试 82 项、clippy 通过。
TIFF layout tags 与 C++ GTiff CreateCopy 的差分仍待补。

#### P4 实施记录 10：GeoTIFF JPEG/LERC compression（Rust 接入，参数/driver 差分待补）

纯 Rust writer 已提供 JPEG 与 LERC codec。`COMPRESS=JPEG` 映射为 JPEG，实际样本类型与
writer 的 8-bit JPEG 约束不相容时返回结构化错误；`COMPRESS=LERC` 映射为无额外量化参数
的 LERC。LERC quality/max-z-error 等 GDAL creation options 尚未加入，不能静默丢弃。

Rust 证据：8-bit source 的 JPEG CLI 输出与 reader 打开成功，Float source 的 JPEG 在写出
tile 前失败；Float GeoTIFF 的 LERC CLI 输出可读回；全套测试 83 项、clippy 通过。JPEG
质量与 LERC 参数、C++ driver metadata/字节差分仍待补。

#### P2 实施记录 6：RasterTiler level-aware overview path（Rust 接入，SuggestedWarp 差分待补）

`RasterTileSamplePlan::sample_values` 先按 source 分辨率请求 `sampling_level_for_ratio`，
然后整张 tile 复用同一个 `SamplingLevel`，避免每个像元退回 base IFD。RasterTiler footprint
helper 增加显式 level 入口，保留旧入口作为 base-level wrapper；overview metadata 的
transform、NoData 和缓存 block 均沿同一 level 传递。当前 ratio 仍是纯 Rust 对内建 north-up
契约的近似，需与 C++ `GDALSuggestedWarpOutput2`/`getOverviewDataset` 做差分。

Rust 证据：新增 overview-only source 测试，若 RasterTiler 回退 base level 会直接失败；
`sample_values` 已验证整张 tile 复用 level 1，且全套测试 83 项、clippy 通过。

### P3：完成 Global Mercator 与投影

将已有 `TileGrid` 接入 RasterTiler、TerrainTiler 和四个 CLI；先支持 source/target 同为
EPSG:3857，再实现 EPSG:4326↔3857 的纯 Rust 坐标转换和反向采样。随后按矩阵加入 C++ 实测
需要的 CRS/WKT 表达；每种转换都要有控制点及输出 tile oracle。

完成标准：`-p mercator` 的 Terrain 与 RasterTiler 对照 C++ 的 z0/z1 及跨纬度样本一致。

### P4：输入、输出格式与可靠性

按 C++ oracle 中实际的 driver 清单，逐个实现纯 Rust 输入解码与 `CreateCopy` 输出。完成
BigTIFF、压缩、内部/外部 overview、格式错误处理、创建选项和可恢复写入；保留 C++ 的每
driver 文件扩展名与失败方式。

完成标准：兼容矩阵不存在未解释的原版可用路径；所有格式由纯 Rust 依赖或项目内实现支持。

### P5：全量审计

在无 GDAL/PROJ 的 CI 环境运行 Rust 测试；在隔离 oracle 环境运行 C++ 对照。输出版本化的
compatibility report，列出每个模块、参数组合、fixture、比较方式和已知差异；有未处理差异
即不宣布完成。

#### P5 实施记录 1：纯 Rust 依赖审计（已完成）

`cargo tree --all-features` 仅显示 `clap`、`flate2`、`geotiff-reader`、`geotiff-writer`、
`ndarray` 及其 Rust 传递依赖；没有 GDAL、PROJ、bindgen、cc/cxx GIS FFI 或系统 GIS 库。
源码中的 GDAL/PROJ 文本仅用于 C++ 行为注释、测试 fixture 和 oracle 脚本。该门禁已完成，
但不等同于 C++ 全量兼容审计完成。
