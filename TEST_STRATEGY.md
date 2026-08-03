# ctb-rs 测试策略

## 分层

1. **领域单元测试**：不触及文件系统，覆盖格网、仿射定位、瓦片坐标、NoData 契约与 terrain payload。
2. **GeoTIFF 驱动集成测试**：对每个小型 fixture 验证元数据、窗口读取、NoData、tiled/striped 和 overview 行为。
3. **CTB 语义兼容测试**：相同输入分别由原 CTB 与 `ctb-rs` 生成；比较瓦片集合、解压 payload 布局、child bits、water mask 和每个高程样本的绝对差（首期不超过 1）。
4. **端到端测试**：生成的 terrain 由独立 reader 解析；Quantized-Mesh 阶段再加入 Cesium/terrain-server smoke test。

## 首批 fixture 清单

| 名称 | 目的 | 当前来源 |
| --- | --- | --- |
| `epsg4326-small-int16.tif` | 65×65 边界、负值与 `i16` 高程 | 待在 GeoTIFF spike 中程序化生成 |
| `epsg4326-small-f32.tif` | 浮点读取和最终量化 | 待生成 |
| `epsg4326-tiled-overview.tif` | tile/strip layout 与内部 overview 选择 | 待生成或纳入可再分发样本 |
| `epsg4326-nodata.tif` | NoData 必须失败 | 待生成 |
| `epsg3857-rejected.tif` | 首期 CRS 拒绝路径 | 待生成 |

Fixture 引入前必须记录生成脚本、许可证和预期元数据。不能将大体积或来源不明的 DEM 直接提交。

## Oracle 规则

- 原 CTB 用固定版本、固定参数和单线程生成 oracle，记录命令、输入 checksum、瓦片路径及解压后的 payload checksum。
- 比较 gzip 前 payload，避免压缩头时间戳等非语义差异。
- 高程比较按坐标索引记录最大差异；任一 NoData、child bit、mask 或瓦片集合差异均为失败。
- 在字节级兼容阶段前，不以压缩文件字节差异作为失败条件。
