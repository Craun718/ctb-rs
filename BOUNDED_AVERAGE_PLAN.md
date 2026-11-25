# 大栅格平均采样的有界读取

保留 pooled source window 的坐标转换、边界、margin 和金字塔选择行为。
单个工作线程的原始采样缓冲区超过 64 MiB 时，按目标高程点的源范围读取。
范围过大先按行切分；单行过大再按列切分。持续使用同一组加权平均
累加器，按从上到下、从左到右顺序处理，保留 NoData、非有限值和类型转换行为。
预算保守地按每个样本 8 字节计算，覆盖所有 RasterSamples 表示；不是进程
内存限制，解码缓存等仍额外占用内存。不修改依赖的 1 GiB 解码限制或公开 API。

## 实施顺序

1. 添加内部预算入口和有界读取，保持小窗口一次读取。
2. 以低预算强制分批，逐位比较 Float32、Int16、NoData、非有限值、边界
   权重、横向切分，记录并检查地理/投影坐标及金字塔的读取请求。
3. 运行测试，并用提供的 TIFF 重跑第 4 层，记录耗时和峰值 RSS；
   第 0 层坐标转换越界独立记录，不在本次修改范围内。

## 验证记录

输入：`/home/gxrs/coding/demo/navara/镶嵌DSM_DEM边缘融合.tif`，
26997 × 22231，Float32，EPSG:4545，512 × 512 分块、ZSTD，无内部金字塔。
此文件与原始复现记录的尺寸、类型及压缩方式匹配。基线为父提交
`cde8dc00827696fbea3e0f892ef7db230fdc3efe`，在 `/tmp` 独立源码和 target 中构建。
使用 release `ctb-tile -c 4 -s Z -e Z -o OUTPUT INPUT`。
耗时为墙钟时间，峰值 RSS 来自 Linux `getrusage(RUSAGE_CHILDREN)`，包含缓存等
整个 CLI 进程内存；不是单次采样缓冲区大小。

| 场景 | 基线 | 本次修改 |
|---|---|---|
| 第 4 层 | 失败：2400681228 字节超过 1073741824 解码预算 | 成功生成 `4/25/10.terrain`，3.641 秒，865232 KiB 峰值 RSS |
| 第 9 层（依次重跑） | 20 瓦片，1.847 秒，1947176 KiB 峰值 RSS | 20 瓦片，3.508 秒，860808 KiB 峰值 RSS |

第 9 层路径集合一致，20/20 gzip 解压后的 terrain payload 逐字节一致。
分批读取减少峰值内存，但此大窗口场景额外的小范围读取增加耗时。
以上是本机单次验证，不是稳定性能基准。第 0 层仍返回
`cannot transform EPSG:4326 to EPSG:4545: Coordinate out of range`，属于独立问题。

- `cargo fmt --all -- --check` 通过。
- `cargo test --all-targets`：108 个库测试、1 个 info 测试、9 个参数测试、
  14 个 CLI 集成测试全部通过。新增 4 个测试以多种低预算检查逐位一致性、
  typed/native-byte Float32 与 Int16、NoData/非有限值、边界权重、行列切分、
  地理/投影/跨 CRS、overview 元数据与基础数据读取组合、错误及短缓冲区。
- 桌面根项目 `cargo check -p webgis-toolbox` 通过。
- `cargo clippy --all-targets -- -D warnings` 仍因未修改的 `geotiff.rs`、
  `sampling.rs`、`terrain.rs` 中 10 个既有 lint 失败；本次采样文件未报告 lint。
- 独立代码复核未发现具体正确性问题。
