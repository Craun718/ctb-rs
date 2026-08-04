# ctb-rs 测试策略

## 1. 原则

测试以 C++ CTB 为 oracle，而不是以当前 Rust 行为为 oracle。每个兼容用例都记录：C++ 提交、
GDAL 版本、命令、输入 checksum、输出路径、解压 terrain payload 或解码 raster、以及比较
结论。压缩容器的时间戳等非语义字段不参与比较。

Rust 常规测试必须不要求 GDAL/PROJ。C++ oracle 允许位于开发或 CI 的隔离环境，生成后的
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

## 3. Fixture 与 oracle 清单

权威清单位于 `tests/fixtures/MANIFEST.md`。新增 fixture 必须补充来源/许可证、生成命令、
checksum、元数据和预期。最低矩阵如下：

| 情况 | Terrain | RasterTiler/GTiff | CRS/overview |
| --- | --- | --- | --- |
| EPSG:4326 Int16、完整 tile | payload、child flags | type/values/transform | direct source |
| Float32、正负/小数值 | `uint16` 截断 | GDAL round/clamp | direct source |
| 仅部分覆盖/右上边界 | destination 0、range、child flags | tile 集 | bounds 包含规则 |
| NoData、缺 SRS、损坏文件 | C++ 对应失败 | C++ 对应失败 | 失败类别与文本 |
| tiled/striped、DEFLATE/LZW、BigTIFF | payload | read/write | I/O 支持 |
| 内部/外部 overview | selected level/payload | selected level/raster | GDAL selection oracle |
| EPSG:3857 direct source | profile 行为 | z0/z1 metadata/values | Mercator Grid |
| 4326↔3857 | 重投影 payload | 重投影 raster | 控制点与 tile oracle |

## 4. 比较方法

- Terrain：解 gzip 后逐字节比较 compact/detailed payload；单独解码 heights、child bitfield 和
  water mask 以定位差异。
- Raster：使用独立纯 Rust reader 比较宽高、band、storage type、NoData、CRS、transform 和
  row-major samples；浮点比较仅在 C++ 输出本身不稳定时使用已记录 epsilon。
- CLI：捕获 stdout、stderr 和 exit status；路径用临时目录归一化后比较。进度输出可按 C++
  的并发不确定性比较格式与数量，不强制线程 id。
- 坐标：固定检查 C++ 控制点；不能用“视觉上正确”取代数值对照。

## 5. 每个变更的验证流程

1. 先新增或更新 C++ oracle，并令其在未改 Rust 前失败；
2. 新增领域/适配器测试锁定最小边界；
3. 实现后运行 `cargo fmt --check`、`cargo test` 和 `cargo clippy -- -D warnings`；
4. 运行受影响的 CLI 差分与多线程一致性测试；
5. 在 `TECHNICAL_PLAN.md` 与 `TODO.md` 记录证据、遗留差异和下一任务。

## 6. P1 RasterTiler 通用 Grid 追加策略

`RasterTileset` 接入 `TileGrid` 时，`tests/cli.rs` 已以 EPSG:3857 全世界 direct-source fixture
覆盖 z0：断言 `{z}/{x}/{y}.tif` 路径、输出 CRS、affine transform 及恒定样本；并以 EPSG:4326
输入 + Mercator profile 断言写入前失败、没有 `.tif`。z1 的 C++ 差分 fixture 仍待补充。输出
affine transform 必须等于 `GlobalMercatorGrid::tile_bounds` 与 `resolution`，像素值必须按既有
RasterTiler footprint 采样。保留 EPSG:4326 Geodetic 现有 tests 作为无回归门禁。

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

GTiff creation-option 测试至少覆盖 `COMPRESS=NONE/DEFLATE/LZW` 的写出与 Rust reader 读回，
并断言未知选项在创建任何 tile 前失败；压缩编码的字节级差异保留给 C++ oracle。
当前三种压缩均有 CLI 写出/读回证据；C++ 字节差分尚未执行。

Overview fixture 使用 `geotiff-writer` 的多级 top-level COG：断言 overview 数量、ratio 在
1/2/4 附近的选择、overview metadata 的像元尺寸和 `read_sampling_window` 的实际样本；
再以 C++ `GDALSuggestedWarpOutput2`/`getOverviewDataset` 输出复核 tie 与边界规则。

CLI 默认值测试分别覆盖 Terrain 65、GTiff 256、geodetic extents 65、mercator extents 256；
Terrain 携带 creation option 必须在输出目录写入前失败。

`ctb-tile` 参数测试还必须覆盖 `-z` 默认 `0.125`、`-m` 默认 `0` 的解析、负数/非有限值
拒绝，以及非默认值在任何 tile 写出前显式返回未实现错误。待 C++ oracle 恢复后，再比较
`-z` 对投影结果的影响，并确认 `-m` 是否仅为执行资源提示。

当前 Rust overview fixture 已覆盖 2/4 倍 top-level overview、1.5/2/4 ratio 选择、派生
GeoTransform 和窗口样本；全套测试 77 项通过，C++ tie/boundary 差分仍待补。

Cargo 命令必须在禁用沙盒的环境执行。生产代码和测试均不得以 `unwrap` 隐藏预期失败；测试中
若使用 `expect`，消息应说明被验证的不变量。
