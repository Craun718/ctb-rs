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

## 16. P18 GeoTIFF 原生 block 缓存

P18 不改变采样算法，只改变 GeoTIFF 窗口读取的底层实现：把 OxiGeo 每次
`64×64` window 重复解压真实 TIFF block 的行为，改为按 `(level, tile_x,
tile_y)` 缓存已解码原生字节。

测试断言：

- 对 tiled 输入，缓存几何为 `tile_width × tile_height` 和
  `tiles_across × tiles_down`；对 striped 输入，缓存几何为
  `level_width × RowsPerStrip`，最后一个 strip 使用实际剩余行数。
- 跨真实 block 边界的窗口，经 block 缓存路径读取的 `Vec<f64>` 与直接调用
  `read_window_into_typed::<f64>` 完全一致；Float32/整数/最终边缘 block
  都必须覆盖。
- 显式读取内部 overview level 时，按该 level 自己的 IFD 几何读取，数值与
  直接读取一致。
- 重复读取同一窗口时复用 block 缓存，输出与首次读取一致。
- VRT 输入仍走原 `read_window` + `copy_to_slice` 路径，不引入 GeoTIFF block
  缓存行为。
- 回归门禁：重建 release 后 geodetic 11391/11391、Mercator 38/38 路径一致，
  解压后 payload 差异为 0；真实 Copernicus DEM 低 zoom 性能需记录 Rust 新
  耗时与 C++ 基线。

2026-08-13 实测结果：

- `cargo fmt --check` 通过；`cargo test --lib geotiff` 20/20 通过；
  `cargo clippy --all-targets -- -D warnings` 通过。
- 全量 `cargo test --lib` 为 91 passed、1 failed，失败是既有
  `error.rs::invalid_zoom_range_display_explains_highest_and_lowest`
  文案断言，不属于 P18。
- 重建 release 后真实 Copernicus DEM：Rust z0 4.24 s、z14->z0 8.18 s；
  C++ z0 0.78 s、z14->z0 1.51 s；旧 Rust 基线为 z0 50.97 s、
  z14->z0 91.68 s。
- geodetic Copernicus：路径 11391/11391，解压后 payload diff 0。
- Mercator 720×720 EPSG:3857：`-s 2 -e 0` 路径 38/38，解压后 payload
  diff 0。

## 17. P19 应用层窗口按 block 批量复制

P19 不改变采样算法或 GeoTIFF 原生 block 缓存，只优化
`CachedRasterSource` 的窗口复制路径：从逐像素 `cached_block` 改为按
`block_size` 对齐遍历，缓存样本改用 `Arc<[f64]>`。

测试断言：

- 跨多个应用层 block 的窗口，按 block 批量复制得到的 row-major 样本与
  逐像素读取完全一致。
- 一个请求命中的每个应用层 block 只触发一次底层读取；不同 block 按 LRU
  正常缓存。
- 声明 NoData 的源继续走 exact-read 路径，不因 P19 改变缓存语义。
- `CachedRasterSource` 单测、`cargo test --lib geotiff`、
  `cargo clippy --all-targets -- -D warnings` 通过。
- 回归门禁：重建 release 后 geodetic 11391/11391、Mercator 38/38 路径一致，
  解压后 payload 差异为 0；真实 Copernicus DEM 低 zoom 性能需记录 Rust 新
  耗时与 C++ 基线。

2026-08-13 实测结果（同一机器非隔离墙钟时间，仅用于趋势对比）：

- `cargo fmt --check` 通过；`cargo test --lib cache` 9/9 通过；
  `cargo test --lib geotiff` 20/20 通过；
  `cargo clippy --all-targets -- -D warnings` 通过。
- 全量 `cargo test --lib` 为 92 passed、1 failed，失败是既有
  `error.rs` 文案断言，不属于 P19。
- 重建 release 后真实 Copernicus DEM：Rust z0 约 0.27 s、
  z14->z0 约 1.0 s；C++ 本轮重跑为 z0 0.58 s、z14->z0 1.63 s。
  P18 记录为 Rust z0 4.24 s、z14->z0 8.18 s；C++ z0 0.78 s、
  z14->z0 1.51 s；旧 Rust 基线为 z0 50.97 s、z14->z0 91.68 s。
- geodetic Copernicus：路径 11391/11391，解压后 payload diff 0。
- Mercator 720×720 EPSG:3857：`-s 2 -e 0` 路径 38/38，解压后 payload
  diff 0。

## 18. P20 GitHub release 资产按平台标识

P20 只修改 `.github/workflows/ci.yml` 的上传路径与矩阵元数据，不涉及 Rust
算法、fixture 或 C++ oracle。验证范围限定为 workflow 配置和发布资产命名：

- `build` job 矩阵新增 `platform_suffix`，四个映射必须为
  `windows-x64`、`macos-arm64`、`linux-arm64`、`linux-x64`。
- `cargo build --all-targets --locked` 之后、上传之前，四个二进制被复制为
  `ctb-tile-<platform_suffix><binary_ext>`、
  `ctb-info-<platform_suffix><binary_ext>`、
  `ctb-export-<platform_suffix><binary_ext>`、
  `ctb-extents-<platform_suffix><binary_ext>`；Windows 保留 `.exe`。
- `actions/upload-artifact@v7` 只上传带平台后缀的文件，artifact 名称继续
  使用 `ctb-binaries-*`，`actions/download-artifact@v8` 的
  `merge-multiple: true` 合并后不得再发生同名覆盖。
- 预期 release 资产为 4 个工具 × 4 个平台，共 16 个文件，文件名全局唯一且
  可识别平台。
- 本地验证 workflow 可被 YAML 解析、`git diff --check` 无空白错误；模拟
  四个平台文件合并时无同名文件。
- 不运行 Rust 测试，也不执行 C++ oracle。

已执行：workflow YAML 解析通过，四个 `platform_suffix` 映射正确；
`git diff --check` 通过；模拟 `ctb-{tile,info,export,extents}-{windows-x64.exe,
macos-arm64,linux-arm64,linux-x64}` 得到 16 个唯一资产名，无同名覆盖。
未在 GitHub 实际推送新 tag，本轮只完成配置级验证。

## 19. P21 跨 worker 共享 GeoTIFF block 缓存与写路径优化

P21 不改变采样算法、CRS/坐标变换、输出 tile 路径或 terrain payload，只
优化 worker 之间的重复解码、source open 的重复 IFD 解析，以及 Terrain
写文件路径。

测试断言：

- `GeoTiffBlockCache` 可通过 `Arc` 在多个 `GeoTiffRasterSource` 之间共享；
  两个 source 对同一 `(level, tile_x, tile_y)` 的首个请求只会触发一次
  `read_tile_band_buffer`，后续请求命中缓存，且窗口数值与直接
  `read_window_into_typed::<f64>` 一致。
- 缓存命中/插入的 `Mutex` 只保护 LRU 元数据和字节预算，不覆盖 deflate；
  代码路径必须先在锁外完成 `read_tile_band_buffer`，再抢锁插入。
- 固定 64 MiB 字节预算和 LRU 淘汰继续生效；单个超大 block 仍保留最近使用
  项，超预算时淘汰较旧项。
- VRT 输入继续走原 `read_window` + `copy_to_slice`，不引入共享 GeoTIFF
  block 缓存。
- `write_gzip` 写入磁盘的 gzip 字节与 `encode_gzip` 完全一致，
  `decode_gzip` 往返后 terrain 相等。
- tileset 预创建目录后，每个 tile 仍先写同目录临时文件再 rename；写失败时
  清理临时文件，路径和最终文件内容不因优化改变。
- `cargo fmt --check`、`cargo test --lib geotiff`、
  `cargo test --lib cache`、`cargo test --lib terrain`、
  `cargo test --lib tileset` 与 `cargo clippy --all-targets -- -D warnings`
  通过；全量 lib 测试除既有 `error.rs` 文案断言外全部通过。
- 回归门禁：重建 release 后 geodetic 11391/11391、Mercator 38/38 路径一致，
  解压后 payload 差异为 0；真实 Copernicus DEM z0/z14->z0 性能需记录 Rust
  新耗时与 C++ 基线。

2026-08-13 实施结果：

- 共享缓存单测验证两个 `Arc` clone 访问同一 block 时只触发一次
  `read_tile_band_buffer`，并发命中与直接读取结果一致；VRT 不进入
  GeoTIFF block 缓存。
- `write_gzip` 写入文件后读取字节等于 `encode_gzip`，`read_gzip` 可往返还原
  terrain。
- `cargo fmt --check` 通过；`cargo clippy --all-targets -- -D warnings`
  通过；`cargo test --lib` 为 93 passed、1 failed，失败仍是既有
  `error.rs` 文案断言，不属于 P21；release 构建通过。
- 真实 Copernicus DEM 性能（同一机器非隔离墙钟）：`z0` 约 0.27 s，
  `z14->z0` 约 0.89 s；P19 为 z0 约 0.27 s、z14->z0 约 1.0 s；C++ 基线
  为 z0 0.58/0.78 s、z14->z0 1.37/1.63 s。
- geodetic Copernicus：`tests/terrain` 与
  `/private/tmp/ctb-p21.abZX3C/full` 对比，路径 11391/11391，解压后
  payload diff 0。
- Mercator 真实 C++ oracle 本轮不可复跑：`cesium-terrain-builder`、
  `/private/tmp/ctb-p18-cpp-run/ctb-tile` 和原始 720×720 EPSG:3857 输入
  均已不存在；`oracle-source.asc` 重建只能得到 9 条路径，不能作为 38/38
  回归证据，Mercator 一致性沿用 P18/P19 记录。

## 20. P22 私有数据性能复核

约束：只允许在文档中记录私有数据文件体积 1.9G，以及可复用的优化方向和
优化内容；不记录路径、名称、CRS、尺寸、分辨率、波段、zoom 范围或 tile
数量。测试产物放在 `/private/tmp`，不进入工作树。本轮不进行 C++ oracle
差分。全量范围预计需数小时，本轮以代表性压力 profile 验证热点，文档不
记录具体 zoom 范围或 tile 数量。

验证目标：

- 当前 release 构建可通过。
- 私有数据端到端测试可完成则记录墙钟耗时；若单轮无法完成，记录代表性
  profile 与峰值内存。
- 根据耗时分布记录后续优化方向；优化内容必须可执行、可验证。

2026-08-13 实施记录：

- 基于当前工作树 release 构建对 1.9G 私有 DEM 发起测试；完整范围预计需要
  数小时，未在单轮会话中跑完，不记录为正式全量墙钟基准。
- 代表性压力阶段采样显示热点稳定收敛在
  `TerrainSamplePlan::sample_heights` → `average_at` →
  `read_sample_raw` → `GeoTiffRasterSource::read_samples` →
  oxigeo `read_window_into` → `CogReader::read_tile_into` →
  `decompress_into_partial` → `oxiarc_lzw::decompress`。
- 主要热点是 LZW 解码临时 `Vec` 分配/增长、字典 reset 和内存分配抖动，
  以及每次采样重建 `proj4rs::Proj`；具体优化方向见
  `TECHNICAL_PLAN.md` P22 与 `TODO.md` P22。
- 本轮不进行 C++ oracle 差分；当前环境没有可用的 C++ 构建/oracle。

后续验证：

- P22 优化后的坐标变换输出与优化前保持一致，既有 CRS/terrain 差分保持
  通过。
- LZW 解码入口改动保持 COG block 输出一致，terrain payload 差分保持 0。
- 缓存几何/预算改动保持命中与未命中结果一致，峰值内存不超出设定预算。
- 全量私有数据端到端基准在可执行的会话中补跑并记录墙钟耗时。

## 21. P23 私有数据性能优化第一轮：Proj 复用

隐私约束与 P22 相同：只允许在文档中记录私有数据文件体积 1.9G，以及可复用
的优化方向和优化内容；不记录路径、名称、CRS、尺寸、分辨率、zoom 范围或
tile 数量。测试产物放在 `/private/tmp`，不进入工作树。本轮不进行 C++
oracle 差分；当前环境没有可用的 C++ 构建/oracle。

测试目标：

- 使用同一 1.9G 私有 DEM、同一代表性范围和同一线程数，比较优化前后
  release 构建的墙钟耗时。
- 代表性输出必须做文件集合与解压后 payload 比较，确认 `proj4rs::Proj`
  缓存没有改变 terrain 数据。
- 单元测试覆盖通用 EPSG 变换：重复调用同一 `(source_crs, target_crs)`
  与首次调用结果精确一致，控制点和未知 EPSG 错误行为不回归。

验证命令：

- `cargo fmt --check`
- `cargo test --lib raster`
- `cargo clippy --all-targets -- -D warnings`
- release 构建后执行私有数据代表性范围前后基准，并在 `/private/tmp`
  比较输出；实施记录只写文件体积 1.9G、代表性前后耗时和输出一致性，
  不写测试文件细节。

2026-08-13 实施记录：

- 代码实现：`src/raster.rs` 使用线程局部 `(source_epsg_code,
  target_epsg_code)` → `ProjectionPair` 缓存，只消除重复
  `Proj::from_epsg_code`/`Proj::init`/projstring 解析；坐标运算、错误
  路径和调用顺序不变。
- 测试结果：`cargo fmt --check`、`cargo test --lib raster`（17/17）、
  `cargo clippy --all-targets -- -D warnings`、`cargo test --test cli`
  （13/13）和 `cargo build --release` 通过；全量 `cargo test` 为 94
  passed，仅保留既有不相关的 `error.rs` 文案断言失败。
- 私有数据结果：使用同一 1.9G 私有 DEM、同一代表性范围和同一线程数，
  分别用优化前与 P23 release 长跑；共同完成的输出文件解压后 payload
  完全一致。两次运行均在完整范围前手动中断且机器负载不稳，因此不以墙钟
  作为正式 A/B 结论；本轮未观察到可确认的端到端加速，说明 Proj 构造在该
  代表性 LZW/采样主导路径中占比小。
- 文档遵守隐私约束：除文件体积 1.9G 外，不记录测试文件路径、名称、CRS、
  尺寸、分辨率、zoom 范围或 tile 数量。

## 22. P24 私有数据性能复测：Rust/C++ 同机对比

隐私约束与 P22/P23 相同：只允许在文档中记录私有数据文件体积 1.9G，以及
可复用的优化方向和优化内容；不记录路径、名称、CRS、尺寸、分辨率、zoom
范围或 tile 数量。测试产物放在 `/private/tmp`，不进入工作树。

测试目标：

- 使用同一 1.9G 私有 DEM、同一代表性范围和同一线程数，分别跑本机
  C++ 0.4.1 oracle 与当前 Rust release，作为一轮可复现的同机对比。
- 代表性输出必须做文件集合与解压后 payload 比较，确认两者仍有既有
  CTB 兼容性，或明确本轮差异范围。
- 若机器负载稳定，记录 Rust/C++ 墙钟并给出比值；否则只记录趋势并说明
  不把中断或高负载单次结果当作正式结论。

验证命令：

- 先确认 C++ oracle 可用：设置动态库搜索路径后执行 `--version`。
- 对同一私有 DEM 分别执行 Rust/C++ `ctb-tile -q -c <同一线程数>
  -s <同一代表性范围> -e <同一代表性范围> -o /private/tmp/<目录>`；
  具体范围和目录不进入文档。
- 用 `find | sort` 比较路径集合，并逐一 `gzip -dc | cmp` 比较解压后
  terrain payload。
- 记录 Rust/C++ 耗时、输出差异状态和下一步优化方向。

2026-08-14 实施结果：

- 已恢复本机 C++ 0.4.1 oracle：补齐动态库搜索路径后版本命令可用。
- 使用同一 1.9G 私有 DEM、同一代表性范围和同一线程数完成 Rust/C++ 对比；
  具体范围与路径集合按隐私约束不写入文档。
- C++ 代表性范围完成墙钟约 43.8 s；Rust 同范围运行约 2h15m 后仍未完成，
  按用户要求手动中断。该差值只作为趋势判断，不作为正式 A/B 墙钟结论。
- Rust 中断时输出集合少于 C++；共同完成的输出中文件集合相同，但解压后
  payload 存在多处差异（包含文件大小不同），另有 Rust 未完成目标未参与
  差分。差异原因需后续单独定位，本轮不把兼容性结论写成通过。
- 直接 GeoTIFF 源单目标采样确认主要热点在
  `GeoTiffReader::read_tile_band_buffer` → `read_window_into` →
  `scatter_bytes` → `decompress_into_partial` → `oxiarc_lzw::decompress`；
  `LzwDecoder::decode` 最大，字典 reset、`add_string_decode` 和
  malloc/free/realloc/memset/memmove 紧随其后，应用层缓存占比很小。
- 后续方向：增加 LZW 直接写入目标缓冲区的解码 API，复用字典缓冲区，
  复核私有数据下 GeoTIFF block cache 预算与访问模式；依赖侧改动必须先
  经 Cargo CLI 流程和授权。
- 文档遵守隐私约束：除文件体积 1.9G 外，不记录测试文件路径、名称、CRS、
  尺寸、分辨率、zoom 范围或 tile 数量。

## 23. P25 Terrain 跨 CRS Average pooled VRT source window

目标：`TerrainSamplePlan::sample_heights` 的 Average 分支在所有 CRS 组合下都
复刻 C++ `GDALTiler::createRasterTile` 的完整 VRT 数据流：overlap destination
transform、`ComputeSourceWindow`、一次 pooled source read、整行
`GWKAverageOrModeComputeLineCoords` 和逐像元加权平均。该优化不修改 LZW
解码，也不引入新算法。

验证命令：

- `cargo fmt --check`
- `cargo test --lib terrain_sampling`
- `cargo test --test cli`
- `cargo clippy --all-targets -- -D warnings`
- `cargo build --release`

oracle 验证：

- 使用 UTM fixture 同时运行 C++ 0.4.1 与 Rust release 的 Terrain 输出，
  比较路径集合和 `gzip -dc` 后的 payload。
- 若 payload 不一致，先修复 P25 的坐标/窗口/权重实现，再重新构建和对比；
  禁止用放宽断言代替 C++ 行为。
- 私有数据只允许记录文件体积 1.9G；本策略不记录路径、名称、CRS、尺寸、
  分辨率、zoom 范围或 tile 数量。

### 23.1 P25 UTM Terrain oracle 实施结果

2026-08-14 实施并验证：

- 使用公开 UTM fixture（由仓库内 Copernicus DEM 测试源派生，32×32、
  EPSG:32649，min=16.77 / max=568.45）同时运行 C++ 0.4.1 与 Rust release
  的 `ctb-tile -q -c 4 -s 7 -e 6` Terrain 输出。
- Rust 与 C++ 各自生成 6 个 `.terrain`（z6-z7），路径集合一致；对每个文件
  `gzip -dc` 后比较，6/6 payload 逐字节一致（SHA-256 均相同）。
- `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo build --release` 通过；`cargo test --lib terrain_sampling` 14/14、
  `cargo test --test cli` 13/13 通过。
- 新增单测 `average_cross_crs_reads_one_pooled_sampling_window`，确认 UTM 源
  跨 CRS 到 geodetic grid 时走一次 pooled sampling window read。
- 补充：完整 `cargo test` 仍被既有 `error.rs` 的 zoom range 消息断言挡住
  （`src/error.rs:46` 使用 `; require:`，`src/error.rs:119` 期望 `:`），该失败
  与 P25 改动无关；排除该单测后 95/95 lib 用例通过。
- 补充观察：同一 fixture 下 C++ 0.4.1 对 `-s 8` 不校验 natural max，仍生成
  z8；Rust 拒绝 `start > natural max`。该差异与 P25 采样路径无关，待后续
  单独确认是否按 C++ CLI 对齐。
- 本轮不评估 1.9G 私有数据性能；LZW 依赖侧优化继续等待 Cargo CLI 授权。

## 24. P26 私有数据 subset 性能测试流程

后续私有数据性能测试固定按以下顺序执行，避免 Rust 长跑占用后 C++ 基准
受机器负载影响：

1. 从 1.9G 私有 DEM 裁出约 200MB 的 subset。
2. 同一 subset、同一 CLI 参数、同一线程数，先跑 C++ 0.4.1，记录完成墙钟。
3. Rust 使用相同参数，timeout = 2x C++ 墙钟；到点未完成则中断并记录部分
   结果。
4. 比较 Rust 超时前共同完成的输出与 C++ 输出的文件集合和解压后 payload。
5. 文档只记录 subset 文件体积约 200MB 和可复用结论，不记录路径、名称、
   CRS、尺寸、分辨率、zoom 范围或 tile 数量。

### 24.1 P26 私有数据 subset 实施结果

2026-08-14 实施结果：

- subset 文件体积约 200MB；Rust 与 C++ 使用同一 subset、同一 CLI 参数和
  同一线程数。
- C++ 0.4.1 完成墙钟约 19.2s；Rust timeout 按 C++ 两倍执行，约 38.3s
  被中断，未跑完。
- Rust 超时前已生成的部分输出与 C++ 共同完成的文件逐一 `gzip -dc` 比较，
  payload 全部逐字节一致。
- 后续仍等待授权推进 `oxiarc_lzw`/`oxigeo` 依赖侧优化，并复核私有数据
  GeoTIFF block cache 预算与访问模式。

## 25. P27 100MB 级别私有数据 subset 复测

继续沿用 P26 固定流程，把 subset 缩小到 100MB 级别，用于缩短滚动迭代
的每轮测试时间。隐私约束不变：文档只记录 subset 文件体积约 100MB，以及
可复用结论，不记录路径、名称、CRS、尺寸、分辨率、zoom 范围或 tile 数量。

### 25.1 P27 实施结果

2026-08-14 实施结果：

- subset 文件体积约 100MB；Rust 与 C++ 使用同一 subset、同一 CLI 参数和
  同一线程数。
- C++ 0.4.1 完成墙钟约 7.89s；Rust timeout 按 C++ 两倍执行，约 15.81s
  被中断，未跑完。
- Rust 超时前共同完成的输出与 C++ 逐一 `gzip -dc` 比较，payload 全部
  逐字节一致。
- 64 MiB block cache 不足以稳定保留低 zoom 一轮访问所需的全部已解码块，
  需要先复核缓存预算。

## 26. P28 私有数据 GeoTIFF block cache 预算复核

约 100MB 私有 subset 下，64 MiB 缓存容量不足低 zoom 一轮访问所需的全部
已解码块，会触发同一批块反复 LZW 解码。本轮把 block cache 预算调整为
819 MiB，与 C++ GDAL block cache 规模对齐，不改 LZW 解码和采样算法。

### 26.1 P28 验证门禁

- `cargo fmt --check`、`cargo test --lib geotiff`、
  `cargo test --test cli`、`cargo clippy --all-targets -- -D warnings`、
  `cargo build --release` 通过。
- 同一约 100MB subset、同一 CLI 参数、同一线程数复测 Rust，C++ 基准仍为
  7.89s，Rust timeout 为 15.78s。
- Rust/C++ 共同输出逐一 `gzip -dc` 比较，payload 必须一致。

### 26.2 P28 实施结果

2026-08-14 实施结果：

- `src/geotiff.rs` block cache 预算调整为 819 MiB，未改 Cargo 依赖、采样
  算法或 LZW 解码路径；验证门禁通过。
- 同一约 100MB subset、同一 CLI 参数和同一线程数复测：C++ 0.4.1 约 7.89s；
  Rust timeout 约 15.78s 被中断，未跑完。
- Rust 超时前共同完成的输出与 C++ 逐一 `gzip -dc` 比较，payload 全部逐字节
  一致。
- 热点仍集中在 `GeoTiffRasterSource::read_samples` ->
  `read_tile_band_buffer` -> `oxiarc_lzw::decompress`；依赖侧优化等待授权。

## 27. P30 脚本化流程首轮真实复测

P29 新增的脚本把后续私有数据对比固定为：先 C++，记录墙钟，再把 Rust
timeout 自动设为 C++ 墙钟两倍。本轮的用途是验证脚本用真实 subset 能完整
跑通这一流程，并继续记录性能基线。

### 27.1 P30 实施结果

2026-08-14 实施结果：

- subset 文件体积约 100MB；Rust 与 C++ 使用同一 subset、同一 CLI 参数和
  同一线程数，命令通过 P29 脚本执行。
- C++ 0.4.1 完成墙钟约 8.151s；Rust timeout 由脚本设为约 16.302s。
- Rust 未在两倍墙钟内完成，约 16.338s 被中断。
- Rust 超时前已生成的部分输出与 C++ 共同完成的文件逐一 `gzip -dc` 比较，
  payload 全部逐字节一致。
- 后续继续按同一脚本化规则滚动测试；LZW 依赖侧优化仍等待授权。

## 28. P31 脚本化流程滚动复测

继续沿用 P30 的固定流程：先跑 C++，记录墙钟，再以 C++ 墙钟两倍作为
Rust timeout。隐私约束不变，只记录约 100MB subset 文件体积、C++ 墙钟、
Rust timeout、Rust 完成/超时状态和 payload 差分结论，不记录测试文件路径、
名称、CRS、尺寸、分辨率、zoom 范围或 tile 数量。

### 28.1 P31 实施结果

2026-08-14 实施结果：

- subset 文件体积约 100MB；Rust 与 C++ 使用同一 subset、同一 CLI 参数和
  同一线程数，命令通过 P29 脚本执行。
- C++ 0.4.1 完成墙钟约 8.316s；Rust timeout 由脚本自动设为约 16.632s。
- Rust 未在两倍墙钟内完成，约 16.665s 被中断。
- Rust 超时前共同完成的输出逐一 `gzip -dc` 比较，33 个共同 `.terrain` 中
  有 1 个 payload 不一致。
- 单独完整复跑同一范围后，该 terrain 文件差异仍稳定存在；该差异先由
  P32 定位，再继续性能优化。

## 29. P35 无效 zoom range 测试期望修正

完整测试必须覆盖 `CtbError::InvalidZoomRange` 的 Display 文本。P35 保持
生产输出不变，只修正测试期望中的标点笔误。

### 29.1 P35 验证门禁

- `cargo test` 必须通过，覆盖原有 120 项单元、集成和 CLI 测试。

### 29.2 P35 实施结果

2026-08-15 实施结果：

- `cargo test` 通过，120 项测试全绿。

## 30. P37 应用层采样与转换优化

P37 在输出一致性边界下优化两个项目侧热点：平均采样循环和 native
GeoTIFF block cache 命中后的 raw bytes 到 `f64` 转换。两项优化都必须保持
读取窗口、NoData、权重公式、浮点计算顺序和数值转换语义不变。

### 30.1 P37 验证门禁

- 平均采样保留既有 `sample_average_pixel` 单元测试，并覆盖窗口分片读取的
  首末行列访问。
- raw bytes 到 `f64` 的专用转换用本机字节序 fixture 覆盖 GeoTIFF 现有
  支持的全部数值类型，并验证长度不匹配时返回错误。
- `cargo fmt --check`、`cargo test`、`cargo build --release` 必须通过。
- 约 100MB 私有 subset 必须复跑 P29 timeout 流程和完整输出对比，42/42
  解压后 payload 保持一致。

### 30.2 P37 实施结果

2026-08-15 实施结果：

- 平均采样既有测试通过，3×3 加权 oracle 覆盖首末行列和中间样本。
- raw bytes 到 `f64` 转换新增单元测试，覆盖全部 8 种支持类型，并验证
  样本数不匹配时返回 `RasterRead` 错误。
- `cargo fmt --check`、`cargo test`、`cargo build --release` 通过；完整测试
  为 122 项全绿。
- P29 timeout 流程已复跑；该轮 Rust 超时前没有共同 terrain。随后完整
  Rust 运行与 C++ 输出路径一致，42/42 解压后 payload 差异为 0。

## 31. P40 OxiGeo 依赖树移除

P40 的测试基线是“依赖替换不可改变可观察输出”。GeoTIFF 读取与写出分别迁移到
`geotiff-reader@0.8.1` 与 `geotiff-writer@0.8.1`；标准 VRT XML 由项目内
`quick-xml` 兼容层解析。测试不得引用 OxiGeo API，也不能把当前依赖实现当作
行为基准。

必备覆盖：

- Reader：4326/3857/任意 proj4rs EPSG、8 种数值样本、NoData 原值透传、
  PixelIsPoint 外角 transform、tile/strip、BigTIFF、LZW/DEFLATE/ZSTD、overview
  元数据和 C++ 已证明的 base IFD 读取行为。
- Cache：同一 GeoTIFF source 被多线程共享时，重复窗口不得重复解码；缓存边界
  与直接 reader 输出一致。
- Writer：样本类型、NoData、GeoTransform、EPSG、Classic/BigTIFF、Predictor、
  tile/strip 与 NONE/DEFLATE/LZW/ZSTD/JPEG/LERC 写读回；JPEG/LERC 的样本类型
  约束沿用既有测试。
- VRT：simple identity、相对路径、source/dst rectangle 裁剪、缩放、band
  NoData、多 source 覆盖、损坏 XML 与缺失 source 的错误路径；warped/pixel
  function/非 GeoTIFF source 必须显式拒绝。
- CLI：VRT 输入仍能生成 terrain/extents；非 GeoTIFF/VRT 扩展在任何输出写入前
  失败；错误文本不再声称由 OxiGeo capability guard 拒绝。
- 门禁：`cargo fmt --check`、`cargo test --all-targets`、
  `cargo clippy --all-targets -- -D warnings`、`cargo build --release`、
  `scripts/verify-ctb-oracle.zsh`、P29 私有 subset 42/42 解压 payload 一致，
  以及 `cargo tree --all-features` 无 `oxigeo*` / `oxiarc*`。

### 31.1 P40 实施结果

2026-08-16 实施结果：

- GeoTIFF 读取迁移到 `geotiff-reader@0.8.1`，跨 worker 共享
  `Arc<GeoTiffFile>` 与其 decoded-block cache；缓存配置为 819 MiB、65536 slots。
  overview 选择继续保留 C++ 已验证的 overview metadata + base IFD 读取行为。
- GeoTIFF 写出迁移到 `geotiff-writer@0.8.1`；`ctb-export`、样本类型、NoData、
  GeoTransform、BigTIFF、Predictor、tile/strip 和 NONE/DEFLATE/LZW/ZSTD/JPEG/LERC
  语义由既有单元/CLI 矩阵覆盖。
- 新增 `src/vrt.rs`：`quick-xml` 解析标准 VRT XML，覆盖相对路径、嵌套 VRT、
  source/destination rectangle、缩放、NoData 与多 source；损坏 XML、缺失 source、
  递归、warped VRT、pixel function 和非 GeoTIFF source 显式失败。
- 输入格式探测识别 Classic TIFF 与 BigTIFF 的 little/big endian header，VRT 只
  读取最多 64 KiB 前缀；其它格式不再读取整个文件后拒绝。
- `cargo fmt --check`、`cargo test --all-targets`（120 项）、
  `cargo clippy --all-targets -- -D warnings`、`cargo build --release` 通过。
- 公开 oracle 5 source × 12 resampling × 2 range 共 120/120 通过，解压后 payload
  一致。
- P29 约 100MB 私有 subset 流程：C++ 与 Rust 均生成 42 个 terrain，路径集合一致，
  42/42 解压后 payload 差异为 0；Rust 在本轮 2 倍 C++ 墙钟上限内完成。
- `cargo tree --all-features` 不包含任何 `oxigeo*` 或 `oxiarc*` crate。

## 32. P41 500MB 级私有 subset 对比测试

P41 不改变实现，只验证 P40 后更大体积输入下的输出一致性和同机耗时趋势。
测试继续使用 `scripts/benchmark-ctb-cpp-rust-timeout.zsh`：C++ 先运行并记录
墙钟，Rust timeout 自动设为两倍墙钟。脚本比较共同 payload 后，还需额外比较
完整输出路径集合，避免仅凭共同文件遗漏缺失或多余输出。

隐私约束不变：文档只记录 subset 体积、C++ 墙钟、Rust timeout/耗时、完成状态、
输出数量和 payload 差分结论，不记录私有输入路径、名称、CRS、尺寸、分辨率、
zoom 范围或 tile 布局。

### 32.1 P41 实施结果

2026-08-16 实施结果：

- 测试输入为 509.0 MiB 私有 DEM subset；C++ 先运行，脚本自动将 Rust timeout
  设为两倍 C++ 墙钟 86.344s。
- C++ 墙钟 43.172s；Rust 墙钟 48.570s，状态 0，未超时。Rust 耗时约为 C++
  的 1.13 倍。
- C++ 与 Rust 均生成 96 个 `.terrain`；完整相对路径集合一致，96/96 解压后
  payload 差异为 0。

## 33. P42 1GB 级私有 subset 对比测试

P42 不改变实现，只验证 P41 后更大体积输入下的输出一致性和同机耗时趋势。
测试继续使用 `scripts/benchmark-ctb-cpp-rust-timeout.zsh`：C++ 先运行并记录
墙钟，Rust timeout 自动设为两倍墙钟。脚本比较共同 payload 后，还需额外比较
完整输出路径集合，避免仅凭共同文件遗漏缺失或多余输出。

隐私约束不变：文档只记录 subset 体积、C++ 墙钟、Rust timeout/耗时、完成状态、
输出数量和 payload 差分结论，不记录私有输入路径、名称、CRS、尺寸、分辨率、
zoom 范围或 tile 布局。

### 33.1 P42 实施结果

2026-08-16 实施结果：

- 测试输入为 971.1 MiB 私有 DEM subset；C++ 先运行，脚本自动将 Rust timeout
  设为两倍 C++ 墙钟 37.876s。
- C++ 墙钟 18.938s；Rust 墙钟 35.249s，状态 0，未超时。Rust 耗时约为 C++
  的 1.86 倍。
- C++ 与 Rust 均生成 89 个 `.terrain`；完整相对路径集合一致，89/89 解压后
  payload 差异为 0。

## 34. P43 1GB LZW 热点复核与 GeoTIFF 采样转换优化

P43 先用同机采样确认 P40 后的真实热点，再实施项目侧零语义变化优化。热点
样本沿用 P42 约 1GB 私有 subset；文档只记录聚合采样结论、耗时、输出数量和
差分结果，不记录私有输入路径、名称、CRS、尺寸、分辨率、窗口、zoom 或 tile
布局。

### 34.1 P43 验证门禁

- 现有 GeoTIFF 单元/CLI 矩阵覆盖样本类型、字节序、BigTIFF、压缩、
  Predictor、overview、NoData 和窗口读取。
- `cargo fmt --check`、`cargo test --all-targets`、
  `cargo clippy --all-targets -- -D warnings`、`cargo build --release` 通过。
- 使用 P42 约 1GB subset、同一 CLI 参数和线程数复测；C++/优化后 Rust 的
  完整输出路径集合一致，89/89 解压后 payload 差异为 0。
- 记录优化前后 Rust 墙钟；C++ 基线沿用 P42 只作同机参考，不把性能收益置于
  输出一致性之上。

### 34.2 P43 实施结果

2026-08-16 实施结果：

- `read_geotiff_window` 直接消费 native-endian decoded band bytes 并转换为
  `f64`，移除 typed `ArrayD<T>` 中间缓冲；overview IFD 样本类型与 base IFD
  不一致时保持拒绝语义。
- 验证门禁通过：`cargo fmt --check`、`cargo test --all-targets`（120 项）、
  `cargo clippy --all-targets -- -D warnings`、`cargo build --release`。
- 优化后采样显示主要剩余热点仍在 `weezl` 解码、解码输出复制和 cache/内层
  Rayon 等待；项目侧采样与 bytes 到 `f64` 转换为小头。
- 最终 release 两轮复测墙钟为 33.25s 和 31.74s；两轮完整路径集合均与 C++
  一致，89/89 解压 payload 差异为 0。

## 35. P44 外部 C++ ctb-tile 切片耗时 CI（不属于 Rust 测试矩阵）

P44 是独立于本项目 Rust 实现的 GitHub Actions 基准，不进入 `cargo test`、
oracle 差分或私有数据对比流程。它使用 Docker Hub 预编译镜像
`homme/cesium-terrain-builder:0.4.1`（内含同版本 0.4.1 的 `ctb-tile`，对应
`ahuarte47/cesium-terrain-builder`，避免在 runner 上从源码构建），验证
`ctb-tile` 能对 `demo/guangxi_8_cities.tif` 完成地形切片，输出
`elapsed_seconds` 与 `terrain_tiles` 到 step summary。

### 35.1 P44 验证范围

- `.github/workflows/ctb-cpp-benchmark.yml` 能被 YAML 解析。
- `git diff --check` 无空白错误。
- workflow 在 `push` / `pull_request` 时触发；LFS 拉取 demo 输入、拉取并运行
  预编译 `ctb-tile` 镜像、`ctb-tile` 非零退出会使 CI 失败。
- 不要求输出与项目 Rust `ctb-tile` 一致，也不作为本项目正确性或性能门禁。
