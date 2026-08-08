# ctb-rs 测试策略

## 1. 原则

测试以 C++ CTB 为基准程序，而不是以当前 Rust 行为为基准程序。每个兼容用例都记录：C++ 提交、
GDAL 版本、命令、输入 checksum、输出路径、解压 terrain payload 或解码 raster、以及比较
结论。压缩容器的时间戳等非语义字段不参与比较。

Rust 常规测试必须不要求 GDAL/PROJ。C++ 基准程序允许位于开发或 CI 的隔离环境，生成后的
小型受许可 fixture、manifest 与 checksum 必须进入仓库。

## 2. 分层

| 层级 | 目标 | 关键断言 |
| --- | --- | --- |
| 领域单元 | C++ `Bounds`/`Grid`/tile/iterator 的公式与边界 | root 层、极值、右上边界、TMS y、zoom、child mask、遍历顺序。 |
| 纯 Rust 适配器 | GeoTIFF/CRS/overview/采样取代 GDAL 的局部行为 | tag、波段、样本类型、NoData、window、destination 初始值、核函数和整数转换。 |
| 文件格式 | Terrain、GTiff 及后续 driver 的编解码 | 未压缩 payload、signed/unsigned bit pattern、GeoTransform、CRS、NoData、压缩。 |
| CLI 进程 | 四个 C++ 工具的契约 | help、参数解析、默认值、stdout/stderr、退出状态、目录布局、quiet/verbose/resume。 |
| C++ 差分 | 最终可观察结果 | tile 路径、payload、样本矩阵、metadata、child flags；必要时容器字节。 |
| 鲁棒性 | Rust 替代层的错误处理 | 损坏 TIFF、缺失 SRS、旋转 transform、NoData、越界、极端尺寸、并发和原子写入。 |

## 3. Fixture 与基准程序清单

权威清单位于 `tests/fixtures/MANIFEST.md`。新增 fixture 必须补充来源/许可证、生成命令、
checksum、元数据和预期。最低矩阵如下：

| 情况 | Terrain | RasterTiler/GTiff | CRS/overview |
| --- | --- | --- | --- |
| EPSG:4326 Int16、完整 tile | payload、child flags | type/values/transform | direct source |
| Float32、正负/小数值 | `uint16` 截断 | GDAL round/clamp | direct source |
| 仅部分覆盖/右上边界 | destination 0、range、child flags | tile 集 | bounds 包含规则 |
| NoData、缺 SRS、损坏文件 | C++ 对应失败 | C++ 对应失败 | 失败类别与文本 |
| tiled/striped、DEFLATE/LZW、BigTIFF | payload | read/write | I/O 支持 |
| 内部/外部 overview | selected level/payload | selected level/raster | GDAL 选择基准程序 |
| EPSG:3857 direct source | profile 行为 | z0/z1 metadata/values | Mercator Grid |
| 4326↔3857 | 重投影 payload | 重投影 raster | 控制点与切片基准程序 |

## 4. 比较方法

- Terrain：解 gzip 后逐字节比较 compact/detailed payload；单独解码 heights、child bitfield 和
  water mask 以定位差异。
- Raster：使用独立纯 Rust reader 比较宽高、band、storage type、NoData、CRS、transform 和
  row-major samples；浮点比较仅在 C++ 输出本身不稳定时使用已记录 epsilon。
- CLI：捕获 stdout、stderr 和 exit status；路径用临时目录归一化后比较。进度输出可按 C++
  的并发不确定性比较格式与数量，不强制线程 id。
- 坐标：固定检查 C++ 控制点；不能用“视觉上正确”取代数值对照。

## 5. 每个变更的验证流程

1. 先新增或更新 C++ 基准程序，并令其在未改 Rust 前失败；
2. 新增领域/适配器测试锁定最小边界；
3. 实现后运行 `cargo fmt --check`、`cargo test` 和 `cargo clippy -- -D warnings`；
4. 运行受影响的 CLI 差分与多线程一致性测试；
5. 在 `TECHNICAL_PLAN.md` 与 `TODO.md` 记录证据、遗留差异和下一任务。

## 6. P1 RasterTiler 通用 Grid 追加策略

## 6a. C++ oracle 恢复后的执行顺序

使用 `/Users/sander/coding/cesium-terrain-builder/build-gdal-v3.11.4/tools` 下由
`build-with-gdal.sh` 生成的 binary，不修改该工程的源文件或工作树。当前构建证据为 C++
commit `d9c29b2e3f9fb9d9d639a1bdd81cc3f42685fa1f`、GDAL `3.11.4`；macOS 运行时需把
`.deps/gdal-install-v3.11.4/lib` 加入动态库搜索路径。执行前记录 binary 的 `--help`/版本信息；
随后固定同一 source fixture，依次比较 geodetic/mercator 的路径与右上边界、Terrain payload、
RasterTiler 的 12 个 resampling、NoData/整数/浮点/越界 destination、overview、EPSG:4326↔3857、
`-z/-m` 及 GTiff tags/layout/compression，最后比较 info/export/extents 的 stdout/stderr/exit
status。首次差异先登记失败证据，再增加最小 Rust 回归测试并修复；通过后才关闭对应 TODO。

`RasterTileset` 接入 `TileGrid` 时，`tests/cli.rs` 已以 EPSG:3857 全世界 direct-source fixture
覆盖 z0：断言 `{z}/{x}/{y}.tif` 路径、输出 CRS、affine transform 及恒定样本；并以 EPSG:4326
输入 + Mercator profile 断言写入前失败、没有 `.tif`。z1 的 C++ 差分 fixture 仍待补充。输出
affine transform 必须等于 `GlobalMercatorGrid::tile_bounds` 与 `resolution`，像素值必须按既有
RasterTiler footprint 采样。保留 EPSG:4326 Geodetic 现有 tests 作为无回归门禁。


## 6b. Oracle 覆盖状态（P0 记录 14/15 后）

| Oracle | 命令 | 结果 |
|--------|------|------|
| Terrain geodetic | `scripts/verify-ctb-oracle.zsh` (5 source x 12 method x 2 range) | 120/120 |
| Terrain + Mercator | `/tmp/ctb-oracle-terrain-mercator.py` (10 method x 5 tile, decompressed compare) | 50/50 |
| GTiff 16x16 | `/tmp/ctb-oracle-16x16.py` (4 type x 12 method x 3 zoom) | 144/144 |
| GTiff Mercator | `/tmp/ctb-oracle-mercator.py` (same-CRS 90 + cross-CRS 50) | 90/90 |
| GTiff creation options | `/tmp/ctb-oracle-gtiff-options.py` (NONE/DEFLATE/LZW + PREDICTOR + TILED) | 132/132 |
| ctb-info | stdout 逐行比较 | 完全一致 |
| ctb-extents | GeoJSON 逐字节比较 | 完全一致 |
| ctb-export | ENVI raw 像素数据比较 | 完全一致 |
| 四 CLI --version | Rust stdout | Rust 0.0.1；C++ oracle 仍为 0.4.1 |
| 总计 | | **874/874** |

Terrain 比较使用解压后 payload（gzip 压缩字节因 flate2/zlib 差异不同，但解压内容一致）。
Terrain child mask 通过 `terrain_child_mask` 以 source bounds 与 tile 四分之一象限的 strict overlaps 判定，
精确复刻 C++ `TerrainTiler::createTile`。

## 7. P2 离散统计核追加策略

使用至少 2×3 的非平坦 source window，覆盖偶数/奇数样本数量、重复值和并列 mode；断言
`mode` 采用 row-major 首次出现的并列值，`med/q1/q3` 采用 nearest-rank（分别为 0.5、
0.25、0.75），并覆盖完全在 source 外的 footprint 返回 `0.0`。该测试先锁定 Rust 的
窗口与排序行为，随后用同一 fixture 的 C++ `-r mode/med/q1/q3` 输出补充差分证据。
当前 Rust 单元覆盖 2×2 非平坦窗口和完全越界窗口；C++ 差分尚未建立。

连续核测试使用至少 6×6 非平坦 source，并分别检查中心点、四边角边界点和非整数坐标；
断言 cubic/cubicspline 的 4×4 tap、lanczos 的 6×6 tap、越界 tap 跳过及权重归一化。核
系数以仓库内 GDAL `gdalresamplingkernels.h` 为实现依据，之后以 C++ `ctb-tile -r` 输出
补充差分记录。
当前 Rust 单元覆盖中心与边缘路径，`cargo test` 通过 72 项；缩放因子、NoData/density 和
C++ 输出差分尚未建立。

oracle 脚本的算法循环必须覆盖 `nearest bilinear cubic cubicspline lanczos average mode
max min med q1 q3`；若 `CTB_ORACLE_BIN` 未提供，脚本应明确退出而不能被解释为差分通过。

CLI golden tests 对 `ctb-info -e` 逐字节比较 65×65 ASCII 输出；对 `ctb-extents` 分别检查
EPSG:4326 geodetic 与 EPSG:3857 mercator 的 z0/z1 文件、GeoJSON polygon 顺序、tile 属性
和科学计数法。错误路径继续比较 exit status 与 stderr。
当前 Rust 证据为 `tests/cli.rs` 的 info 输出断言和 extents geodetic/unsupported-mercator
路径测试；Mercator direct-source 及 C++ 逐字节输出差分仍待补。

投影测试固定 EPSG:4326 控制点（0°、±180°、有效纬度边界）与 EPSG:3857 的 origin shift，
并检查 4326 source→3857 target 的 RasterTiler z0/z1 metadata、样本和 source 覆盖外的
destination 初始值；反向路径使用同样控制点。所有转换先做纯 Rust 数值测试，再接入 C++
差分。
当前 Rust 证据包括 `raster.rs` 的双向控制点、`tests/cli.rs` 的 4326→3857 GTiff 输出以及
Mercator extents 和 Terrain z0 输出；C++ 输出、反向 RasterTiler tile、Terrain payload、
overview/NoData 仍待补。

CRS 边界测试还必须覆盖 EPSG:4326 输入 ±90°、±85.0511287798066° 和超出有效范围的
纬度，断言正向结果落在 Global Mercator grid bounds 内且反向控制点保持一致；之后与 GDAL
坐标变换输出做数值差分。

Rust 已覆盖 ±90° 到有效边界的裁剪，当前全套测试 78 项通过；GDAL 数值差分仍待补。

RasterTiler resampling 测试必须以非平坦窗口调用 `sample_with_footprint_raster_tiler`，覆盖
12 个 CLI 算法名称；连续核断言有限输出，离散统计断言与对应 footprint helper 相同，并
保留 C++ GDAL 输出作为后续数值 oracle。

Rust 当前已覆盖 12 个名称的 RasterTiler 分支，全套测试 79 项通过；C++ 数值差分仍待补。

NoData fixture 必须包含单个无效 tap、边缘混合窗口和全 NoData footprint；断言 reader 不再
整窗失败，内部无效值为 NaN，有效采样按权重/统计过滤，全无效结果为 0.0，并检查 Terrain
最终按 CTB `((height + 1000) * 5)` 编码。随后与 GDAL warp 的 density 输出做差分。

Rust 当前已覆盖混合/全 NoData window 及 12 个 RasterTiler 分支，全套测试 80 项通过；
GDAL density 差分仍待补。

GTiff creation-option 测试至少覆盖 `COMPRESS=NONE/DEFLATE/LZW/ZSTD` 的写出与 Rust reader 读回，
并断言未知选项在创建任何 tile 前失败；压缩编码的字节级差异保留给 C++ oracle。
当前四种压缩均有 CLI 写出/读回证据；C++ 字节差分尚未执行。

压缩矩阵覆盖 `COMPRESS=ZSTD/JPEG/LERC`：CLI 写出后由 Rust reader 读回样本、CRS、transform 和
NoData tag；C++ oracle 恢复后再比较压缩字节与 driver metadata。

JPEG 测试使用 8-bit source 并断言非 8-bit source 在 tile 写出前失败；LERC 使用 Float32/64
source 验证无额外量化参数的读回，LERC 参数选项仍作为未实现/错误路径覆盖。

Rust 当前已覆盖上述 JPEG/LERC 成功与 JPEG 样本类型错误路径，全套测试 83 项通过；质量、
LERC 参数和 C++ driver 差分仍待补。

Creation-option 测试还需覆盖 `BIGTIFF=NO/YES/IF_NEEDED` 的 TIFF header/reader 读取，以及
整数样本 `PREDICTOR=2`、浮点样本 `PREDICTOR=3`；不相容 predictor、重复冲突选项和未知
选项必须在写出任何 tile 前失败。

Rust 当前已覆盖上述 BigTIFF 变体解析、YES header、浮点 Predictor=3 成功和 Predictor=2
失败路径；全套测试 82 项通过，整数 Predictor=2 与 C++ driver 差分仍待补。

Tiled layout 测试覆盖默认 256×256、显式 block 尺寸、strip 默认/`TILED=NO`，以及非正数或
非 16 倍 block 尺寸在写出前失败；用 TIFF tags/layout metadata 与 C++ GTiff CreateCopy 做
差分，不能只断言文件可打开。

Rust 已覆盖 TILED 默认 block、显式 32×16 block 和 block 尺寸错误路径，全套测试 82 项
通过；TIFF layout tag 与 C++ 差分仍待补。

Overview fixture 使用 `geotiff-writer` 的多级 top-level COG：断言 overview 数量、ratio 在
1/2/4 附近的选择、overview metadata 的像元尺寸和 `read_sampling_window` 的实际样本；
再以 C++ `GDALSuggestedWarpOutput2`/`getOverviewDataset` 输出复核 tie 与边界规则。

CLI 默认值测试分别覆盖 Terrain 65、GTiff 256、geodetic extents 65、mercator extents 256；
Terrain 携带 creation option 必须在输出目录写入前失败。

CLI 版本测试固定 C++ oracle 的四个工具 `--version` stdout 为 `0.4.1`；Rust 四个 CLI
`--version` 固定输出当前 Cargo package 版本（当前为 `0.0.1`），不再跟随 C++ oracle
版本号。帮助测试比较选项集合、默认值、参数顺序和退出状态，路径前缀与 clap 自动换行
属于需单独归一化的展示差异。

当前 oracle 执行证据：C++ `ctb-tile` 与 Rust Terrain payload 对 plain、float-negative、
tiled-overview、high-resolution（无 overview）四类输入的 12 算法和 automatic/limited
范围均逐字节通过；high-resolution-overview 在 `0/0/0.terrain` 首个失败。该失败保留为
overview source-window 回归，不得用已通过的 direct 矩阵代替。

Mercator 最小差分使用同一 source fixture 改写为 EPSG:3857、z0、Terrain；两边路径集合均为
`0/0/0.terrain`，但 raw byte 4225 起出现 C++ `5500/6000` 对 Rust `6500/7000`。测试需把
该边缘像元映射回 source/world 坐标，并分别断言 source 覆盖内采样、覆盖外 destination 初值
和 child flags，不能只比较整包失败。

RasterTiler plain z0 GTiff/tile-size-16 oracle 结果：5 个连续核和 7 个 footprint 统计核均
逐值通过。原始失败情形是中心位于 source bounds 外而 footprint 擦边，C++ 样本为 destination
初值 0；Rust 已在 RasterTiler 统计入口补门禁，且现有 Terrain source-edge overlap 测试仍通过。

NoData 最小 oracle 使用 2×2 Int32 GeoTIFF、NoData=200；C++/Rust RasterTiler average GTiff
逐值一致，Terrain z0 gzip payload 逐字节一致。后续仍需混合窗口、全 NoData、12 算法和
overview density。

Mercator 边界审计必须保存：source 2×2 的 GeoTransform/CRS/样本矩阵、Terrain expanded
target bounds、目标 row/column 的 world center 与 footprint、C++ `CPL_DEBUG=ON` 的
`GWKAverageOrMode Src=...` window，以及 Rust 对应 source row/column。只有这些中间值一致
后，才可关闭 Mercator/overview TODO；不接受仅凭最终 payload 猜测的舍入修复。

`ctb-tile` 参数测试还必须覆盖 `-z` 默认 `0.125`、`-m` 默认 `0` 的解析、负数/非有限值
拒绝，以及非默认值在任何 tile 写出前显式返回未实现错误。待 C++ oracle 恢复后，再比较
`-z` 对投影结果的影响，并确认 `-m` 是否仅为执行资源提示。

当前 Rust overview fixture 已覆盖 2/4 倍 top-level overview、1.5/2/4 ratio 选择、派生
GeoTransform 和窗口样本；全套测试 77 项通过，C++ tie/boundary 差分仍待补。

Level-aware RasterTiler 测试还需断言 `sample_values` 只选择一次 level，并从 overview IFD
读取，而不是逐像元回退 base IFD；与同一 fixture 的 base/overview 样本和 C++ SuggestedWarp
选择结果进行差分。

已恢复的 C++ oracle 首个 overview 证据为：`high-resolution-overview / nearest / automatic`
的 `0/0/0.terrain` 两个 raw payload 均 8452 字节，但 overview 区域出现 `5500`（C++）对
`6500`（修复前 Rust）的差异；去掉 overview 后 12 算法均通过。回归要求目标/source 像元
分辨率比例参与 level 选择，并重跑完整 5 输入 × 12 算法 × 2 zoom-range 矩阵。

进一步的 C++ `CPL_DEBUG=ON` 证据显示 Terrain warp 使用 `GWKAverageOrMode`，与 C++ CLI
传入的 12 个 `-r` 名称无关。因此 Terrain 差分必须同时断言：12 个命令的 payload 彼此相同，
且等于 C++ 默认 Average；`-r` 差分只在 `-f GTiff` RasterTiler 路径执行。

Rust overview-only source 已验证 RasterTiler 复用选定 level，当前全套测试 83 项通过；
C++ ratio/tie 差分仍待补。

Cargo 命令必须在禁用沙盒的环境执行。生产代码和测试均不得以 `unwrap` 隐藏预期失败；测试中
若使用 `expect`，消息应说明被验证的不变量。
 
## 8. P2 GDAL 核函数精确匹配（根因 D/E/F）
 
16×16 GTiff fixture 的 144 组 RasterTiler 差分中，P0 记录 9 消除了 16 组边缘差异，
剩余 4 组为 1-ULP 整数舍入偏差，涉及三个独立根因：
 
- 根因 D（bilinear 累加序）：bilinear 改用 GDAL GWKBilinearResample4Sample 的预乘角点权重
  直接累加（gdalwarpkernel.cpp:2696-2810），替换可分离横向/纵向插值。测试断言 4 个角点权重
  按 UL*(rx*ry)+UR*((1-rx)*ry)+LL*(rx*(1-ry))+LR*((1-rx)*(1-ry)) 累加序计算。
 
- 根因 E（cubic 权重公式 + 分离卷积）：cubic 改用 GDAL GWKCubicComputeWeights 系数公式
  （gdalwarpkernel.cpp:2946-2956）+ 分离 CONVOL4 结构（先横向 4 行，再纵向），替换非分离 2D
  卷积。测试断言权重系数与 GDAL 多项式求值序一致，且卷积先横向再纵向。
 
- 根因 F（average footprint 来源）：average_at 的 footprint 来源从世界坐标像元边界改为
  source_center±0.5（GDAL padfX±0.5），确保 footprint 始终恰好 1 个 source pixel 宽。

根因 D 已在工作树中实现。根因 E 和 F 尚待实现；实现后须运行 cargo test + cargo clippy
-D warnings，再以 C++ oracle 差分复核 144 组是否全绿。

## 9. P9 任意 EPSG 输入 CRS 重投影（proj4rs）

P9 使用 `proj4rs@0.1.10` 的 `crs-definitions` 功能解析 GeoTIFF 输入 CRS。EPSG:4326 与
EPSG:3857 仍走既有内建公式，避免破坏 P0–P6 的 oracle；其它 EPSG 输入经 proj4rs 转换到
目标 CTB profile。proj4rs 不解析任意 WKT，NTV2 grid shift 仍为实验性，因此任意 WKT 输入
和带本地 grid shift 文件的 CRS 不作为 P9 的接受范围。

测试策略：

- `raster.rs` 单元测试覆盖 `Crs::Epsg(u16)`：
  - EPSG:32630 `(500000, 0)` 与 EPSG:4326 `(-3, 0)` 的控制点互换；
  - EPSG:27700 `(400000, -100000)` 逆变换到 EPSG:4326 后在合理容差内回到原坐标；
  - 未知 EPSG 和无法解析的坐标变换返回 `UnsupportedCrs`。
- `geotiff.rs` 单元测试覆盖任意 EPSG 打开：
  - 使用 `GeoTiffBuilder::epsg(32630)` 生成投影坐标 fixture，打开后
    `metadata().crs == Crs::Epsg(32630)`；
  - 使用未知 EPSG fixture，打开返回 `UnsupportedCrs`。
- CLI 集成测试覆盖投影坐标 GeoTIFF 输入：
  - 写入 EPSG:32630 的 32×32、8 km pixel、约 256 km 局部范围北向上 GeoTIFF，
    固定 z6 后 `ctb-tile -p geodetic` 能生成 terrain 切片；
  - 同一 fixture 在 `ctb-tile -p mercator` 下能生成 GTiff 切片，输出 tile 的
    EPSG 为 3857，且能采样到源值；局部切片避免把大范围目标 tile 反转到 UTM 投影域之外。

完成门禁沿用 P7：`cargo fmt --check`、`cargo test`、`cargo clippy --all-targets -- -D
warnings` 全部通过；既有 4326↔3857 oracle 行为不回归。

## 10. P10 OxiGeo 栅格读写迁移

P10 将 fixture 与测试辅助函数从 `geotiff-reader` / `geotiff-writer` 迁移到
OxiGeo 0.2.3，并保持现有 GeoTIFF 行为基线。测试必须只断言 OxiGeo 0.2.3
实际支持的读取范围：GeoTIFF 与 VRT。输出仍只验证 GeoTIFF。

测试策略：

- 保留现有 GeoTIFF open/metadata/value/overview/BigTIFF/Predictor/tile/strip
  断言；fixture 生成与读回改为 OxiGeo。
- 新增 VRT fixture：写入一个可被 OxiGeo 生成或人工构造的 `.vrt`，引用仓库内
  小 GeoTIFF；断言 VRT 能打开、metadata 正确、数值窗口读取与源 GeoTIFF 一致。
- 新增不支持格式拒绝测试：`.nc`、`.jp2` 或 `.h5` 输入在写出任何 tile 前返回
  `UnsupportedRaster`，输出目录不含 tile 文件。
- `COMPRESS=JPEG` 与 `COMPRESS=LERC` 的 CLI 测试从“成功写出”改为“在写出任何
  tile 前失败”，并断言 `0/0/0.tif` 不存在。
- BigTIFF 测试保留 header 断言：`BIGTIFF=NO` 为 `II*\0` / `MM\0*`，
  `BIGTIFF=YES` 为 `II+\0` / `MM\0+`；`IF_NEEDED` 按文件大小自动选择。
- `overview_count()` 在 VRT 输入上为 0；GeoTIFF overview 的
  `sampling_level_for_ratio` 保持 `level: 0` 加 overview metadata 的 C++ 行为。
- `CachedRasterSource::new_with_nodata_cache` 单元测试验证声明 NoData 的源仍只
  读取一次相邻 block，默认 `new` 保留原有逐窗口读取行为。
- 所有维度从 OxiGeo `u64` 转入现有 `u32` 接口的转换测试覆盖合法边界与溢出拒绝；
  测试代码如使用 `expect`，消息必须说明被验证的不变量。

门禁：`cargo fmt --check`、`cargo test --all-targets`、
`cargo clippy --all-targets -- -D warnings`、
`scripts/verify-ctb-oracle.zsh` 通过；`cargo tree` 无
`geotiff-reader` / `geotiff-writer`。

## 11. P11 GitHub Actions Node.js 运行时升级

GitHub Actions 已弃用 Node.js 20 action 运行时，`actions/checkout@v4` 与
`actions/upload-artifact@v4` 会在 runner 上输出 deprecation warning。升级到
Node.js 24 主版本 `actions/checkout@v5`、`actions/upload-artifact@v5` 后，
验证范围限定为 workflow 配置：

- 确认 `.github/workflows/ci.yml` 可被 YAML 解析。
- 检查 `actions/checkout@v4`、`actions/upload-artifact@v4` 不再出现。
- 保持 CI 的触发事件、矩阵、构建命令与 artifact 上传行为不变。
- 不运行 Rust 测试，也不执行 C++ oracle；本变更不涉及采样或栅格行为。

## 12. P12 全部 GitHub Actions 升级到当前最新主版本

`actions/checkout` 与 `actions/upload-artifact` 升级到当前最新主版本 `v7`；
`dtolnay/rust-toolchain@stable` 按官方 README 保留为最新 stable Rust 引用。
验证范围限定为 workflow 配置：

- 确认 `.github/workflows/ci.yml` 可被 YAML 解析。
- 检查 `actions/checkout` 与 `actions/upload-artifact` 均引用 `v7`。
- 核对 v7 action 的输入定义与当前 `name`、`path`、`if-no-files-found` 用法兼容。
- 保持 CI 的触发事件、矩阵、构建命令与 artifact 上传行为不变。
- 不运行 Rust 测试，也不执行 C++ oracle；本变更不涉及采样或栅格行为。

## 13. P13 真实 Copernicus DEM 差分审计

真实输入为 Copernicus DSM COG：
`tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`（Git LFS；原始路径
`/Users/sander/coding/demo/download-data/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`）。
其元数据为 EPSG:4326、3600×3600、Float32、DEFLATE、PREDICTOR=3、三级 overview。

对比策略：

- C++ oracle 使用 `build-gdal-v3.11.4/tools` 下的 `ctb-tile` 与 `ctb-extents`，
  macOS 运行前设置 `DYLD_LIBRARY_PATH` 指向同 build 目录。
- Rust 使用当前源码重新构建后的 release 二进制，避免用旧产物作结论。
- Terrain 只比较 gzip 解压后的 payload；压缩容器字节差异不作为行为差异。
- `ctb-extents` 比较每个 `{zoom}.geojson` 的路径集合与 GeoJSON 文本。
- 真实数据可能产生大量高 zoom tile；若默认范围过大，先记录 `ctb-extents`
  给出的 zoom 范围，再对代表性 zoom 做 payload 差分，不能静默跳过。

### 13.1 实测结果（2026-08-06）

输入 MD5：`6de035f523ed325945108641b4056415`。C++ oracle `0.4.1`，Rust
`0.0.1`。`ctb-extents` 默认范围生成的 15 个 GeoJSON 文件逐字节一致；全范围
`ctb-tile -q -c 4 -s 14 -e 0` 的 11,391 个 Terrain 路径完全一致。

解 gzip 后 payload 比较：

| zoom | files | payload same | payload diff | height samples diff | max u16 diff | max meters |
|---:|---:|---:|---:|---:|---:|---:|
| 0 | 1 | 0 | 1 | 12 | 1468 | 293.6 |
| 1 | 1 | 0 | 1 | 25 | 1919 | 383.8 |
| 2 | 1 | 0 | 1 | 17 | 1557 | 311.4 |
| 3 | 2 | 0 | 2 | 54 | 1360 | 272.0 |
| 4 | 2 | 0 | 2 | 256 | 1562 | 312.4 |
| 5 | 2 | 0 | 2 | 810 | 1688 | 337.6 |
| 6 | 2 | 0 | 2 | 1112 | 857 | 171.4 |
| 7 | 4 | 0 | 4 | 2641 | 5 | 1.0 |
| 8 | 6 | 0 | 6 | 9169 | 11 | 2.2 |
| 9 | 16 | 0 | 16 | 30230 | 736 | 147.2 |
| 10 | 42 | 0 | 42 | 89221 | 18 | 3.6 |
| 11 | 156 | 0 | 156 | 323785 | 21 | 4.2 |
| 12 | 576 | 0 | 576 | 952836 | 41 | 8.2 |
| 13 | 2116 | 0 | 2116 | 2543559 | 84 | 16.8 |
| 14 | 8464 | 32 | 8432 | 7209362 | 605 | 121.0 |
| total | 11391 | 32 | 11359 | 11163089 | 1919 | 383.8 |

结论：

- `ctb-extents`、Terrain 路径集合、child flag 和 water mask byte 均一致。
- 高度样本存在广泛差异：99.7% 的 Terrain 文件不同，约 23.2% 的 65×65
  高度样本不同，最大 383.8 m。
- 性能差距明显：z14 单独运行 Rust 约 3.5x 慢，全范围 z14->z0 约 40x 慢。
- 当前合成 fixture 的 overview oracle 不能覆盖真实 COG 行为；P13 的真实
  COG source-window oracle 已建立（见 13.2），overview warp 混合根因已定位，
  Rust 的数值实现差异仍打开。

### 13.2 真实 COG source-window oracle（2026-08-06）

使用 `/private/tmp/ctb-p13-compare/ctb-p13-oracle.cpp` 直接调用
`ctb::GlobalGeodetic(65)` 与 `ctb::TerrainTiler`，暴露
`createRasterTile`/`terrainTileBounds`，读取实际创建的 65×65 VRT 并写出
`float` raw 与 CTB u16 编码。oracle 与 C++ `ctb-tile` 解压 payload 在四个
选定坐标上完全一致，因此其 captured overview 与 warp window 可作为真实 COG
行为证据。

四个坐标的 GDAL 选择与 warp window：

| coord | suggested output | selected overview | overview GT | GDAL warp `Src=` |
|---|---:|---:|---|---:|
| z0 tx=1 ty=0 | 3600x3600 | 2 | `108,0.00222222,0,23.0001,0,-0.00222222` | `0,0,3600x3600` |
| z1 tx=3 ty=1 | 3600x3600 | 2 | `108,0.00222222,0,23.0001,0,-0.00222222` | `0,0,3600x3600` |
| z9 tx=819 ty=318 | 3600x3600 | 2 | `108,0.00222222,0,23.0001,0,-0.00222222` | `0,380,127x162` |
| z14 tx=26214 ty=10194 | 3600x3600 | 2 | `108,0.00222222,0,23.0001,0,-0.00222222` | `0,447,4x6` |

oracle 与 C++/Rust 的 u16 样本差分：

| coord | oracle vs C++ | oracle vs Rust | max u16 diff | index | oracle u16 | Rust u16 |
|---|---:|---:|---:|---:|---:|---:|
| z0 tx=1 ty=0 | 0 | 12 | 1468 | 1794 | 5000 | 6468 |
| z1 tx=3 ty=1 | 0 | 25 | 1919 | 3524 | 5000 | 6919 |
| z9 tx=819 ty=318 | 0 | 785 | 3 | 3249 | 5527 | 5524 |
| z14 tx=26214 ty=10194 | 0 | 454 | 447 | 4185 | 5447 | 5000 |

根因已定位到 CTB/GDAL 的 overview warp 混合行为：transformer 使用 overview
坐标，但 `psWarpOptions->hSrcDS` 保持主数据集，GDAL 据此夹取并读取 base
窗口。Rust 当前 `sampling_level_for_ratio` 已经携带该意图（`level: 0` +
overview metadata），但数值仍未对齐，后续必须逐 destination 像元对照 GDAL
source-window/权重。

门禁：真实 COG oracle 属于开发环境审计，不在无 GDAL 的 CI 中运行。修复 Rust
后，四个坐标的 `oracle vs Rust` 必须为 0；合成 high-resolution-overview
fixture 通过不能替代真实 COG oracle。

### 13.3 根因确认（2026-08-06）

GDAL warp 的数值路径已确认，并用 `/private/tmp/ctb-p13-diag` 在四个 oracle
坐标上逐字节复现。C++ 使用 overlap GT 作为 warp destination transform，
用 overview GT 做坐标数学，但 `psWarpOptions->hSrcDS` 保持 base 数据集，
因此读取 base 窗口。`GRA_Average` 的 margin 来自 GDAL
`GWKAverageOrModeThread`/`PerformWarp` 的 transform scale：
`dfXScale = overview_pixel_width / overlap_pixel_width`，
`margin = 2 * max(1, ceil(1 / dfXScale))`。本输入 overview 像素宽 `1/450`，
z3/z4/z5/z6/z9 overlap 像素宽分别为
`0.3515625/0.17578125/0.087890625/0.0439453125/0.0054931640625`，
对应 margin `318/160/80/40/6`。不能按 base 数据宽度与 heightmap 尺寸推导，
也不能写死 112。

### 13.4 P14 Terrain GRA_Average warp 对齐测试

生产代码改动前先更新技术方案，再按以下策略测试：

- 合成源上验证 overlap GT：`TerrainSamplePlan` 的 cell 尺寸等于
  `tile_bounds / (grid_tile_size - 1)`，overlap GT origin 为
  `(min_x - cell_width, max_y + cell_height)`。
- 合成源上验证 pooled `ComputeSourceWindow`：边界 21 点、1e-6 取整、
  base 尺寸夹取、跨度 >0.9 base 时整幅读取。
- 合成源上验证 per-pixel margin gate 与 average 权重：被 margin 拒绝的
  像元返回 0.0；正常像元按 GDAL 边界权重和 weighted incremental average
  得到期望值。
- 合成源上验证 margin 公式：`dfXScale = nDstXSize / nSrcXSize`、
  `dfYScale = nDstYSize / nSrcYSize`，`margin =
  2 * max(1, ceil(1 / dfScale))`；用真实 COG 已知 pooled window 验证
  z0/z1/z2=112、z3/z4/z5=64x8、z6=24x8、z9 row 321=4x2、z14=2x2。
  注意 `nSrcSize <= nDstSize`（1:1 或上采样）时 margin 恒为 2。
- 合成 65×65 全世界源验证空 pooled source window：source 上右边界恰好在
  `(180, 90)` 时，C++/Rust 的 tile 计划都会包含 `y=1`/`x=2` 的越界 tile；
  其中 `ComputeSourceWindow` 返回宽度或高度为 0 时，Rust 必须按 GDAL
  `WarpRegion` 的跳过语义输出全 0 高度，且不能发起 0 尺寸
  `read_sampling_window` 请求。
- 保持 `SamplingLevel { level: 0, overview metadata, base data size }`
  语义不变；真实 COG 回归中确认仍从 base IFD 读取 overview 坐标窗口。
- 开发环境门禁：用 `/private/tmp/ctb-p13-compare` 的 oracle raw 对比
  Rust 输出，四个坐标必须 `diff_count=0`。

#### 13.4 实施记录

2026-08-07 P14 geodetic Average 路径已实现并验证：

- `cargo fmt`、`cargo test`、`cargo clippy --all-targets -- -D warnings` 全部
  通过；lib 单元 86 项、`ctb-tile` 参数单元 9 项、CLI 12 项、
  `ctb-info` 1 项通过。
- 真实 Copernicus DEM 四个 oracle 坐标
  `z0(1,0)`、`z1(3,1)`、`z9(819,318)`、`z14(26214,10194)` 调用生产
  `TerrainSamplePlan::sample_heights`，float raw 与 oracle 全部
  `diff_count=0`。
- 真实 DEM 全量比较暴露 margin 初版按 transform ratio 推导：11 个 payload
  差异位于 `0/1/0`、`1/3/1`、`2/6/2`、`3/12/5`、`4/25/10`、
  `5/51/20`、`6/102/40` 与 `9/819/321` 至 `9/822/321`。GDAL
  `PerformWarp` 实际按 pooled source window 尺寸推导，修正后需重新跑全量
  差分。
- 修正 `average_margin` 为按 pooled source window 宽/高推导并重建 release
  后，真实 DEM 全量差分收敛：11391/11391 个 Terrain 文件路径一致，解压后
  payload 差异为 0。
- 65×65 全世界合成输入跑生产 `ctb-tile -s 0 -e 0`，六个 `.terrain` 解压后
  与 C++ CTB oracle 逐字节一致，覆盖上边界空 pooled source window。

### 13.5 Mercator Terrain VRT block pooled 测试

2026-08-07 建立 Mercator oracle：

- 输入：`/private/tmp/ctb-mercator-pooled-check/world3857/source.tif`，
  720×720、EPSG:3857、Int32，由 EPSG:4326 世界数据 `gdalwarp` 生成。
- C++：`/private/tmp/ctb-oracle-wrapper.sh` 调
  `/Users/sander/coding/cesium-terrain-builder/build-gdal-v3.11.4/tools/ctb-tile`
  （GDAL 3.11.4），输出 `/private/tmp/ctb-mercator-pooled-check/woc`。
- Rust：`/Users/sander/coding/ctb-rs/target/release/ctb-tile`，输出
  `/private/tmp/ctb-mercator-pooled-check/wrc`。
- 路径清单：`wo.txt` / `wr.txt`，38 个文件一致；block 修正前有 10 个 payload
  差异，详见 TECHNICAL_PLAN P15。

测试断言：

- `TerrainSamplePlan` 的 warp block 尺寸：geodetic 65×65、Mercator
  256×128，由 `min(grid_tile_size, 512)` 与 `min(grid_tile_size, 128)`
  推导。
- `compute_source_window` 对矩形 destination 使用 `nDstXSize/nDstYSize`
  分别采样；Mercator 首 block 在 720×720 源上得到
  `(0,0,720,359)`，与 GDAL debug 一致。
- `average_margin` 对 `(destination=256, source=720)` 与
  `(destination=128, source=359)` 均得到 6，避免继续使用 65 导致错误 margin。
- `sample_average_with_gdal_window` 按 block 计算 window 和 margin，但输出
  仍固定为 65×65，对应 `TerrainTiler` 的
  `RasterIO(0,0,TILE_SIZE,TILE_SIZE)`。
- `GWKAverageOrModeComputeLineCoords` 的近似行坐标：Mercator Average 对
  `warp_block_width`（256）个点调用 `GDALApproxTransform`，Rust 必须复现
  整行递归近似，不能退回逐像素精确变换；`mercator-coord-diag` 在
  `z2/0/0 pos15,46` 上 exact/approx 的 X 坐标分别为
  `9.8823529411764746` / `9.8823529411764497`。
- 开发环境门禁：重跑 38 个 Mercator Terrain，路径与解压后 payload 必须全部
  一致；重跑 Copernicus geodetic 全量差分确认 11391/11391 且 payload 差为 0。

2026-08-07 完成 P15 实现后的最终结果：

- `TerrainSamplePlan` warp block：geodetic 65×65、Mercator 256×128。
- `compute_source_window` 使用矩形 destination 尺寸；Mercator 首 block 仍为
  `Src=0,0,720x359`，X/Y margin 均为 6。
- `GWKAverageOrModeComputeLineCoords` 的整行 `GDALApproxTransform` 已移植，
  并通过 z2/0/0 row46 col49 的 C++ oracle 行坐标测试。
- 重建 release 后 Mercator 38/38 路径一致、解压后 payload 差异为 0。
- 重建 release 后 Copernicus geodetic 11391/11391 路径一致、解压后 payload
  差异为 0，确认 65×65 路径无回归。

## 14. LFS fixture 管理

真实 Copernicus DEM 已归档到仓库内
`tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`，由 Git LFS 管理。它
不是合成 fixture，常规 `cargo test` 不读取；后续 oracle 脚本如需使用该输入，
应使用仓库内路径，并确保 clone/checkout 后已拉取 LFS 对象。

- SHA-256：
  `7670186b097b61e7fd7b6b9310783d0dfec564c2faa167f28560b1e375fc17ca`。
- 元数据：EPSG:4326、3600×3600、Float32、
  `Origin=(107.999861111111116, 23.000138888888888)`、
  `Pixel Size=(0.000277777777778, -0.000277777777778)`、
  DEFLATE、PREDICTOR=3、三级 overview。
- 清单：见 `tests/fixtures/MANIFEST.md` 的 LFS 清单。

## 15. P17 GitHub Actions release 发布

P17 只修改 `.github/workflows/ci.yml`，不涉及 Rust 算法、fixture 或 C++
oracle。验证范围限定为 workflow 配置：

- 确认 `release` job 仅在 `refs/tags/v*` 时执行，且 `needs: build`。
- 确认 `permissions.contents` 为 `write`，发布动作能创建 release。
- 确认 `actions/download-artifact@v8` 使用 `path: dist` 与
  `merge-multiple: true` 下载全部 `ctb-binaries-*`。
- 确认 `softprops/action-gh-release@v3` 的 `files` 为 `dist/*`，并设置
  `fail_on_unmatched_files: true`。
- 本地验证 workflow 可被 YAML 解析、`git diff --check` 无空白错误，并核对
  `actions/download-artifact` 与 `softprops/action-gh-release` 的版本 tag
  存在。
- 不运行 Rust 测试，也不执行 C++ oracle。

已执行：workflow YAML 解析通过，`git diff --check` 通过；
`actions/download-artifact@v8`、`softprops/action-gh-release@v3` 与现有
`actions/upload-artifact@v7` 的对应主版本 tag 均存在。未在 GitHub 实际推送
`v*` tag，本轮只完成配置级验证。
