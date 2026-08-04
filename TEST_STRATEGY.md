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

Cargo 命令必须在禁用沙盒的环境执行。生产代码和测试均不得以 `unwrap` 隐藏预期失败；测试中
若使用 `expect`，消息应说明被验证的不变量。
