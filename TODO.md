# ctb-rs 实施 TODO

本清单从 `TECHNICAL_PLAN.md` 派生。每个任务完成时必须同时更新技术方案、测试策略和本清单，
并附上 C++ 源码位置或 oracle 证据。

## P0：规格和现状审计（最高优先级）

- [x] 用已修复的 `/Users/sander/coding/cesium-terrain-builder/build-with-gdal.sh` 重新构建 C++
      oracle；产物为 `build-gdal-v3.11.4/tools/{ctb-tile,ctb-info,ctb-export,ctb-extents}`，
      GDAL `3.11.4`、C++ commit `d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`。旧的 GDAL API
      构建阻塞已解除。
- [ ] 完整盘点 `ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents` 的参数、默认值、输出与错误路径。
- [ ] 建立 `src/*` 到 C++ 类/函数的逐项映射，标记“已实现”“已 oracle 验证”“未实现”。
- [ ] 审计现有 Geodetic Terrain 和 GTiff 路径，消除与 C++ 不符的现有行为后才扩展功能。
- [ ] 将所有 fixture/oracle 元数据补入 `tests/fixtures/MANIFEST.md`。

## P1：通用 Grid 接入

- [x] 为 `RasterTileSamplePlan` 接入 `TileGrid`，不改变现有 Geodetic 结果（`RasterTileSamplePlan::from_grid`；Mercator destination-cell 单元测试）。
- [x] 将 `RasterTileset` 写入入口和内部 sample-plan 构造改为 `TileGrid`，保持 C++ RasterIterator 顺序与路径布局（`RasterIterator.hpp`/`GridIterator.hpp`；Rust z0 Mercator 过程测试）。
- [ ] 为 `TilesetPlan` 的通用 Grid 范围计算建立 C++ upper-right 边界 oracle（Rust upper-edge
      回归已覆盖）。
- [x] RasterTileset 在写入前计算 TileGrid 范围，并支持内建 EPSG:4326↔3857 重投影；未知 CRS
      仍拒绝（`TilesetPlan::from_raster_with_tile_grid`、`raster.rs`；CLI 输出 CRS 测试）。
- [x] 让 `ctb-tile -f GTiff -p mercator` 构造 Mercator Grid（Rust z0/z1 路径与 metadata 已覆盖）；
      C++ 固定 EPSG:3857 direct-source 的 paths/samples 差分仍待补。
- [ ] 运行 Geodetic 无回归差分及新的 Mercator direct-source C++ oracle 测试；Geodetic
      direct Terrain 已通过 4 组输入 × 12 算法 × 2 range。high-resolution overview 的 24
      组差异根因已定位（P0 记录 4），修复后应可达全 120 组通过，随后继续 Mercator。
      现 120/120 组通过。

## P2：GDAL VRT 等价

- [x] 定位 high-resolution-overview 24 组 terrain payload 差异根因：C++
      `GDALTiler::createRasterTile` overview 路径不更新 `psWarpOptions->hSrcDS`，warp
      transformer 用 overview 坐标但从主数据集读数据（TECHNICAL_PLAN P0 记录 4）。
- [x] 实现根因修复：`sampling_level_for_ratio` 返回 `level: 0` 但保留 overview metadata，
      复现 C++ 从主数据集读取的 warp 行为。修复后 119/120 组通过。
- [x] 实现 warp 工作数据类型整数舍入：GDAL 将源 band 的 Int32 类型传播为 warp working
      data type，平均结果经 `floor(x+0.5)` 舍入后写入（TECHNICAL_PLAN P0 记录 5）。
      Rust `sampling.rs` 已在 `sample_with_footprint_level` 和
      `sample_with_footprint_raster_tiler_level` 返回前按 `sample_type` 舍入。
      全 120 组 oracle 矩阵逐字节通过。
- [x] 实现并测试 `mode`、`med`、`q1`、`q3` 离散统计采样核；固定 row-major 窗口、空覆盖
      返回 0、mode 首次出现 tie-break 和 nearest-rank 分位数规则（`GDALTiler.cpp` 的
      `eResampleAlg` 分支；Rust 非平坦窗口单元测试；71 tests passed）。C++ 输出差分仍待补，
      不代表 P2 兼容性已完成。
- [x] 按 GDAL `gdalresamplingkernels.h` 实现 cubic、cubicspline、lanczos 的有限 kernel、
      边缘 tap 丢弃和权重归一化；用非平坦 source fixture 锁定数值（`sampling.rs`，72 tests
      passed）。C++ 差分、缩放和 NoData/density 仍待补。
- [x] 修正 `filtered_sample` tap 范围为 GDAL `nFiltInitX..=nXRadius`
      （`((radius+1)%2)-radius ..= radius`），并为 `Cubic` 加入 4-sample 边界 bilinear 回退
      （对应 `GWKCubicResample4Sample` 的 `iSrcX-1..iSrcX+2` 越界检查）。2×2 fixture GTiff
      oracle 由 110/120 收敛至 120/120（TECHNICAL_PLAN P0 记录 8；`sampling.rs`）。cubicspline
      的界内 NoData density 回退仍归 P2 NoData fixture。
- [x] 将 `scripts/verify-ctb-oracle.zsh` 的 resampling 矩阵扩展到 CLI 的全部 12 个算法；
      脚本通过 `zsh -n`。使用恢复后的 C++ oracle 执行并记录 12 个算法的数值差异。
- [ ] 固定 `GDALTiler::createRasterTile` 的 GeoTransform、destination 初始化和 band 行为 oracle。
- [ ] 以 `TerrainTiler::terrainTileBounds` 验证 terrain 重叠坐标和第 66 个边缘样本。
- [ ] 以 C++ TerrainTiler 的固定 `GRA_Average` 验证所有 `-r` 名称输出相同；Rust 写入入口已
      固定 Average，待完整 oracle 矩阵通过后关闭。
- [ ] 用 `getOverviewDataset` 的 SuggestedWarp 中间值建立 overview 选择 oracle；已观察到
      high-resolution overview case 的目标/source resolution ratio 差异，Rust 已改为按目标
      tile resolution 选择，仍需完整矩阵回归。
- [x] 实现并验证 Terrain/RasterTiler 的内部 overview level-aware 读取与有界缓存（Rust
      fixture 已覆盖，Rust 83 tests passed；C++ SuggestedWarp ratio 差分仍待补）。
- [x] 补齐 RasterTiler 的 nearest、bilinear、cubic、cubicspline、lanczos、average、mode、max、
      min、med、q1、q3 Rust 分支；复用现有核函数和离散统计实现，非平坦 Rust fixture 待补。
- [x] 对上述 12 个 RasterTiler 算法分别生成非平坦 C++ oracle 并完成数值差分；16×16
      fixture 已覆盖 Int32（144/144）、Int32+NoData（144/144）、Float32（144/144）、
      Mercator 同 CRS（90/90）和跨 CRS 4326→3857（50/50）；Terrain 全量矩阵 120/120。
- [x] 对 integer/float、NoData 的转换顺序逐项差分；Int32、Float32、Int32+NoData
      oracle 均通过。source 覆盖外的 destination 初始化仍待补。
- [x] 完成 RasterTiler footprint 核的 source-outside destination 初始化差分；plain z0 GTiff
      tile-size-16 的 12 算法均通过 C++，7 个统计核通过中心 bounds 门禁修复；NoData 和
      overview 仍单独验证。
- [x] 将 center-bounds 门禁从全部算法收窄到 center-based 算法（nearest/bilinear/cubic/
      cubicspline/lanczos），footprint 算法（average/mode/max/min/med/q1/q3）不再受门禁约束，
      匹配 GDAL `GWKGeneralCase` vs `GWKAverageOrMode` 差异（TECHNICAL_PLAN P0 记录 7；
      `sampling.rs`；12/12 RasterTiler GTiff oracle 像素匹配）。
- [x] 为 footprint 算法增加 GDAL `GWKAverageOrModeThread` margin gate：将目标像元角点变换到
      source pixel 坐标后检查是否在 `[-nXMargin, nSrcSize+nXMargin]` 内
      （`gdalwarpkernel.cpp:6681-6754`；`nXMargin = 2*max(1,ceil(1/dfXScale))`；
      `dfXScale = tile_size / level.data_width`）。16×16 fixture 的 14 组 footprint 边界差异
      由本修复消除（TECHNICAL_PLAN P0 记录 9 根因 A；`sampling.rs`）。
- [x] 将 `average_at` 的几何 overlap 权重替换为 GDAL `COMPUTE_WEIGHT` / `COMPUTE_WEIGHT_Y`
      宏公式（`gdalwarpkernel.cpp:6838-6849`），边界像元权重使用 `[dfXMin, iSrcX+1]` 线性长度
      而非 clipped overlap（TECHNICAL_PLAN P0 记录 9 根因 B；`sampling.rs`）。
- [x] 将 destination centre 计算从 `(min_x + max_x) / 2.0` 改为
      `bounds.min_x + (column + 0.5) * resolution`（等价 GDAL `GenImgProjTransformer`
      `(iDstX + 0.5) * res + origin`），消除末位 ULP 差异在 bilinear 4-sample 中传播导致的
     整数舍入偏差（TECHNICAL_PLAN P0 记录 9 根因 C；`raster_sampling.rs`）。
- [x] 将 `bilinear` 从可分离插值改为 GDAL `GWKBilinearResample4Sample` 预乘角点权重直接累加
      （`acc = UL*(rx*ry) + UR*((1-rx)*ry) + LL*(rx*(1-ry)) + LR*((1-rx)*(1-ry))`；
      `gdalwarpkernel.cpp:2675-2683`），消除累加序差异在 px(5,54) 处的 1-ULP 舍入偏差
      （TECHNICAL_PLAN P0 记录 10 根因 D；`sampling.rs`）。
- [x] 将 cubic 分支改用 GDAL `GWKCubicComputeWeights` 系数公式（`gdalwarpkernel.cpp:2946-2956`）
      + 分离 CONVOL4 结构（先横向再纵向），替换 Rust `kernel_weight` 非分离 2D 卷积
      （`gdalwarpkernel.cpp:3015-3047`；TECHNICAL_PLAN P0 记录 10 根因 E；`sampling.rs`）。
- [x] 将 `average_at` 的 footprint 来源从世界坐标像元边界改为 source_center ± 0.5
      （GDAL `padfX ± 0.5`；`gdalwarpkernel.cpp:6810-6811`），消除非对称权重导致的 1-ULP
     舍入偏差（TECHNICAL_PLAN P0 记录 10 根因 F；`sampling.rs`）。
- [x] 将正向 GeoTransform 坐标计算从 `origin + pixel * res` 改为
      `pixel.mul_add(res, origin)`（Y 轴 `pixel.mul_add(-res, max_y)`），复制 GDAL
      `GDALApplyGeoTransform` 在 clang ARM64 上的 FMA contraction 行为。像元角点
      `min_x`/`max_x`/`min_y`/`max_y` 同理改为 `mul_add`（`gdaltransformer.cpp:3124-3140`；
      TECHNICAL_PLAN P0 记录 11 根因 G；`raster_sampling.rs`）。
- [x] 将逆向 GeoTransform 坐标计算从 `(world - origin) / pixel_width` 改为
      `GDALInvGeoTransform` 预计算倒数 + `mul_add`（`inv_pw = 1.0 / pw;
      inv_ox = -origin / pw; pixel = world.mul_add(inv_pw, inv_ox)`），匹配 GDAL
      `GDALInvGeoTransform` + FMA 内联应用（`gdaltransformer.cpp:3162-3168, 4576-4588`；
      TECHNICAL_PLAN P0 记录 11 根因 G；`sampling.rs`）。
- [x] 将 `average_at` 累加循环从 `sum += sample * df_weight; … sum / total_weight` 改为
      GDAL 加权增量算法（`total_weight += df_weight; value += (df_weight / total_weight)
      * (sample - value)`），并使用 `mul_add` 匹配 clang FMA contraction（TECHNICAL_PLAN
      P0 记录 12 根因 H；`gdalwarpkernel.cpp:7016-7086`；`sampling.rs`）。
- [x] 用含多个 NoData 像元的 fixture 验证 GDAL CTB warp 路径的 NoData 处理：
      GDALCreateWarpedVRT 不设 padfSrcNoDataReal，因此 NoData 像元作为普通值传入
      所有采样算法（nearest 输出 NoData 原值，average 纳入 NoData，min 选 NoData）。
      移除 `geotiff.rs::mark_nodata` 后 16×16 NoData fixture 达到 144/144，原始无
      NoData fixture 仍为 144/144（TECHNICAL_PLAN P0 记录 13 根因 I）。
      Terrain 编码和 Float32 NaN 源的过滤差异仍待补。

## P3：Mercator 与重投影

- [x] 将 GlobalMercator 接入 `ctb-tile` 的 Terrain 与 RasterTiler 分支（Raster/Terrain Rust
      CLI 已覆盖；C++ payload 差分待补）。
- [x] 固定 EPSG:4326↔3857 的控制点、轴顺序、纬度范围和 C++ tile oracle（跨 CRS GTiff
      oracle 50/50 通过；Rust 控制点已覆盖）。
- [x] 完成 Mercator direct-source z0 upper-edge payload 回归；16×16 fixture 直同 CRS
      达到 90/90，跨 CRS 4326→3857 达到 50/50。
- [x] 修复 Terrain + Mercator 的 grid tile_size：C++ ctb-tile.cpp 按 profile 设默认
      tile_size（geodetic=65, mercator=256），terrain heightmap 的 TILE_SIZE=65
      是 config.hpp 编译期常量，与 grid tile_size 独立。Rust 旧实现硬编码
      GlobalMercatorGrid(65) 导致 max_zoom=4（应为 2）和采样点位置错误。
      修复后 TerrainSamplePlan 分离 grid tile_size 和 heightmap_size；
      CLI 和 terrain writer 移除 tile_size==65 门禁
      （TECHNICAL_PLAN P0 记录 14 根因 J；`terrain_sampling.rs`、`tileset.rs`、
      `src/bin/ctb-tile.rs`）。
- [x] 修复 Terrain child mask 计算：C++ 使用 source bounds 与 tile 四分之一象限的
      strict `<` overlaps 判定 child flag（`TerrainTiler.cpp:55-73`、`Bounds.hpp:222-227`），
      Rust 旧实现用 tile-coordinate child_mask_for 会错误包含边界相切的 tile。新增
      `terrain_child_mask` 和 `strict_overlaps` 辅助函数并接入两个 terrain writer；
      `max_zoom` 使用自然 max（`grid.zoom_for_resolution`）而非 `plan.max_zoom`
      （TECHNICAL_PLAN P0 记录 15 根因 K；`tileset.rs`）。
- [ ] 记录 Mercator 2×2 source 的 Terrain expanded bounds、65 个目标像元中心及 C++ `GWKAverageOrMode` source window，解释中心边界四样本 `5500/6000` 对 `6500/7000` 的差异；overview warp 复用同一坐标审计，不引入未经 oracle 证明的 epsilon。
- [x] 实现纯 Rust 4326↔3857 source/target 坐标变换及反向采样，覆盖 RasterTiler 目标像素中心/footprint（`raster.rs`、`raster_sampling.rs`、CLI；74 tests passed）；TerrainTiler 已接入 `TerrainSamplePlan` 和 factory writer，C++ 差分仍待完成。
- [x] 对 EPSG:4326→3857 正向变换补齐有效纬度裁剪，并用超范围控制点和 tile 边界测试验证
      （Rust 78 tests passed）；C++ GDAL 数值差分仍待补。
- [ ] 根据 C++ oracle 矩阵登记并实现后续实际需要的 CRS/WKT 表达；当前内建 EPSG:4326/3857 已实现。

## P4：格式与 CLI 全量兼容

- [x] 收敛 `ctb-info -e` 输出换行/尾部空格，并让 `ctb-extents` 通过 `TileGrid` 支持
      geodetic/mercator（C++ `tools/ctb-info.cpp`、`ctb-extents.cpp`；CLI golden tests；72
      tests passed）。C++ 逐字节差分与重投影输入仍待补。
- [x] 按 C++ 可用 driver 建立输入格式、输出 format、extension 和 creation option 矩阵：
      C++ 使用 GDAL 的多 driver 体系；纯 Rust 端已实现 GeoTIFF 输入（geotiff-reader）
      和 GeoTIFF/Terrain 输出（geotiff-writer / gzip）。其余 GDAL driver 待 C++ oracle
      实测需要时按优先级翻译。
- [x] 完成 GTiff creation options、样本类型和 metadata 的像素数据 oracle：NONE/DEFLATE/LZW
      + PREDICTOR=1/2 + TILED=YES/NO 共 132 个 tile 逐像素一致。PREDICTOR=3 对整数数据
      被 C++ GDAL 和 Rust 均正确拒绝。TIFF 容器 tag 序列化字节差异为已知格式实现差异。
      DEFLATE、LZW、ZSTD、JPEG、LERC、BIGTIFF、PREDICTOR 已实现，PackBits 受 writer API 限制；C++ 字节差分和其他 options 仍待补。
- [x] 接入 GTiff `TILED=YES/NO`、`BLOCKXSIZE/BLOCKYSIZE`，并覆盖 block 约束测试（Rust
      82 tests passed）。
- [ ] 完成上述 options 与 C++ GTiff CreateCopy 的 layout tags/metadata 差分。
- [x] 用真实含 overview 的 GeoTIFF fixture 验证 overview 数量、选择边界、缩放 GeoTransform
      和 level-aware window 读回（Rust 80 tests passed）；当前实现存在，C++ SuggestedWarp
      差分仍待补。
- [ ] 逐 driver 以纯 Rust 实现 C++ `CreateCopy` 路径；每个 driver 有独立 oracle。
- [x] 覆盖 BigTIFF、常用压缩、strip/tile 的像素数据验证：BigTIFF=YES/NO/IF_NEEDED、
      COMPRESS=NONE/DEFLATE/LZW/ZSTD、TILED=YES/NO 像素数据已验证一致。
- [ ] 对四个 CLI 完成 help、成功、参数错误、I/O 错误、quiet/verbose/thread/resume 差分。
- [x] 完成四个 CLI --version 差分：C++ 0.4.1 = Rust 0.4.1。help 文本选项语义一致，
      排版格式因 clap vs getopt 不同（已知差异）。
      clap 的格式化帮助仍待 golden 收敛。
- [ ] 补齐 `ctb-tile -z/--error-threshold` 与 `-m/--warp-memory`：当前先解析默认值并对
      非默认值显式报未实现错误；待 C++ oracle 可运行后再验证 ApproxTransformer 和 warp
      memory 对结果/性能契约的实际影响。
- [x] 校正 `ctb-tile`/`ctb-extents` 的 profile 默认 tile size（Terrain 65、非 Terrain 256），
      并拒绝 Terrain 的 `--creation-option`（C++ `ctb-tile.cpp`、`ctb-extents.cpp`；76 tests
      passed）。C++ CLI 差分仍待补。
- [x] 将 RasterTiler 默认 tile size 改为 profile-based（geodetic=65、mercator=256），匹配 C++
      `ctb-tile.cpp:503-507` 按 profile 而非输出格式设默认值的逻辑（TECHNICAL_PLAN P0 记录 6；
      `profile_default_tile_size()`；Terrain 仍固定 65 且拒绝显式非 65，待 P3 mercator terrain
      grid 路径完成后统一处理）。

## P5：完成门禁

- [x] 无 GDAL/PROJ/FFI GIS 依赖的 `cargo tree --all-features` 审计通过；纯 Rust 依赖树已
      记录在 `TECHNICAL_PLAN.md`，C++ oracle 构建环境另行记录。
- [x] Rust 单元、集成、CLI、多线程和差分测试全绿（85 tests, clippy clean）。
- [x] 生成并提交版本化兼容性报告（TECHNICAL_PLAN P5 记录 2）；Terrain 120/120、
      GTiff 572/572、ctb-info/extents/export 像素级通过。
- [ ] C++ 全量兼容矩阵没有未解释条目；GTiff 容器元数据和 creation options 字节差分仍待补。
