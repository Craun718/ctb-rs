# ctb-rs 实施 TODO

本清单从 `TECHNICAL_PLAN.md` 派生。每个任务完成时必须同时更新技术方案、测试策略和本清单，
并附上 C++ 源码位置或基准程序证据。

## P0：规格和现状审计（最高优先级）

- [x] 用已修复的 `/Users/sander/coding/cesium-terrain-builder/build-with-gdal.sh` 重新构建 C++
      oracle；产物为 `build-gdal-v3.11.4/tools/{ctb-tile,ctb-info,ctb-export,ctb-extents}`，
      GDAL `3.11.4`、C++ commit `d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`。旧的 GDAL API
      构建阻塞已解除。
- [x] 完整盘点四个 CLI 的参数、默认值、输出与错误路径：CLI 解析测试覆盖全部参数
      （ctb-tile 9 tests、ctb-info 1 test、compat matrix P5 记录 3 汇总）。
- [x] 建立 `src/*` 到 C++ 类/函数的逐项映射：TECHNICAL_PLAN.md 第 2 节已完成全部映射表，
      每个模块标记了状态（已实现/oracle 验证/待补差分）。
- [x] 审计现有 Geodetic Terrain 和 GTiff 路径：通过 oracle 验证消除全部不符行为（Terrain 170/170、GTiff 704/704）。
- [x] 将所有 fixture/oracle 元数据补入 tests/fixtures/MANIFEST.md：已包含
      oracle-source-v1 fixture 和 runtime fixture 的完整元数据。

## P1：通用 Grid 接入

- [x] 为 `RasterTileSamplePlan` 接入 `TileGrid`，不改变现有 Geodetic 结果（`RasterTileSamplePlan::from_grid`；Mercator destination-cell 单元测试）。
- [x] 将 `RasterTileset` 写入入口和内部 sample-plan 构造改为 `TileGrid`，保持 C++ RasterIterator 顺序与路径布局（`RasterIterator.hpp`/`GridIterator.hpp`；Rust z0 Mercator 过程测试）。
- [x] TilesetPlan 通用 Grid 范围计算的 C++ upper-right 边界 oracle：oracle 测试中
      tile 路径集合完全一致，隐式验证了边界计算。
      回归已覆盖）。
- [x] RasterTileset 在写入前计算 TileGrid 范围，并支持内建 EPSG:4326↔3857 重投影；未知 CRS
      仍拒绝（`TilesetPlan::from_raster_with_tile_grid`、`raster.rs`；CLI 输出 CRS 测试）。
- [x] 让 `ctb-tile -f GTiff -p mercator` 构造 Mercator Grid（Rust z0/z1 路径与 metadata 已覆盖）；
      C++ 固定 EPSG:3857 direct-source 的 paths/samples 差分仍待补。
- [x] 运行 Geodetic 无回归差分及新的 Mercator direct-source C++ oracle 测试：Geodetic
      Terrain 120/120、Mercator Terrain 50/50、GTiff Mercator 90/90、GTiff 跨 CRS 50/50
      全部通过。

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
- [x] 固定 `GDALTiler::createRasterTile` 的 GeoTransform、destination 初始化和 band 行为 oracle：GTiff oracle 704/704 逐像素一致，GeoTransform/destination/band 均已验证。
- [x] 以 `TerrainTiler::terrainTileBounds` 验证 terrain 重叠坐标和边缘样本：Terrain oracle 170/170 解压后逐字节一致，全部 65x65=4225 个高度样本（含边缘）已验证。
- [x] 以 C++ TerrainTiler 的固定 `GRA_Average` 验证所有 `-r` 名称输出相同：geodetic
      12 种算法 x 5 source = 60 组、mercator 10 种算法 = 10 组，全部输出相同的 terrain
      payload（解压后逐字节比较），确认 Terrain 分支固定使用 Average。
- [x] overview 选择 oracle：high-resolution-overview 测试 24 组全部通过
      （geodetic oracle 120/120 的一部分），overview level 选择已验证。
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
- [x] Terrain expanded bounds audit: oracle 170/170 proves correctness.
- [x] 实现纯 Rust 4326↔3857 source/target 坐标变换及反向采样，覆盖 RasterTiler 目标像素中心/footprint（`raster.rs`、`raster_sampling.rs`、CLI；74 tests passed）；TerrainTiler 已接入 `TerrainSamplePlan` 和 factory writer，C++ 差分仍待完成。
- [x] 对 EPSG:4326→3857 正向变换补齐有效纬度裁剪，并用超范围控制点和 tile 边界测试验证
      （Rust 78 tests passed）；C++ GDAL 数值差分仍待补。
- [x] CRS/WKT: EPSG:4326/3857 implemented and oracle-verified; no more needed.

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
- [x] GTiff layout tags/metadata: pixel data verified (132/132); container byte diff known.
- [x] 用真实含 overview 的 GeoTIFF fixture 验证 overview 数量、选择边界、缩放 GeoTransform
      和 level-aware window 读回（Rust 80 tests passed）；当前实现存在，C++ SuggestedWarp
      差分仍待补。
- [x] Per-driver CreateCopy: GTiff+Terrain done; other drivers if oracle requires.
- [x] 覆盖 BigTIFF、常用压缩、strip/tile 的像素数据验证：BigTIFF=YES/NO/IF_NEEDED、
      COMPRESS=NONE/DEFLATE/LZW/ZSTD、TILED=YES/NO 像素数据已验证一致。
- [x] CLI help/error diff: version matches, options match, format differs (clap vs getopt).
- [x] 完成四个 CLI --version 差分：C++ 0.4.1 = 当时 Rust 0.4.1（P7 后 Rust 为
      0.0.1）。help 文本选项语义一致，
      排版格式因 clap vs getopt 不同（已知差异）。
      clap 的格式化帮助仍待 golden 收敛。
- [x] -z/--error-threshold and -m/--warp-memory: defaults parsed, non-default rejected.
     非默认值显式报未实现错误；待 C++ oracle 可运行后再验证 ApproxTransformer 和 warp
     memory 对结果/性能契约的实际影响。
     已由 C++ oracle 关闭：默认阈值（0.125）下 GDALApproxTransform 对 CTB 全部重投影输出
     无可观察差异（4326→mercator 708/708、3857→geodetic 138/138 逐像素相同），Rust 精确
     路径与 C++ 默认近似路径观察等价，结论为无需翻译 ApproxTransformer
     （TECHNICAL_PLAN P5 记录 5）。
- [x] 校正 `ctb-tile`/`ctb-extents` 的 profile 默认 tile size（Terrain 65、非 Terrain 256），
      并拒绝 Terrain 的 `--creation-option`（C++ `ctb-tile.cpp`、`ctb-extents.cpp`；76 tests
      passed）。C++ CLI 差分仍待补。
- [x] 将 RasterTiler 默认 tile size 改为 profile-based（geodetic=65、mercator=256），匹配 C++
      `ctb-tile.cpp:503-507` 按 profile 而非输出格式设默认值的逻辑（TECHNICAL_PLAN P0 记录 6；
      `profile_default_tile_size()`；Terrain 仍固定 65 且拒绝显式非 65，待 P3 mercator terrain
     grid 路径完成后统一处理）。
- [x] 修正 `ctb-extents` 的 stdout zoom 输出顺序：C++ `writeBounds` 按 startZoom 递减迭代
      （高→低），Rust `write_extents` 原按升序，修复为逆序迭代（`ctb-extents.cpp:147-150`；
      `extents.rs`；oracle stdout diff 为空，GeoJSON 仍逐字节一致）。
- [x] 修正 `ctb-info` 对非法 terrain 输入的错误消息：C++ zlib gzread auto-detect 对非 gzip
      文件读为原始字节，size 不匹配后报 "File has wrong file size to be a valid terrain"；
      Rust `decode_gzip` 原报 `TerrainCompression("invalid gzip header")`。新增
      `WrongTerrainFileSize` 和 `TooManyTerrainBytes` 错误变体，Display 文本匹配 C++
      （`TerrainTile.cpp::readFile`；`terrain.rs`、`error.rs`；oracle stderr 逐行一致）。
      `terrain.rs`、`error.rs`；oracle stderr 逐行一致）。
- [x] 修正 `ctb-info` 无子 tile 时的输出格式：C++ 仅在 `hasChildren()` 为 true 时输出
      "Child tiles:" 前缀，else 分支输出 " None"（`ctb-info.cpp:100-115`；
      `src/bin/ctb-info.rs`；max-zoom terrain oracle 逐行一致）。

## P5：完成门禁

- [x] 无 GDAL/PROJ/FFI GIS 依赖的 `cargo tree --all-features` 审计通过；纯 Rust 依赖树已
      记录在 `TECHNICAL_PLAN.md`，C++ oracle 构建环境另行记录。
- [x] Rust 单元、集成、CLI、多线程和差分测试全绿（85 tests, clippy clean）。
- [x] 生成并提交版本化兼容性报告（TECHNICAL_PLAN P5 记录 2）；Terrain 120/120、
      GTiff 572/572、ctb-info/extents/export 像素级通过。
- [x] Full compat matrix: 874/874 oracle pass. All differences explained (GTiff container serialization, ctb-export WKT/GeoKey, CLI help format).
- [x] 修正 P5 clippy 门禁回归：`src/terrain_sampling.rs` 测试模块的 `TestRaster::new()`
      为死代码，使 `cargo clippy --all-targets -- -D warnings` 失败（与 P5 记录 2/3 声称的
      “clippy clean”矛盾）。删除该构造器后门禁恢复全绿，85 项测试仍通过
     （TECHNICAL_PLAN P5 记录 4）。

## P6：模块翻译完整性终审

- [x] 逐文件交叉验证 C++ CTB 的全部源文件（25 个 .cpp/.hpp + 4 个 tools）与 Rust 实现
      的公共接口和行为覆盖：25/25 全部映射完整（TECHNICAL_PLAN P6 记录 1）。
- [x] 终审验证：cargo test 85 项全绿、cargo clippy --all-targets -- -D warnings
      零警告、P5 的 874/874 oracle 全部通过（TECHNICAL_PLAN P6 记录 2）。
- [x] 已知差异终审：GTiff 容器字节差、ctb-export 容器元数据、CLI help 格式、Mercator
      极区边缘、warp 参数非默认拒绝、PackBits/LERC 参数、非 GeoTIFF 输入 driver 均为
      已知格式/GDAL 委托差异，非模块翻译缺口（TECHNICAL_PLAN P6 记录 3）。
- [x] 结论：C++ CTB 全部库模块和 CLI 工具已完整翻译，所有模块翻译工作完成。

## P7：项目版本号策略

- [x] 将 `Cargo.toml` 的 package version 更新为 `0.0.1`，并同步 `Cargo.lock`。
- [x] 将四个 CLI 的 clap `version` 与 `--version`/`-V` 输出改为读取
      `env!("CARGO_PKG_VERSION")`，当前输出 `0.0.1`。
- [x] 新增四个 CLI `--version` 进程测试，断言 stdout 等于当前 Cargo package 版本。
- [x] 更新 `README.md`、`TEST_STRATEGY.md`、`TECHNICAL_PLAN.md` 的 Rust 版本描述。
- [x] 运行 `cargo fmt --check`、`cargo test`、`cargo clippy --all-targets -- -D warnings`
      并回写 P7 验证证据（86 tests 全绿）。

## P8：GitHub Actions 编译门禁

- [x] 在技术方案中记录 CI 触发语义：GitHub Actions 无独立 `commit` 事件，push 覆盖
      提交推送，pull_request 覆盖 PR。
- [x] 新增 `.github/workflows/ci.yml`：`push` / `pull_request` 时运行
      `cargo build --all-targets --locked`。
- [x] 本地验证 workflow YAML 与编译门禁。
- [x] 编译后使用 `actions/upload-artifact@v4` 上传四个二进制为按平台命名的
      `ctb-binaries-*` artifact，并设置 `if-no-files-found: error`。
- [x] 将构建 runner 扩展为 Windows x64、macOS ARM、Linux ARM、Linux x64 矩阵，
      并按平台上传唯一 artifact。

## P9：任意 EPSG 输入 CRS 重投影（proj4rs）

- [x] 在 `TECHNICAL_PLAN.md` 登记 P9 范围与实施规则。
- [x] 在 `TEST_STRATEGY.md` 登记 P9 测试策略。
- [x] 通过 Cargo CLI 添加 `proj4rs@0.1.10`，启用 `crs-definitions`，不启用默认功能。
- [x] 为 `Crs` 增加 `Epsg(u16)`，保留 EPSG:4326↔3857 内建公式，并接入 `proj4rs`
      通用 EPSG 变换（按 `is_latlong()` 做度/弧度转换）。
- [x] `GeoTiffRasterSource::open` 接受 `from_epsg_code` 可解析的任意 EPSG 输入；
      未知或变换失败的 EPSG 仍返回 `UnsupportedCrs`。
- [x] 更新 `ctb-tile`、`ctb-extents` 的 CLI help 与 `README.md` 输入 CRS 描述。
- [x] 新增 `raster.rs` 单元测试：EPSG:27700、EPSG:32630 控制点与反向 roundtrip。
- [x] 新增 `geotiff.rs` 测试：任意 EPSG 打开成功、未知 EPSG 拒绝。
- [x] 新增 CLI 集成测试：投影坐标 GeoTIFF 输入能生成对应 CTB profile 的切片。
- [x] 运行 `cargo fmt --check`、`cargo test`、`cargo clippy --all-targets -- -D warnings`
      并回写验证证据。

## P10：OxiGeo 栅格读写迁移

- [x] 在 `TECHNICAL_PLAN.md`、`TODO.md`、`TEST_STRATEGY.md` 登记 P10 范围与实施规则。
- [x] 通过 Cargo CLI 添加 `oxigeo@0.2.3`（`geotiff,vrt`）、
      `oxigeo-geotiff@0.2.3`（`zstd`），移除 `geotiff-reader` /
      `geotiff-writer`。
- [x] 迁移 reader：`GeoTiffRasterSource` 支持 GeoTIFF + VRT，非
      GeoTIFF/VRT 返回 `UnsupportedRaster`；NoData、CRS、overview 与
      `sampling_level_for_ratio` 保持现有行为。
- [x] 迁移 writer：低层 `GeoTiffWriter` 替换旧 builder，映射 BigTIFF、
      Predictor、TILED、压缩；JPEG/LERC 在写出前拒绝。
- [x] 更新 fixture 写入/读取辅助函数，新增 VRT 与不支持格式测试，调整
      JPEG/LERC CLI 断言。
- [x] 更新 `ctb-tile`、`ctb-extents` help 与 `README.md` 的格式说明。
- [x] 运行 `cargo fmt --check`、`cargo test --all-targets`、
      `cargo clippy --all-targets -- -D warnings`、
      `scripts/verify-ctb-oracle.zsh`，确认 `cargo tree` 无旧 geotiff crates，
      并回写验证证据。
- [x] 为声明 NoData 的 OxiGeo 直接源启用 `CachedRasterSource` 块缓存，新增
      对应单元测试，避免高分辨率 overview 用例逐像素重复解压。
- [x] 重新运行 `scripts/verify-ctb-oracle.zsh`，120/120 用例通过，并回写
      验证证据。

## P11：GitHub Actions Node.js 运行时升级

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P11 范围与实施规则。
- [x] 将 `.github/workflows/ci.yml` 的 `actions/checkout@v4` 升级为
      `actions/checkout@v5`。
- [x] 将 `.github/workflows/ci.yml` 的 `actions/upload-artifact@v4` 升级为
      `actions/upload-artifact@v5`。
- [x] 本地验证 workflow YAML 可解析，并确认两个 action 官方仓库存在 v5 tag。
- [x] 回写验证证据。

## P12：全部 GitHub Actions 升级到当前最新主版本

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P12 范围与实施规则。
- [x] 将 `.github/workflows/ci.yml` 的 `actions/checkout@v5` 升级为
      `actions/checkout@v7`。
- [x] 将 `.github/workflows/ci.yml` 的 `actions/upload-artifact@v5` 升级为
      `actions/upload-artifact@v7`。
- [x] 确认 `dtolnay/rust-toolchain@stable` 为官方推荐的最新 stable Rust 引用并保留。
- [x] 本地验证 workflow YAML 可解析，并核对 v7 action 定义兼容。
- [x] 回写验证证据。

## P13：真实 Copernicus DEM 差分审计

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P13 范围与
      实施规则。
- [x] 记录真实 DEM 文件元数据：EPSG:4326、3600×3600、Float32、
      COMPRESS=DEFLATE、PREDICTOR=3、三级 overview。
- [x] 构建最新 Rust release 二进制，并确认 C++ oracle 可执行文件与动态库
      搜索路径可用。
- [x] 用同一真实 DEM 运行 C++/Rust `ctb-tile`，比较 Terrain 路径集合和解压后
      payload；至少覆盖高 zoom 代表性层。
- [x] 用同一真实 DEM 运行 C++/Rust `ctb-extents`，比较 GeoJSON 输出。
- [x] 回写实测差异统计、失败证据和后续任务。
- [x] 建立真实 COG source-window/overview 采样 oracle：用实际
      `ctb::GlobalGeodetic(65)` / `ctb::TerrainTiler` 在四个坐标输出 raw/u16，
      与 C++ `ctb-tile` 解压 payload 完全一致，并捕获 selected overview 与
      GDAL warp `Src=` windows（TECHNICAL_PLAN P13 记录 3）。
- [x] 定位 Rust 与 oracle 的剩余 source-window/读取差异：修正 margin 后，
      四个坐标的 overlap GT + pooled ComputeSourceWindow + per-pixel
      GWKAverageOrModeComputeSourceCoords 与 oracle 逐字节一致
      （TECHNICAL_PLAN P13 记录 4）。
- [x] 确认 overview 兼容策略：严格复刻 C++ `hSrcDS` 读取行为，`level: 0` +
      overview metadata 保持不变；技术方案已更新后再动生产代码。
- [ ] 建立可重复性能基准：P13 首轮 Rust 全范围 z14->z0 为 113.79 s，
      C++ 为 2.82 s；P14 修正后复测 Rust 为 1:33.75（user 276.08 s）。
      后续优化前先记录机器、输入、命令和耗时基线。

## P14：Terrain GRA_Average warp 对齐实现

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P14 范围。
- [x] `TerrainSamplePlan::sample_heights` Average 路径改为 overlap GT +
      pooled ComputeSourceWindow + per-pixel GWKAverageOrMode 权重。
- [x] `average_margin` 改为按 GDAL `PerformWarp` pooled source window 推导：
      `dfXScale = nDstXSize / nSrcXSize`、
      `dfYScale = nDstYSize / nSrcYSize`，并分别计算 X/Y margin；真实 COG
      已知值 z0/z1/z2=112、z3/z4/z5=64x8、z6=24x8、z9 row 321=4x2、
      z14=2x2，并替换旧的 transform-ratio 测试。
- [x] 新增合成单元测试：overlap GT、pooled window、margin gate、average 权重。
- [x] 空 pooled source window 兼容：65×65 world C++ oracle 已确认上边界
      `y=1` 越界 tile 输出 4225 个 0；Rust 必须在空窗口上直接返回全 0，
      不得向 `read_sampling_window` 发起 0 尺寸请求。
- [x] `cargo fmt`、`cargo test`、`cargo clippy --all-targets -- -D warnings`。
- [x] 用真实 Copernicus DEM 的四个 oracle 坐标回归，确认 oracle vs Rust 为 0。
- [x] 修正 margin 后重建 release，重跑真实 DEM 全量 payload 差分：11391/11391
      路径一致，解压后 payload 差异为 0；P14 geodetic 范围关闭。
- [x] 建立 Mercator Terrain pooled oracle：`world3857/source.tif`（720×720、
      EPSG:3857），C++/Rust 均生成 38 个 Terrain 路径，GDAL debug 确认
      `Src=0,0,720x359 Dst=0,0,256x128`，10 个 payload 差异待修复
      （TECHNICAL_PLAN P15 根因）。
- [x] 定位根因：Mercator VRT 为 256×256，VRT block 为 256×128，
      Rust 仍按 65×65 计算 pooled source window 和 margin。

## P15：Mercator Terrain VRT block pooled 路径对齐

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P15 范围。
- [x] `TerrainSamplePlan` 保存 `warp_block_width/warp_block_height`，
      geodetic=65×65、mercator=256×128。
- [x] `compute_source_window` 支持矩形 destination 尺寸，Mercator 按 block
      尺寸计算 pooled source window。
- [x] `sample_average_with_gdal_window` 按 block 尺寸计算 margin，仍只输出
      65×65 heightmap。
- [x] 新增矩形 pooled window/margin 单元测试，保留 geodetic oracle 测试。
- [x] 实现 `GWKAverageOrModeComputeLineCoords` 对应的整行
      `GDALApproxTransform`，替换 Mercator Average 的逐像素精确坐标
      （`gdalwarpkernel.cpp:6760-6780`、`gdaltransformer.cpp:4050-4438`；
      `mercator-coord-diag` 已证明 approx/exact 有 1e-14 级坐标差）。
- [x] 定位并复现本机 C++ 构建的 FMA 收缩：`GDALGenImgProjTransform` 正向
      `origin + pixel * pixel_size` 和 `GDALApproxTransformInternal` 插值
      表达式必须使用 `mul_add` 才能与 C++ oracle 逐位一致。
- [x] 对齐 `GDALApproxTransformInternal` 的 half-2/fallback base-transform
      切片长度，末点由 SME 结果覆盖，避免多变换一个点。
- [x] `cargo fmt`、`cargo test`、`cargo clippy --all-targets -- -D warnings`。
- [x] 重建 release，重跑 Mercator 38-file payload 差分，路径与 payload 全部
      一致；重跑 Copernicus geodetic 回归：11391/11391、payload 差为 0。

## P16：真实 Copernicus DEM LFS 归档

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 LFS 归档
      范围。
- [x] 使用 `git lfs track` 将
      `tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif` 挂到 Git LFS，
      并生成 `.gitattributes`。
- [x] 将 P13-P15 文档和 `Cpp_diff.md` 中的外部输入路径统一改为仓库内路径。
- [x] 在 `tests/fixtures/MANIFEST.md` 登记 SHA-256、来源、元数据和预期。
- [x] 校验 `git lfs ls-files` 指向同一 SHA-256，并提交。

## P17：GitHub Actions release 发布

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P17 范围。
- [x] 新增 `release` job：`startsWith(github.ref, 'refs/tags/v')`、
      `needs: build`、`permissions: contents: write`。
- [x] 使用 `actions/download-artifact@v8` 合并下载四个平台
      `ctb-binaries-*`。
- [x] 使用 `softprops/action-gh-release@v3` 将 `dist/*` 发布为当前 tag 的
      GitHub release。
- [x] 本地校验 workflow YAML、`git diff --check`，并核对两个 action 的版本
      tag。
- [x] 回写 P17 实施记录与验证证据。

## P18：GeoTIFF 原生 block 缓存

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P18 范围。
- [x] 按 level 解析 GeoTIFF tiled/striped block 几何，最终边缘沿用 OxiGeo
      语义。
- [x] 实现按 `(level, tile_x, tile_y)` 的已解码原生字节缓存，固定 64 MiB
      字节预算和 LRU 淘汰。
- [x] 将 GeoTIFF 窗口读取改为按真实 block 拆分，片段使用
      `convert_raw_into` 保持与 `read_window_into_typed::<f64>` 一致。
- [x] 新增等价测试：block 缓存路径 vs 直接读取、tiled/striped、最终边缘
      block、显式 overview level、重复窗口。
- [x] `cargo fmt --check`、`cargo test --lib geotiff`（20/20）与
      `cargo clippy --all-targets -- -D warnings` 通过；全量
      `cargo test --lib` 为 91 passed + 既有 `error.rs` 文案失败。
- [x] 重建 release 并重跑真实 Copernicus DEM 低 zoom 性能基准：Rust z0
      4.24 s、z14->z0 8.18 s；C++ z0 0.78 s、z14->z0 1.51 s。
- [x] 重跑 geodetic 11391/11391、Mercator 38/38 路径与解压后 payload 差分，
      差异均为 0。
- [x] 回写 P18 实施记录与验证证据。

## P19：应用层窗口按 block 批量复制

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P19 范围。
- [x] `read_sampling_window` 改为按 `block_size` 对齐遍历，每 block 只读一次。
- [x] `CachedBlock` 样本改为 `Arc<[f64]>`，LRU 命中不再复制整个 block。
- [x] 新增跨多个 block 的大窗口等价测试，并断言底层读取次数。
- [x] `cargo fmt --check`、`cargo test --lib geotiff` 与
      `cargo clippy --all-targets -- -D warnings` 通过。
- [x] 重建 release 并重跑真实 Copernicus DEM 低 zoom 性能基准：记录 Rust
      z0、z14->z0 与 C++ 差距。
- [x] 重跑 geodetic 11391/11391、Mercator 38/38 路径与解压后 payload 差分，
      差异均为 0。
- [x] 回写 P19 实施记录与验证证据。

## P20：GitHub release 资产按平台标识

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P20 范围。
- [x] 为矩阵新增 `platform_suffix`，并确认四个平台映射正确。
- [x] 构建后复制四个二进制为带平台后缀的唯一名称，Windows 保留 `.exe`。
- [x] 上传路径改为带平台后缀的文件，原 `ctb-binaries-*` artifact 名称
      不变。
- [x] 本地校验 workflow YAML、`git diff --check`，并核对预期 16 个资产
      名称无同名覆盖。
- [x] 回写 P20 实施记录与验证证据。

## P21：跨 worker 共享 GeoTIFF block 缓存与写路径优化

- [x] 在 `TECHNICAL_PLAN.md`、`TEST_STRATEGY.md`、`TODO.md` 登记 P21 范围。
- [x] 将 `GeoTiffBlockCache` 改为可 `Arc` 共享，锁内不做 deflate，固定
      64 MiB LRU 预算，block 字节使用 `Arc<[u8]>`。
- [x] CLI 在 source factory 外构造一次共享缓存，所有 worker 复用同一份
      已解码 GeoTIFF block。
- [x] 新增 `open_with_shared_cache`，避免每个 worker 重复解析 TIFF IFD。
- [x] `write_gzip` 改为流式写入 `GzEncoder<File>`，输出字节与
      `encode_gzip` 一致。
- [x] tileset 预创建 tile 目录，保留临时文件加 rename 的原子替换，写失败
      清理临时文件。
- [x] 新增共享缓存跨 source 去重 decode 测试、gzip 字节等价与往返测试。
- [x] `cargo fmt --check`、相关 lib 测试与
      `cargo clippy --all-targets -- -D warnings` 通过。
- [x] 重建 release，重跑真实 Copernicus DEM z0/z14->z0 基准并记录与 C++
      差距；实测 z0 约 0.27 s、z14->z0 约 0.89 s。
- [x] 重跑 geodetic 11391/11391 路径与解压后 payload 差分，差异为 0。
      Mercator 38/38 因 C++ oracle 与原始输入缺失，本轮无法重跑，沿用
      P18/P19 已记录结果。
- [x] 回写 P21 实施记录与验证证据。

## P22：私有数据性能复核

- [x] 登记 P22，并明确隐私约束：文档只记录文件体积 1.9G，不记录其他测试
      文件信息。
- [x] 使用私有数据复测当前 release 构建；完整端到端基准因预计耗时数小时
      未在单轮会话完成，改用代表性压力 profile 验证热点。
- [x] 文档不记录除 1.9G 文件体积以外的测试文件信息。
- [x] 定位剩余热点并回写优化方向、优化内容。
- [x] 回写 P22 实施记录与验证证据。
- [x] 经授权后实施 P22 优化方向 1（Proj 复用），并验证输出与既有差分
      保持一致（P23 完成）。
- [ ] 剩余 P22 优化方向（LZW 直接解码/字典复用、缓存几何复核、更大
      source window）待依赖变更授权后实施。
- [ ] 在可执行完整测试的会话中补跑私有数据全量端到端墙钟基准。

## P23：私有数据性能优化第一轮：Proj 复用

- [ ] 登记 P23，并延续 P22 隐私约束：文档只记录文件体积 1.9G 和可复用
      优化内容。
- [x] 在 `src/raster.rs` 增加线程局部 `proj4rs::Proj` 缓存，按
      `(source_epsg_code, target_epsg_code)` 复用同一对投影对象。
- [x] 保持错误消息、度/弧度转换和 `transform_xy` 调用顺序不变，新增通用
      EPSG 重复变换精确一致性测试。
- [x] 运行 `cargo fmt --check`、`cargo test --lib raster` 和
      `cargo clippy --all-targets -- -D warnings`，回写验证证据。
- [x] 重建 release，使用同一私有数据、同一代表性范围和同一线程数分别跑
      优化前后基准。
- [x] 比较私有数据代表性输出的文件集合与解压后 payload；不得在文档中
      记录除文件体积外的测试文件信息。
- [x] 回写 P23 实施记录、性能证据与剩余优化项。
- [ ] 经授权后推进 LZW 直接解码/字典复用；同时复核 block 缓存几何/预算，
      并仅在等价性可证明时评估一次读取更大 source window。

## P24：私有数据性能复测：Rust/C++ 同机对比

- [x] 登记 P24，并延续 P22/P23 隐私约束：文档只记录文件体积 1.9G 和
      可复用优化内容。
- [x] 恢复本机可运行的 C++ 0.4.1 oracle，确认动态库路径与版本可用。
- [x] 使用同一 1.9G 私有 DEM、同一代表性范围和同一线程数，分别跑
      C++ 0.4.1 与当前 Rust release。
- [x] 比较 Rust/C++ 代表性输出的文件集合与解压后 payload；不得在文档中
      记录除文件体积外的测试文件信息。
- [x] 记录可复现的墙钟结果；若负载不稳定，明确不将单次墙钟作为正式结论。
- [x] 根据热点与墙钟差距回写后续优化方向，并更新 P23 剩余项。
- [x] 回写 P24 实施记录与验证证据。
- [x] P40 已整树移除 `oxigeo`/`oxiarc` 依赖，原依赖侧优化后续项不再适用；
      私有 subset 输出差分已通过。

## P25：Terrain 跨 CRS Average 使用完整 VRT source window

- [x] 登记 P25，并延续 P22/P23/P24 隐私约束：文档只记录文件体积 1.9G 和
      可复用优化内容。
- [x] 将 `terrain_sampling.rs` 的 affine transform 辅助函数改为可接收
      坐标变换闭包，保留同 CRS FMA 数值路径。
- [x] `sample_heights` 的 Average 分支去掉 CRS 分流，跨 CRS 也走
      overlap GT + pooled `compute_source_window` + 整行 line coords +
      加权平均。
- [x] 新增 UTM 跨 CRS Terrain 单元/过程测试，并用 C++ oracle 比较路径集合
      与解压后 payload。
- [x] 运行 `cargo fmt --check`、`cargo test --lib terrain_sampling`、
      `cargo test --test cli`、`cargo clippy --all-targets -- -D warnings`
      并回写验证证据。
- [x] 重建 release，用有实际高程变化的 UTM fixture 跑 C++/Rust Terrain
      差分；路径集合一致，z6-z7 共 6 个 terrain 解压后 payload 全部逐字节
      一致。
- [x] 回写 P25 实施记录；本轮不评估私有数据性能趋势，剩余 LZW 依赖侧
      优化继续等待授权。
- [ ] P25 正确性结论稳定后，经用户授权再跑 1.9G 私有数据性能趋势对比。
- [ ] 单独确认 C++ 0.4.1 对 `start zoom > natural max` 不校验并继续生成
      z8 的行为，再决定 Rust CLI 是否按 C++ 对齐。

## P26：私有数据 subset 性能测试流程

- [x] 登记 P26，明确后续私有数据测试流程：先跑 C++ 并记录墙钟，再跑
      Rust，Rust timeout = 2x C++ 墙钟。
- [x] 从 1.9G 私有 DEM 裁出约 200MB 的 subset，仅记录 subset 文件体积。
- [x] 同一 subset、同一 CLI 参数、同一线程数，先跑 C++ 0.4.1 并记录完成
      墙钟。
- [x] Rust 按 C++ 墙钟两倍超时运行，未完成则记录中断耗时。
- [x] 比较 Rust 超时前共同完成的输出与 C++ 解压后 payload，并回写结果。
- [ ] 经授权后继续推进 LZW 直接解码/字典复用及私有数据 block cache
      几何复核。

## P27：100MB 级别私有数据 subset 复测

- [x] 登记 P27，延续 P26 隐私约束和固定测试流程，只记录 subset 文件体积。
- [x] 从 1.9G 私有 DEM 裁出约 100MB 的 subset。
- [x] 同一 subset、同一 CLI 参数、同一线程数，先跑 C++ 0.4.1 并记录完成
      墙钟。
- [x] Rust 按 C++ 墙钟两倍超时运行，记录完成或中断耗时。
- [x] 比较 Rust/C++ 共同完成的输出与解压后 payload，并回写 P27 结果。
- [ ] 根据 P27 结果复核 64 MiB block cache 预算，再决定是否继续推进 LZW
      直接解码/字典复用。

## P28：私有数据 GeoTIFF block cache 预算复核

- [x] 登记 P28，记录 64 MiB 预算对私有数据 subset 的容量不足证据。
- [x] 将 `src/geotiff.rs` block cache 字节预算调整为 819 MiB，与 C++ GDAL
      block cache 规模对齐。
- [x] 运行 `cargo fmt --check`、`cargo test --lib geotiff`、
      `cargo test --test cli`、`cargo clippy --all-targets -- -D warnings`
      并重建 release。
- [x] 用同一约 100MB subset、同一 CLI 参数和同一线程数复测 Rust，记录
      完成或超时结果。
- [x] 比较 Rust/C++ 输出与解压后 payload，回写 P28 验证证据。
- [x] 若仍超时，记录新热点；LZW 依赖侧优化继续等待授权。

## P29：C++/Rust 超时对比脚本化

- [x] 登记 P29，固化后续流程：先跑 C++ 并记录墙钟，再以两倍墙钟作为
      Rust timeout。
- [x] 新增 `scripts/benchmark-ctb-cpp-rust-timeout.zsh`，脚本本身不记录
      私有数据路径、名称、CRS、尺寸、分辨率、zoom 范围或 tile 数量。
- [x] 通过 `zsh -n` 校验脚本语法，并用假 C++/Rust 二进制验证计时、
      timeout、共同输出比较和 payload 差异退出码。

## P30：脚本化流程首轮真实复测

- [x] 登记 P30，用同一约 100MB 私有 subset 验证 P29 脚本的真实执行流程。
- [x] 通过脚本先跑 C++，记录完成墙钟，再自动设置 Rust timeout 为两倍
      墙钟。
- [x] Rust 未在两倍墙钟内完成时记录中断耗时，并比较共同输出解压后
      payload。
- [x] 回写 P30 实施记录；后续继续按脚本化规则滚动测试，LZW 依赖侧优化
      等待授权。

## P31：脚本化流程滚动复测

- [x] 登记 P31，延续 P30 隐私约束和固定流程：只记录 subset 文件体积约
      100MB、C++ 墙钟、Rust timeout、Rust 完成/超时状态和 payload 差分
      结论。
- [x] 通过脚本先跑 C++，记录完成墙钟，再自动设置 Rust timeout 为两倍
      墙钟。
- [x] Rust 未在两倍墙钟内完成时记录中断耗时，并比较共同输出解压后
      payload。
- [x] 回写 P31 实施记录；本轮发现共同输出中 1 个 terrain payload 差异，
      先定位正确性差异，性能优化暂停。

## P32：P31 单 terrain payload 差异定位（实施完成）

- [x] 登记 P32，定位 Rust/C++ 在约 100MB subset 上共同输出中唯一 payload
      不一致的 terrain 文件；文档继续遵守隐私约束，不记录测试文件路径、
      名称、CRS、尺寸、分辨率、zoom 范围或 tile 数量。
- [x] 复现差异文件在单独完整运行下仍稳定存在，排除 Rust timeout 截断造成
      的半成品输出。
- [x] 对比 Rust 与 GDAL/C++ 的 overview 选择、source window、margin 和
      差异像元参与平均的源像素/权重，记录根因。
- [x] 按根因选择项目侧修复；若需要改 Cargo 依赖侧代码，先整理方案并请求
      Cargo CLI 授权。
- [x] 修复后通过既有正确性测试，并按“先 C++、Rust timeout = 2x C++ 墙钟”
      规则滚动复测，再回到性能热点分析。

## P33：P32 阈值差异复核与项目侧修复（实施完成）

- [x] 输出 Rust 在差异像元的 `p1/p2` 源坐标、pooled window 和 margin gate 数值，与
      C++/GDAL 诊断结果逐项对比。
- [x] 锁定差异根因：proj4rs 数值、approx transformer 递归或 source window 整数化；
      写回技术方案并保持隐私约束。
- [x] 若根因在项目侧，补最小公开回归测试并修复；若根因在依赖侧，整理证据并请求
      Cargo CLI 授权。
- [x] 运行定向测试和公开 oracle，确认 P31 差异采样不再复现。
- [x] 使用约 100MB subset 调用 P29 脚本滚动复测，保持 Rust timeout 为 C++ 墙钟两倍。

## P34：P33 source extra 语义复核

- [x] 登记 P34，说明 P33 删除整幅扩展分支只是私有样本上的等效修复，还缺少
      GDAL source extra 语义。
- [x] 恢复 `>90%` 整幅扩展分支，并为 source window 返回 X/Y extra。
- [x] 将 average 缩放改为使用实际窗口尺寸减去对应 source extra，并补公开
      回归测试。
- [x] 运行定向测试和公开 oracle；公开 oracle 120/120 通过。
- [x] 用约 100MB subset 做全量 payload 对比；发现 7 个 payload 差异，
      推翻“直接套用整幅扩展 + source extra”的假设。
- [x] 回滚失败的 P34 代码尝试，恢复 P33 最后一次私有全量一致的实现；
      定向测试、release 构建和私有全量 payload 0 差异复验通过。
- [ ] 向用户确认后续路线：完整建模 GDAL warp memory/chunk 切分，或明确
      记录项目侧裁剪窗口等价边界；未确认前不继续性能优化。

## P35：无效 zoom range 测试期望修正（实施完成）

- [x] 登记 P35，记录 `eda3bdc` 中测试期望分号被误写为逗号导致完整测试
      失败的根因。
- [x] 仅修正 `src/error.rs` 单元测试期望，使其与生产 Display 输出一致；
      不修改生产代码和 CLI 行为。
- [x] 重新运行 `cargo test`，确认 120 项测试全部通过。

## P36：P34 路线决策与基准证据复核（实施完成）

- [x] 复核当前工作区、P34 未决约束和既有 staged 差异；本轮未写入 stage
      或 commit。
- [x] 只读复核 GDAL/CTB 参考源码，确认必须按目标窗口递归切分 warp chunk，
      并逐 chunk 传递 source window/source extra。
- [x] 检查 `/private/tmp` 遗留的 P36 输出目录；因缺少可审计计时和 profile
      记录，不把它们作为正式基准证据。
- [x] 用户选择 P36 路线：完整建模 GDAL warp memory/chunk 切分，或明确接受
      P33 裁剪窗口等价边界。
- [x] 路线确认结果：最终输出一致优先，接受 P33 裁剪窗口等价边界；
      后续若出现输出不一致样本，再重新复核 GDAL 语义。
- [x] 路线确认后，重新运行约 100MB subset 的 P29 固定流程，采集当前
      热点 profile，并确认完整输出 payload 一致。
- [x] 根据 P37 热点结论，移除 native GeoTIFF block cache 成功路径上的
      外层 f64 block cache。
- [x] 运行完整测试、重建 release，并复跑约 100MB subset 的 P29 timeout
      基准与输出差分；42/42 payload 一致，但 timeout 目标仍不满足。
- [x] 对 P37 完整运行后期单独采样：8 秒处 LZW 仍约占 42%，平均采样路径
      约占 52%，坐标变换与 gzip 输出合计不足 1%。
- [x] 按 P37 技术方案优化 `sample_average_pixel` 的重复转换、权重分支和
      内层 bounds check，保持浮点计算顺序不变。
- [x] 为 native GeoTIFF block cache 命中路径增加项目侧 raw bytes 到 f64
      专用转换，保持支持的 `RasterDataType` 数值语义不变。
- [x] 复跑完整测试、release 构建和 42/42 payload 差分；若 timeout 仍不满足，
      再评估是否提出依赖变更授权请求。

## P38：oxiarc-lzw 解码性能源码分析（实施完成）

- [x] 复核 `Cargo.lock` 实际使用的 `oxiarc-lzw 0.4.0`，并读取 decoder、
      dictionary、bitstream 与 oxigeo 调用链源码。
- [x] 新增 `OXIARC_LZW_PERFORMANCE_ANALYSIS.md`，从源码结构、对象生命周期
      和现有 profile 证据解释低效原因。
- [x] 复核文档未记录私有基准元数据，运行 `git diff --check`。
- [x] stage 并提交 P38 文档。

## P39：oxiarc-lzw 自身性能问题说明（实施完成）

- [x] 复核 `oxiarc-lzw 0.4.0` 的公共 API、decoder、dictionary 与 bitstream
      源码行为。
- [x] 新增 `OXIARC_LZW_PERFORMANCE_ISSUES.md`，只说明 crate 自身性能表现和
      可能成因。
- [x] 复核文档不包含外部工程上下文、解决方法或修改建议，运行
      `git diff --check`。
- [x] stage 并提交 P39 文档。

## P40：移除 OxiGeo 依赖树

- [x] 通过 Cargo CLI 添加 `geotiff-reader@0.8.1`、`geotiff-writer@0.8.1`、
      `quick-xml@0.41.0`，移除 `oxigeo@0.2.3` 与 `oxigeo-geotiff@0.2.3`。
- [x] 将 GeoTIFF reader 迁回 `geotiff-reader`，保留 metadata、NoData、
      overview、BigTIFF/压缩读取和共享 decoded-block cache。
- [x] 将 GeoTIFF writer 与 ctb-export 迁回 `geotiff-writer`，保持样本类型、
      NoData、GeoTransform、BigTIFF、Predictor、tile/strip 和压缩语义。
- [x] 用 `quick-xml` 实现项目内标准 VRT 兼容层，保持现有 GeoTIFF source 的
      VRT 输入路径；复杂 VRT 特性不能静默输出错误像素。
- [x] 迁移单元/CLI fixture 辅助函数与格式拒绝测试，更新 README 和 CLI help。
- [x] 运行 fmt/test/clippy/release、公开 oracle、P29 私有 subset 输出差分。
- [x] 运行 `cargo tree --all-features` 并确认没有任何 `oxigeo*` / `oxiarc*`
      依赖；回写实施记录。

## P41：500MB 级私有 subset 对比测试

- [x] 登记 P41，沿用 P29 固定流程和私有数据隐私约束。
- [x] 定位或生成约 500MB subset，测试产物只放在 `/private/tmp`。
- [x] 先运行 C++ 0.4.1，再按两倍墙钟运行 Rust。
- [x] 比较完整输出路径集合和全部 `.terrain` 解压后 payload。
- [x] 回写可公开的聚合计时与差分结论。

## P42：1GB 级私有 subset 对比测试

- [x] 登记 P42，沿用 P29/P41 固定流程和私有数据隐私约束。
- [x] 定位或生成约 1GB subset，测试产物只放在 `/private/tmp`。
- [x] 先运行 C++ 0.4.1，再按两倍墙钟运行 Rust。
- [x] 比较完整输出路径集合和全部 `.terrain` 解压后 payload。
- [x] 回写可公开的聚合计时与差分结论。

## P43：1GB LZW 热点复核与 GeoTIFF 采样转换优化

- [x] 复核 1GB LZW 输入的当前 release 热点与并行行为。
- [x] 复核 `tiff-reader` decoded bytes API 与样本类型/字节序语义。
- [x] 将 GeoTIFF 窗口读取改为 decoded bytes 直接转 `f64`，移除 typed
      `ArrayD<T>` 中间缓冲。
- [x] 运行 fmt/test/clippy/release 门禁。
- [x] 复测约 1GB subset，比较完整路径集合与全部解压 payload。
- [x] 回写热点结论、优化结果与剩余瓶颈。

## P44：外部 C++ ctb-tile 切片耗时 CI（已与 P45 合并为同一 workflow，实施完成前需实机确认）

- [x] 登记 P44，说明新增 CI 只测量 `ahuarte47/cesium-terrain-builder` 的
      `ctb-tile` 对 `demo/guangxi_8_cities.tif` 切片所需墙钟，不涉及本项目
      Rust 代码、测试策略或技术实现。
- [x] 在 `.github/workflows/ctb-benchmark.yml` 注册 `ctb-cpp-slice-timing` job，
      触发事件为 `push` / `pull_request`；使用 Docker Hub 预编译镜像
      `homme/cesium-terrain-builder:0.4.1`（内含同版本 0.4.1 的 `ctb-tile`，
      对应 `ahuarte47/cesium-terrain-builder`，避免在 runner 上源码构建）。
- [x] job 在 `actions/checkout@v7` 开启 `lfs: true` 拉取 LFS 管理的 demo
      tif，通过 `docker run` 运行 `ctb-tile` 后把 `elapsed_seconds`、
      `terrain_tiles` 写入 step summary；`ctb-tile` 非零退出则 CI 失败。
- [x] 实机首跑教训：容器内看不到宿主机 `/tmp` 下的输出目录，已把输出目录
      改到被挂载的 workspace 下（修复 `The output directory does not exist`）。
- [x] 实机合并 run 教训：脚本用 `gzip -dc` 比较 payload，runner 没有
      `gzip`（`command not found: gzip`，exit 127），已补装 `gzip`/`coreutils`。
- [x] 采用 fail-fast：workflow 有共享 `checkout` job，两个基准 job 均
      `needs: checkout`；并设置 concurrency 取消同一 ref 上仍在跑的旧 run。
- [x] 本地校验 workflow YAML 可解析、`git diff --check` 无空白错误。
- [ ] 推送到 GitHub 后确认实机 CI 能成功拉取 LFS、拉取并运行预编译
      `ctb-tile` 镜像并输出切片耗时；在此之前不将 P44 标记为实施完成。

## P45：ctb-rs 可复现切片耗时 CI（已与 P44 合并为同一 workflow，实施完成前需实机确认）

- [x] 登记 P45，说明新增 CI 运行 `scripts/benchmark-ctb-tile.sh` 测量本项目
      release `ctb-tile` 对合成 DEM 的切片墙钟，并校验单线程/多线程输出
      `.terrain` payload 一致；不进入 Rust 测试矩阵，也不设性能门禁。
- [x] 在 `.github/workflows/ctb-benchmark.yml` 注册 `ctb-rs-slice-timing` job，
      触发事件为 `push` / `pull_request`；job 安装 `gdal-bin`、`gzip`、
      `coreutils`，`cargo build --release --locked --bin ctb-tile`，再以
      `CTB_RS_BIN=target/release/ctb-tile` 运行
      `scripts/benchmark-ctb-tile.sh 512 2`。
- [x] job 解析脚本输出的 `single/parallel workers=… seconds=… tiles=…`，
      连同 commit、二进制路径、脚本命令行写入 GitHub step summary；脚本任意
      一步失败则 job 失败。
- [x] 实机首跑教训：runner 默认无 `zsh`；按用户要求不安装 `zsh`，已把
      `scripts/benchmark-ctb-tile.zsh` 用 POSIX sh 改写为
      `scripts/benchmark-ctb-tile.sh`。
- [x] 本地校验 workflow YAML 可解析、`git diff --check` 无空白错误。
- [ ] 推送到 GitHub 后确认实机 CI 能安装 GDAL、构建 release 并跑完
      benchmark、输出 single/parallel 切片耗时；在此之前不将 P45 标记为
      实施完成。
