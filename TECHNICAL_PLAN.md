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
| `GlobalGeodetic`、`GlobalMercator` | `grid` | 公式已落地；Mercator 尚未接入 RasterTiler/CLI。 |
| `GDALTiler`、`GDALTile`、`gdaloverviewdataset` | `raster`、`geotiff`、`sampling`、`cache` | 用纯 Rust RasterSource、坐标变换、窗口/overview 选择和采样顺序逐项等价；不能以新的“优化型”数据流替换 C++ 行为。 |
| `RasterTiler`、`RasterIterator` | `raster_sampling`、`raster_tileset` | 已有 geodetic GTiff 子集；改为通用 Grid 后补齐 profile、算法、格式和 creation options。 |
| `TerrainTiler`、`TerrainTile`、`TerrainIterator` | `terrain`、`terrain_sampling`、`tileset` | heightmap-1.0 路径已存在；继续以 `terrainTileBounds`、Float32 读回、`uint16_t((h+1000)*5)` 和 child 逻辑逐项复核。 |
| `ctb-tile` | `src/bin/ctb-tile.rs` | 参数骨架、Terrain 和 GTiff 子集存在；必须完成完整参数矩阵与原版分支。 |
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

### P1：收敛既有 Geodetic 路径

先对 Terrain heightmap 和 `-f GTiff` 的 EPSG:4326 direct-source 路径逐项比对：tile range、
terrain overlap、全部 12 个 resampling 名称的分支、样本类型、NoData、creation options、
quiet/verbose/resume 和错误文本。先修正任意与 C++ oracle 不符的既有实现，再扩展功能。

完成标准：已支持路径的裸 terrain payload 逐字节相同；GTiff 的路径、栅格、CRS、transform、
样本类型和 NoData 相同；全部差异有明确 C++ 证据。

#### P1 实施记录 1：RasterTiler 的通用 Grid 写入边界（已实现，尚待 C++ 差分）

`RasterTileSamplePlan::from_grid` 已按 `GDALTiler::createRasterTile` 所持有的 `const Grid &`
计算目标像素中心和 footprint。下一步将 `RasterTileset` 的公开写入入口同样改为接收
`&dyn TileGrid`，并只通过 `TilesetPlan::from_raster_with_tile_grid` 生成范围；不匹配的 source
CRS 必须在任何输出目录写入之前被拒绝。Tile 队列继续严格映射 `GridIterator`：每层 x 递增、
每个 x 内 y 递增，且调用端按最高 zoom 到最低 zoom 消费。`ctb-tile -f GTiff -p mercator`
据此构造 `GlobalMercatorGrid`；Terrain 分支在 P3 前仍明确拒绝 Mercator，不能误用 Geodetic。

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
