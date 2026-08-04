# ctb-rs 实施 TODO

本清单从 `TECHNICAL_PLAN.md` 派生。每个任务完成时必须同时更新技术方案、测试策略和本清单，
并附上 C++ 源码位置或 oracle 证据。

## P0：规格和现状审计（最高优先级）

- [x] 建立 C++ CTB commit、构建环境和 GDAL 版本的 oracle 记录（CTB
      `d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`、GDAL 3.13.2、CMake Release、`/usr/bin/c++`）；
      旧 CTB 与当前 GDAL API 不兼容，构建失败原因已记录。
- [ ] 完整盘点 `ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents` 的参数、默认值、输出与错误路径。
- [ ] 建立 `src/*` 到 C++ 类/函数的逐项映射，标记“已实现”“已 oracle 验证”“未实现”。
- [ ] 审计现有 Geodetic Terrain 和 GTiff 路径，消除与 C++ 不符的现有行为后才扩展功能。
- [ ] 将所有 fixture/oracle 元数据补入 `tests/fixtures/MANIFEST.md`。

## P1：通用 Grid 接入

- [x] 为 `RasterTileSamplePlan` 接入 `TileGrid`，不改变现有 Geodetic 结果（`RasterTileSamplePlan::from_grid`；Mercator destination-cell 单元测试）。
- [x] 将 `RasterTileset` 写入入口和内部 sample-plan 构造改为 `TileGrid`，保持 C++ RasterIterator 顺序与路径布局（`RasterIterator.hpp`/`GridIterator.hpp`；Rust z0 Mercator 过程测试）。
- [ ] 为 `TilesetPlan` 的通用 Grid 范围计算建立 C++ upper-right 边界 oracle。
- [x] RasterTileset 在写入前计算 TileGrid 范围，并支持内建 EPSG:4326↔3857 重投影；未知 CRS
      仍拒绝（`TilesetPlan::from_raster_with_tile_grid`、`raster.rs`；CLI 输出 CRS 测试）。
- [ ] 让 `ctb-tile -f GTiff -p mercator` 构造 Mercator Grid，并以 C++ 固定 EPSG:3857 direct-source z0/z1 的 paths、metadata 和 samples。
- [ ] 运行 Geodetic 无回归差分及新的 Mercator direct-source 测试。

## P2：GDAL VRT 等价

- [x] 实现并测试 `mode`、`med`、`q1`、`q3` 离散统计采样核；固定 row-major 窗口、空覆盖
      返回 0、mode 首次出现 tie-break 和 nearest-rank 分位数规则（`GDALTiler.cpp` 的
      `eResampleAlg` 分支；Rust 非平坦窗口单元测试；71 tests passed）。C++ 输出差分仍待补，
      不代表 P2 兼容性已完成。
- [x] 按 GDAL `gdalresamplingkernels.h` 实现 cubic、cubicspline、lanczos 的有限 kernel、
      边缘 tap 丢弃和权重归一化；用非平坦 source fixture 锁定数值（`sampling.rs`，72 tests
      passed）。C++ 差分、缩放和 NoData/density 仍待补。
- [x] 将 `scripts/verify-ctb-oracle.zsh` 的 resampling 矩阵扩展到 CLI 的全部 12 个算法；
      脚本通过 `zsh -n`。运行所需的 C++ oracle binary 尚未构建，差分仍待执行。
- [ ] 固定 `GDALTiler::createRasterTile` 的 GeoTransform、destination 初始化和 band 行为 oracle。
- [ ] 以 `TerrainTiler::terrainTileBounds` 验证 terrain 重叠坐标和第 66 个边缘样本。
- [ ] 用 `getOverviewDataset` 的 SuggestedWarp 中间值建立 overview 选择 oracle。
- [ ] 实现并验证内部 overview 的 level-aware 读取与有界缓存。
- [ ] 补齐 RasterTiler 的 nearest、bilinear、cubic、cubicspline、lanczos、average、mode、max、min、med、q1、q3；每种算法先有非平坦 C++ oracle。
- [ ] 对 integer/float、NoData 和 source 覆盖外的转换顺序逐项差分。

## P3：Mercator 与重投影

- [x] 将 GlobalMercator 接入 `ctb-tile` 的 Terrain 与 RasterTiler 分支（Raster/Terrain Rust
      CLI 已覆盖；C++ payload 差分待补）。
- [ ] 固定 EPSG:4326↔3857 的控制点、轴顺序、纬度范围和 C++ tile oracle（Rust 控制点已
      覆盖，C++ 差分待补）。
- [x] 实现纯 Rust 4326↔3857 source/target 坐标变换及反向采样，覆盖 RasterTiler 目标像素
      中心/footprint（`raster.rs`、`raster_sampling.rs`、CLI；74 tests passed）；TerrainTiler
      已接入 `TerrainSamplePlan` 和 factory writer，C++ 差分仍待完成。
- [ ] 根据 C++ oracle 矩阵登记并实现后续实际需要的 CRS/WKT 表达。

## P4：格式与 CLI 全量兼容

- [x] 收敛 `ctb-info -e` 输出换行/尾部空格，并让 `ctb-extents` 通过 `TileGrid` 支持
      geodetic/mercator（C++ `tools/ctb-info.cpp`、`ctb-extents.cpp`；CLI golden tests；72
      tests passed）。C++ 逐字节差分与重投影输入仍待补。
- [ ] 按 C++ 可用 driver 建立输入格式、输出 format、extension 和 creation option 矩阵。
- [ ] 完成 GTiff creation options、样本类型和 metadata 的全部已登记组合；当前 None、
      DEFLATE、LZW 已实现，PackBits 受 writer API 限制；C++ 字节差分和其他 options 仍待补。
- [ ] 逐 driver 以纯 Rust 实现 C++ `CreateCopy` 路径；每个 driver 有独立 oracle。
- [ ] 覆盖 BigTIFF、常用压缩、strip/tile、内部/外部 overview 与损坏文件。
- [ ] 对四个 CLI 完成 help、成功、参数错误、I/O 错误、quiet/verbose/thread/resume 差分。
- [ ] 补齐 `ctb-tile -z/--error-threshold` 与 `-m/--warp-memory`：当前先解析默认值并对
      非默认值显式报未实现错误；待 C++ oracle 可运行后再验证 ApproxTransformer 和 warp
      memory 对结果/性能契约的实际影响。
- [x] 校正 `ctb-tile`/`ctb-extents` 的 profile 默认 tile size（Terrain 65、非 Terrain 256），
      并拒绝 Terrain 的 `--creation-option`（C++ `ctb-tile.cpp`、`ctb-extents.cpp`；76 tests
      passed）。C++ CLI 差分仍待补。

## P5：完成门禁

- [x] 无 GDAL/PROJ/FFI GIS 依赖的 `cargo tree --all-features` 审计通过；纯 Rust 依赖树已
      记录在 `TECHNICAL_PLAN.md`，C++ oracle 构建环境另行记录。
- [ ] Rust 单元、集成、CLI、多线程和差分测试全绿。
- [ ] C++ 全量兼容矩阵没有未解释条目。
- [ ] 生成并提交版本化兼容性报告；任何未实现 C++ 路径不得标记为完成。
