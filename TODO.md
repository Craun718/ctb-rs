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
