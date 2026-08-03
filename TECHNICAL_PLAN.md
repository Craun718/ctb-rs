# ctb-rs 技术方案

## 1. 目标与边界

将 Cesium Terrain Builder（CTB）重写为纯 Rust 实现，消除对 GDAL、PROJ 以及其他 C/C++ GIS 运行时的依赖。项目首先交付一条可用、可验证的垂直链路：

```text
EPSG:4326 GeoTIFF DEM
  -> 窗口化读取 / 采样 / 重采样
  -> TMS Global Geodetic 瓦片格网
  -> CTB heightmap-1.0 或 Cesium Quantized-Mesh 1.0
```

本项目长期目标是替代 CTB 所使用的 GDAL 能力，而不是在第一阶段宣称替代整个 GDAL。新增格式和 CRS 必须通过稳定的 Rust trait 接入，不能重新耦合 CLI、瓦片逻辑和格式解码器。

### 已确认的产品决策

| 项目 | 决策 |
| --- | --- |
| 原生性 | 禁止 GDAL、PROJ 及 C/C++ FFI GIS 依赖 |
| 首期输入 | GeoTIFF，且坐标参考系必须为 EPSG:4326 |
| NoData | 直接报错，要求调用方预处理填补（与原 CTB 一致） |
| 输出 | 先兼容 CTB `heightmap-1.0`，再增加 Quantized-Mesh 1.0 + `layer.json` |
| 命令行 | 保留 `ctb-tile`、`ctb-info`、`ctb-export`、`ctb-extents` |
| 验收 | 先语义一致：格式、覆盖瓦片、瓦片接缝一致；高程误差首期不超过一个输出 `i16` 单位。后续另设字节级兼容阶段 |

### 明确不在第一期

- 任意 CRS、PROJ pipeline、EPSG 数据库的完整覆盖。
- GeoTIFF 之外的读取驱动、VRT XML 兼容和任意格式写出。
- 旋转/剪切仿射 GeoTransform、GCP/RPC/geolocation-array。
- 自动 NoData 修补、水体推断、影像多波段瓦片输出。
- 原 CTB `--output-format <任意 GDAL 驱动>` 的泛化输出能力。

这些项目均属于后续扩展路线，不能以临时条件分支进入首期实现。

## 2. 原 CTB 行为盘点

原项目将下列责任交给 GDAL：

1. 打开任意栅格、读取第一波段、读取 GeoTransform、SRS 和 overview；
2. 将数据集范围转换到目标格网 CRS；
3. 为每个目标瓦片创建内存 VRT，并按指定算法 warp/resample；
4. 从重采样结果读取 65×65 高程；
5. 将高程写为 heightmap terrain，或交给 GDAL 写其他栅格格式；
6. `ctb-export` 使用 GDAL 写 GeoTIFF。

`ctb-rs` 的核心不是复刻 GDAL VRT；而是将其替换为显式、可测试的 `RasterSource -> WarpPlan -> TileSampler` 数据流。首期因输入和目标都是 EPSG:4326，`WarpPlan` 会退化为仿射坐标映射，但接口必须为 CRS 转换预留位置。

## 3. 目标架构

建议采用 Cargo workspace，按职责拆分。名称可在实现时微调，公共依赖方向不能反转。

```text
ctb-core (无 I/O 的格网、几何、采样、terrain 数据模型)
  ^        ^             ^
  |        |             |
ctb-raster  ctb-terrain  ctb-quantized-mesh
  ^             ^              ^
  |             |              |
ctb-geotiff ----+--------------+
  ^
ctb-cli (ctb-tile / ctb-info / ctb-export / ctb-extents)
```

### `ctb-core`

- `Crs`: 首期仅接受规范化后的 `Epsg(4326)`；未知、缺失或不兼容 CRS 返回结构化错误。
- `AffineTransform`: 完整表示六参数 GeoTransform；首期校验北向上、无旋转/剪切。
- `Bounds`, `TileCoord`, `TileRange`, `GlobalGeodeticGrid`, `GlobalMercatorGrid`。
- `RasterMetadata`: 宽高、波段、样本类型、NoData、CRS、仿射定位、金字塔描述。
- `Resampling`: `Nearest`、`Bilinear`、`Average` 首先实现；其他 CTB CLI 所列算法以枚举保留，但未支持时清晰失败。
- `TileSampler`: 用目标像元中心映射到源空间并按重采样器取样；产生 `Vec<f64>`，最终编码阶段再量化。

### `ctb-raster`

定义纯 Rust 扩展边界：

```rust
trait RasterSource: Send + Sync {
    fn metadata(&self) -> &RasterMetadata;
    fn overview_levels(&self) -> &[Overview];
    fn read_window(&self, request: WindowRequest) -> Result<RasterWindow>;
}
```

- `RasterWindow` 明确携带边界、采样类型、NoData 掩码和 overview 层级。
- 选择“不比目标分辨率更粗、且最接近”的 overview；没有内部 overview 时使用原始层。首期不生成 `.ovr`。
- 读取窗口必须带 halo，以支持双线性和平均采样，且瓦片间共享相同世界坐标计算，保证接缝一致。
- 通过有界 LRU 块缓存和 `rayon` 并行瓦片任务实现吞吐；缓存键含 source id、overview、band、块坐标。

### `ctb-geotiff`

- 基于 Rust TIFF/GeoTIFF 解析能力构建，不链接 GDAL/PROJ。选型前做一项 spike：确认候选库能读取 BigTIFF、tiled/striped TIFF、压缩、全部 DEM 常见整数/浮点样本类型、GeoKeyDirectory、ModelPixelScale、ModelTiepoint 与内部 overview。
- 解码首个波段，识别 `GDAL_NODATA` tag；任一参与输出的 NoData 值立即返回 `NoDataEncountered` 错误。
- 校验 CRS 为 EPSG:4326、仿射变换非旋转、像素尺度有效。错误要指出标签、CRS 或变换为何不支持。
- COG 不是独立格式：只要满足 range-capable reader 和 TIFF tile/overview 规则，即在 GeoTIFF 驱动中支持。网络 HTTP Range 读取放在其后的 I/O adapter 阶段，首期只需本地文件。

### `ctb-terrain`

- 严格实现 CTB 的 heightmap-1.0：65×65 默认 `i16` little-endian 高程、child bitfield 与单字节 all-land water mask，并使用 gzip。
- 独立实现 reader/writer；`ctb-info` 使用 reader，不能反向依赖 GeoTIFF。
- 在所有层级生成完毕后，根据真实已写出的子瓦片回填四个 child bits，避免原先“仅按范围推断”造成的错误。
- 高程转换必须显式定义舍入、范围检查和溢出策略；首期拒绝无法表示为 `i16` 的高程，不静默截断。

### `ctb-quantized-mesh`

- 复用 `TileSampler` 输出，编码 Quantized-Mesh 1.0 头部、顶点、三角形、edge indices；同时生成符合 Cesium 预期的 `layer.json`。
- 第一版选择规则网格三角化，先保证高度和边缘一致；网格简化、skirt、法线、水面掩码、metadata 扩展设为独立迭代。
- 该编码器不应影响 heightmap 的字节格式和路径布局。

### `ctb-cli`

- 用 `clap` 定义四个二进制或一个多子命令二进制并保留原可执行文件名。
- `ctb-tile`：首期接受 GeoTIFF、`geodetic` profile、`Terrain`/`QuantizedMesh` 输出；保留原有选项语义，尚不支持的选项报“未实现”，不忽略。
- `ctb-info`：读取并检查 heightmap；后续增加 `--format` 支持 Quantized-Mesh。
- `ctb-export`：将 heightmap 导出为 EPSG:4326 GeoTIFF，由 `ctb-geotiff` writer 生成。
- `ctb-extents`：基于 `RasterMetadata` 与格网生成每层 GeoJSON，不读取像元。

## 4. 分阶段实施计划

### Phase 0：基线、规格与测试资产

1. 固化原 CTB 版本/提交和命令行 golden fixtures。
2. 收集有授权的小型 EPSG:4326 DEM：整瓦片、跨瓦片边界、负高程、浮点、内部 overview、striped/tiled、含 NoData、损坏元数据。
3. 以原 CTB 为 oracle，记录每个 fixture 的瓦片范围、最大 zoom、解压后高程、child flags、gzip 外的裸 payload。
4. 建立 `cargo nextest`/集成测试和模糊测试入口；所有 fixture 的许可证和来源写入清单。

完成标准：测试可在没有 GDAL 的 Rust CI 环境运行；oracle 数据已被提交或可复现下载。

### Phase 1：纯 Rust heightmap MVP

1. 建立 workspace、错误模型、格网数学和 tile path 布局。
2. 实现本地 EPSG:4326 GeoTIFF 单波段读取与元数据验证。
3. 实现 nearest/bilinear/average、窗口读取、最大 zoom 计算、Global Geodetic 遍历及 65×65 东北像素重叠。
4. 实现 heightmap reader/writer、gzip、child flags、`ctb-info`。
5. 实现 `ctb-tile --output-format Terrain`、`ctb-extents` 和 `ctb-export`。

完成标准：四个 CLI 可用；输出在 Cesium 中正常加载；golden 中瓦片坐标、覆盖范围、解压格式和 child 标记一致，所有有效高程与 CTB 相差不超过 1 个 `i16` 单位。

### Phase 2：性能和大文件可靠性

1. tiled/striped、BigTIFF、常见压缩与内部 overview；基准测试大 DEM。
2. 有界块缓存、`rayon` 并发、确定性的输出排序与 `--resume`。
3. 内存、文件描述符、损坏 TIFF、极端边界经纬度的回归和 fuzz 测试。

完成标准：内存上界可配置；单/多线程结果内容一致；典型 DEM 的性能基线记录在仓库中。

### Phase 3：Quantized-Mesh

1. 规则网格 Quantized-Mesh 与 `layer.json`。
2. 相邻瓦片边界一致性、terrain-server/Cesium 端到端测试。
3. 再评估 skirt、法线、水体掩码和网格简化。

完成标准：同一 GeoTIFF 可选择输出两种 terrain 格式；Cesium 可在层级切换中无裂缝渲染。

### Phase 4：纯 Rust CRS 扩展

1. 将 `Crs` 扩展成登记式转换器，先实现 EPSG:3857 和 EPSG:4326 <-> 3857。
2. 实现 UTM/WGS84 等明确优先级的投影，不承诺“任意 EPSG”直到有独立的 datum、椭球、投影与 WKT/PROJJSON 解析能力。
3. 为每个 CRS 维护权威控制点、反变换误差测试和轴序策略（固定 GIS x/y）。

完成标准：每新增 CRS 都有正反变换精度阈值、范围限制和明确的 datum 假设；绝不静默把未知 CRS 当作 WGS84。

### Phase 5：格式与虚拟数据集生态

1. 新增 `RasterSource` 驱动：HGT、ASCII Grid、DTED、ENVI 等，按实际需求排序。
2. 实现 Rust 原生的 mosaic/composite 描述格式；仅在确有互操作需求时考虑受限 VRT XML reader。
3. COG HTTP Range、外部概览、更多输出格式。

## 5. GeoRust 与依赖原则

- 几何与边界类型优先使用 GeoRust 的 `geo-types` / `geo`，但不把栅格读写或投影责任伪装进几何库。
- TIFF/GeoTIFF 解码器必须是纯 Rust；在 Phase 0 spike 结束前不锁死具体 crate 和版本，以 API、许可证、维护状态、BigTIFF/overview/压缩覆盖和无 FFI 为准。
- 使用 `thiserror`、`clap`、`flate2`（纯 Rust 后端）、`rayon`、`serde`/`serde_json` 等通用 Rust 库时也必须确认其启用的 feature 不引入系统 GIS 库。
- 依赖锁定、SBOM/`cargo deny` 和 CI 中 `cargo tree` 检查是交付条件，防止后续间接引入 GDAL/PROJ。

## 6. 兼容性与数值策略

- 将“CTB 兼容”拆成：CLI 契约、瓦片编号/路径、无压缩 terrain payload、gzip、采样数值、Cesium 视觉效果。逐项测试，不能只比较压缩文件。
- 统一使用 `f64` 做世界坐标与采样累积，最终单点、显式舍入为 terrain 的 `i16`；禁止依赖平台默认转换。
- 明确像元解释（PixelIsArea/PixelIsPoint）并用 fixture 覆盖。terrain 的东/北额外一像素必须从同一世界坐标采样，不能通过相邻瓦片复制。
- 边界范围、Antimeridian、极点、坐标反向和浮点 epsilon 都需建立专门测试。

## 7. 主要风险与缓解

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 纯 Rust GeoTIFF 库缺少关键 TIFF 功能 | 首期读不到真实 DEM | 在 Phase 0 先 spike；必要时在 `ctb-geotiff` 内补齐解析，而非引入 GDAL |
| GDAL 与自研重采样数值不同 | 无法字节级匹配 | 首期以语义/容差验收；保存 oracle 中间栅格，定位坐标、核函数和舍入差异 |
| “任意 CRS”范围无限 | 计划失控 | CRS 以登记式逐个交付；每个 CRS 有明确范围、datum 和误差指标 |
| 大型 TIFF 内存与 I/O 压力 | 生产不可用 | 窗口化、块缓存、有界队列、并发基准；禁止全图解码 |
| Quantized-Mesh 细节复杂 | 延误核心 terrain 输出 | 独立于 MVP，在 Phase 3 从规则网格开始 |

## 8. 建议的下一步

开始 Phase 0：先建立 workspace 骨架和测试 fixture 清单，并做 GeoTIFF 纯 Rust依赖 spike。spike 的输出是选型报告与最小读取验证，不开始实现未验证的格式抽象。

## 9. 实施状态与本轮边界

**状态：Phase 0 已启动（2026-08-04）。**

已完成：无 I/O 的领域模块、`RasterSource` 契约、Global Geodetic 格网和 CTB heightmap 原始 payload 的最小测试已落地；`cargo test` 已通过。GeoTIFF spike 已选定 `geotiff-reader` 0.8.0 的 `local` feature：其声明为纯 Rust、MIT/Apache-2.0、MSRV 1.85，公开 API 覆盖 EPSG、仿射变换、NoData、窗口读取和 overview。依赖树已确认不含 GDAL、PROJ 或 GIS FFI；当前待用可再分发 fixture 验证实际解码能力。

本轮目标：以纯 Rust 生成的小型 GeoTIFF fixture 验证 reader 的公共 API，并据此实现受限的 `GeoTiffRasterSource`。适配器只接受 EPSG:4326、north-up、单波段 DEM；不支持的 CRS、旋转变换、NoData 或窗口越界必须返回结构化错误，绝不做隐式转换或填补。

本轮结果：`geotiff-writer` 0.8.0（开发依赖）生成的 EPSG:4326 小型 `f64` GeoTIFF 已被 `geotiff-reader` 成功读取。已验证 CRS、north-up transform、首波段窗口读取、NoData 命中、非法 CRS 及窗口越界；生产依赖保持为 reader，writer 仅用于测试。tiled/striped、BigTIFF 和内部 overview 仍未验证，保留在 Phase 2 前的 spike 清单中。

下一实施单元：采样器以 GeoTIFF 的 PixelIsArea、corner-based affine transform 为准。世界坐标先转换为连续像元中心坐标 `(col - 0.5, row - 0.5)`；nearest 选择包含目标点的像元，bilinear 对相邻四个像元做线性插值，并在 source 边缘钳制邻居索引。平均重采样与目标瓦片窗口规划随后实现，避免过早把核函数与瓦片遍历耦合。

采样器结果：nearest、bilinear、边缘邻居钳制和世界范围拒绝已实现为格式无关的 `sample_at`，并以内存 `RasterSource` 做了独立测试。`Average` 目前会返回明确的“需要输出像元 footprint”错误；这符合设计，直到瓦片窗口规划能提供精确面积为止。

下一实施单元：建立 terrain 目标瓦片采样规划。对 `GlobalGeodeticGrid` 中的每个 `TileCoord`，使用 `tile_size - 1` 个 cell 将瓦片范围均分为 `tile_size × tile_size` 个高度样点；东、北样点位于瓦片实际上界，使相邻瓦片共享同一世界坐标。采样器将接受一个目标样点中心和其 cell footprint：nearest/bilinear 用中心点，average 对 footprint 中覆盖的源像元中心取均值。首期 average 采用明确、确定的中心点覆盖规则，非面积加权；这一规则须在 CTB golden 对比前重新评估。

目标瓦片规划结果：`TerrainSamplePlan` 已输出行优先的 `Vec<f64>`，并验证相邻瓦片的东西与南北边缘坐标相同。Average 已按 footprint 覆盖的源像元中心作确定性均值；没有中心落入 footprint 时回退为 nearest。该回退和中心点平均仍是首期实现选择，必须在接入原 CTB golden fixtures 时以实际差异校正。

下一实施单元：heightmap 量化器采用 Rust `f64::round`（半值远离零），随后做有限性和 `i16` 范围验证；NaN、无穷大及超范围高程均返回错误，不做截断。量化器只构建 `HeightmapTerrain`，child flags 由后续 tileset 规划阶段根据实际生成的子瓦片设置，不能在单瓦片编码时猜测。

量化器结果：该早期实现已被 CTB 兼容编码替代；最终 API 为 `HeightmapTerrain::from_sampled_meters`，输出 `u16` 的 offset/scale 值。child flags 保持由调用方显式传入，尚未关联 tileset 遍历。

下一实施单元：将已存在的 raw heightmap payload 包装为 gzip。压缩库须以纯 Rust 后端启用；API 分为内存 `encode_gzip`/`decode_gzip` 与路径 I/O，前者用于确定性格式测试，后者供 future CLI 使用。解压必须限制 payload 大小为 CTB 所允许的 compact/detailed 两种长度，防止无界资源消耗。

gzip 容器结果：`flate2` 使用 `rust_backend` feature 直接依赖，解析到的 `zlib-rs` 也是 Rust 实现，不含系统 zlib 或 C FFI。compact/detailed payload 的内存和文件往返、损坏 gzip 流、超出 terrain 最大 payload 的解压均已测试；解压上限为 detailed payload 加一个检测字节。

下一实施单元：tileset 规划器将以 `RasterMetadata` 的 north-up EPSG:4326 bounds 和像元宽度决定 max zoom；遍历每个 zoom 内与数据集相交的 Global Geodetic `TileCoord`，先生成 all-land tile 的高程 payload，再按下一层源覆盖范围推导各 tile 的 child bit，最后写 `{z}/{x}/{y}.terrain`。写入采用临时文件后 rename，`resume` 只跳过已经存在的最终 tile。单元测试先使用内存输出计划验证坐标集合和 child flags，文件系统测试单独覆盖路径布局与原子写入。

规划器结果：max zoom、非零面积相交的上界排他范围和 child bit 推导均已实现并通过测试。写入实现被以下待决策项阻塞：对于仅覆盖部分 TMS tile 的局部 DEM，瓦片中落在 DEM bounds 之外的目标样点没有栅格值；严格 NoData 策略仅规定了已读到的 NoData 样本，尚未定义该空间外区域的高程策略。该策略影响边缘地形、接缝和与 CTB 的兼容性，必须先确认。

兼容性决策（已确认）：以原 CTB 源码与其使用的 GDAL 默认 warp 行为为准。heightmap 的整数值表示 0.2 米单位、相对 -1000 米基准：`encoded = (meters + 1000) * 5`。Rust 实现仍会显式检测非有限和 `i16` 溢出；截断方向和 DEM 覆盖范围外的 destination 初始化值必须从本地 GDAL 源码与 oracle fixture 中验证后实现，不能由 Rust 默认转换或猜测决定。

已验证的 CTB/GDAL 事实：`i_terrain_height` 是 `uint16_t`；CTB 将 VRT 的 `Float32` 高程以 C++ `uint16_t((height + 1000) * 5)` 写入（有效范围内向零截断）。`GDALCreateWarpedVRT()` 会在未设置 `INIT_DEST` 时设为 `INIT_DEST=0`，所以没有 source coverage 的目标样点以 `0 m` 初始化，编码值为 `5000`。计划中任何 `i16` 或直接写米值的描述均被此事实取代。对超出 `u16` 可表示范围的输入，Rust 仍返回结构化错误，而不复刻 C++ 的未定义转换行为。

落实规则：采样请求的中心不在 `RasterMetadata` bounds 内时不调用 source，而是返回 CTB VRT 的 destination 初始值 `0.0 m`。中心在 bounds 内仍委托 source；若读取到显式 NoData 则保留失败行为。此规则仅模拟已确认的无 source coverage 情况；重采样核在 source 边界的细节继续由 CTB oracle fixture 对比校准。

tileset 写入结果：`write_heightmap_tileset` 按最深 zoom 到最低 zoom 的顺序写 `{z}/{x}/{y}.terrain`，以已成功写出或 `resume` 时已存在的实际子 tile 设置 parent child bits。输出先写同目录临时文件后 rename；写入、gzip 回读和 resume 跳过路径均有测试。Average footprint 的空覆盖范围会回退 nearest，避免 NaN；完整 GDAL kernel 边界行为仍待 oracle 确认。

下一实施单元：CLI 使用 `clap`，保留 `ctb-tile`、`ctb-info`、`ctb-extents` 和 `ctb-export` 四个二进制名。首个子项只将已经可用的 heightmap 库 API 暴露为 `ctb-tile --output-dir --resume INPUT` 和 `ctb-info [--show-heights] [--no-child] [--no-type] INPUT`；不支持的 CTB 选项必须报错，不能静默忽略。extents 和 export 在各自 writer 已验证后接入。

CLI 子项结果：`ctb-tile` 和 `ctb-info` 已实现且保留原命名、关键参数与输出含义。`ctb-tile` 暂只输出 CTB heightmap、`geodetic` profile 和当前实现的 Average 采样；未支持参数由 clap 拒绝。下一子项是 `ctb-extents`，它只需已经稳定的 `TilesetPlan`，不增加 GIS 依赖。

`ctb-export` 兼容规则（已由 CTB 源码确认）：输出 GeoTIFF 的 band 标记为 signed 16-bit，但其 buffer 直接复用 terrain 的 `uint16` bit pattern，故大于 32767 的编码值在导出 TIFF 中显示为负数；不做 offset/scale 反算。输出 CRS 是 EPSG:4326，transform 的像元宽高为瓦片 bounds 除以 65。该 writer 使用已验证的纯 Rust `geotiff-writer`，需作为生产依赖。

CLI 完成状态：四个可执行名均已实现。`ctb-extents` 从 `TilesetPlan` 写逐层 GeoJSON；`ctb-export` 使用生产级纯 Rust writer，保持原 CTB 的 signed bit-pattern 和 65 像元 transform。现在以子进程端到端测试锁定实际 CLI 契约。

P1 验证状态：四个 CLI 的子进程级端到端测试均已通过。CTB golden 对比进入执行：环境已提供 cmake 与 GDAL，将在隔离构建目录编译已提供的原 CTB 源码。源码审计已固定高度编码和无 coverage 初始化值；完整 raster warp kernel 将以该可执行 oracle 逐 payload 确认。

oracle 发现的输入兼容修正：`geotiff-reader` 的 typed window API 不会自动从 GeoTIFF 原生类型转成 `f64`。`RasterSource` 的公共样本类型仍为 `f64`，但 GeoTIFF adapter 将以 DEM 常用的 `u8/i8/u16/i16/u32/i32/f32/f64` 原生类型解码，再进行显式 Rust 转换；不可识别类型返回结构化读取错误。该修正是实现既有“样本类型 → f64 栅格契约”的必要补全。

首个 CTB executable oracle 已完成：将同一份 2×2、EPSG:4326、Int32 GeoTIFF 输入原 CTB 与 Rust 实现，两侧都生成了 zoom 0–2 的 10 个 gzip terrain 文件，解压后的每个 payload 均为 8,452 bytes（4,225 个 `u16` 高程加 compact 后缀）。这确认了格网层级、文件布局和 payload 结构。字节差异仅落在源像元边界，逐 tile 为 2–16 bytes；例如 `0/0/0` 仅有两个高度样本不同。下一单元不改变产品范围：以 CTB `TerrainTiler` 的 VRT 目标范围、像元中心及 GDAL `Average` 边缘覆盖语义为契约，先建立该行为的领域级测试，再替换当前连续双线性/中心点 average 的边缘路径，直至最小 fixture 的原始 payload 一致。

重叠像元修正结果：`TerrainSamplePlan` 已改为 CTB VRT 的 west/north-overlap 像元中心与完整像元 footprint。最小 oracle 的 zoom 0、zoom 1 共 6 个裸 payload 已逐字节一致；zoom 2 的 4 个瓦片仍各有 3–5 个高度样本差异。原因已缩小为目标像元小于源像元时，当前 Average 的“落在 footprint 内的源像元中心等权平均”与 GDAL 基于相交面积的覆盖权重不同。下一单元在不修改格网、量化、文件格式或边界偏移的前提下，将 Average 改为轴对齐 PixelIsArea 的精确面积加权；无相交面积时仍返回 GDAL 已确认的 `0 m` destination 初值。

P1 oracle 完成：Average 已替换为 north-up、轴对齐 PixelIsArea footprint 与源像元的精确相交面积加权；无覆盖面积返回 `0 m`。同一 2×2 Int32 EPSG:4326 fixture 在原 CTB 与 Rust 端均生成 zoom 0–2 的 10 个 terrain 文件，所有 gzip 解压后的 8,452-byte payload 都以 `cmp` 逐字节一致。这覆盖了 west/north overlap、上采样和下采样边缘、Float32 转换、量化、路径、child bit 与 compact 后缀。该 oracle fixture 现为后续扩展的回归基线；更大/压缩/tiled/overview GeoTIFF 与其他 resampling 方法仍按 Phase 2 单独增加。

GeoTIFF 能力验收完成：用 GDAL 在临时目录从该 fixture 生成 `TILED=YES`、`COMPRESS=DEFLATE`、内部 1×1 overview 的 GeoTIFF，纯 Rust `ctb-tile` 成功读取并生成相同的 10 个 terrain 文件；其全部裸 payload 与未分块源逐字节一致。首期 reader 的 tiled、DEFLATE 压缩和 overview 元数据/读取路径已获真实文件验证。大型多 block、BigTIFF 与不同压缩编码仍不扩展为首期承诺。

## 10. 原版 CTB 差异盘点与本轮实施边界

2026-08-04 以原版 `tools/*.cpp` 与当前四个 Rust 二进制逐项核对后的结论如下：

| 能力 | 原版 CTB | 当前 ctb-rs | 本轮处置 |
| --- | --- | --- | --- |
| GeoTIFF -> `Terrain`、Global Geodetic、`--resume` | 支持 | 已支持且最小 oracle 裸 payload 字节一致 | 保持回归 |
| `--start-zoom` / `--end-zoom` | 支持 | 缺失 | 实现并验证范围、路径和 child flags |
| `nearest` / `bilinear` / `average` | 支持（另有更多 GDAL 算法） | 领域层已实现，CLI 固定 average | 接入三个已实现算法；其余算法明确拒绝 |
| `--tile-size` | 支持任意尺寸 | 固定 65 | Terrain 仅接受 65；其他值明确拒绝，避免改变 heightmap-1.0 规格 |
| `--profile mercator` | 支持 | 未支持 | 维持 Phase 4 CRS/格网范围，明确拒绝 |
| 任意 GDAL 输出格式、creation option、warp 参数 | 支持 | 未支持 | 维持“不引入 GDAL 泛化写出”的首期边界，明确拒绝 |
| 线程数、进度输出 | 支持 | 未支持 | 性能并发仍属 Phase 2；本轮不伪造参数 |
| Quantized-Mesh / `layer.json` | 原版不提供 | 未提供 | 依既定 Phase 3 独立实现 |

本轮实现保持 EPSG:4326、单波段 north-up GeoTIFF 与 CTB `heightmap-1.0` 的既定契约。先在不含 I/O 的 `TilesetPlan` / 写入 API 中加入 zoom 范围与 `Resampling` 选择，再让 `ctb-tile` 解析原版同名参数。范围必须满足 `0 <= end <= start <= 自动 max zoom`；child mask 按下一层源覆盖推导。三个重采样选项必须由固定 oracle fixture 和 CLI 集成测试覆盖；未实现的原版选项须在参数解析期给出清楚错误，绝不静默忽略。

本轮验证结果：受限 zoom 范围、`nearest`、`bilinear`、`average`、`--tile-size 65` 和 `--profile geodetic` 已接入；CLI/领域测试通过。原 CTB oracle 对 `-s 1 -e 1` 的三个算法共 4 个瓦片逐高度样本均一致（每个裸 payload 的前 8,450 字节一致）。末尾 child flag 存在唯一分歧：原 CTB 对未生成的 z=2 覆盖子瓦片仍设 bit；现有方案规定只按实际写出的子瓦片设 bit。该差异不能在不改变既定技术方案的情况下消除，后续实现暂停等待产品决策。

产品决策（2026-08-04）：为保持原 CTB 的限定 zoom 输出字节兼容，child mask 改为由源数据在下一 zoom 的可覆盖 tile 推导，与该子瓦片是否在本次命令中写出无关。完整输出和 `--resume` 的结果不变；限定范围时 child mask 可能引用本次目录中不存在的子文件，这是原版可观察行为，已作为兼容性契约接受。

验收结果：使用原 CTB executable 与同一 2×2 EPSG:4326 GeoTIFF，对 `-s 1 -e 1` 的 `nearest`、`bilinear`、`average` 各生成 4 个 tile；Rust 与原版的全部 12 个 gzip 解压 payload 均以 `cmp` 逐字节一致，包含 child mask。`cargo test` 通过 40 项测试，`cargo clippy --all-targets -- -D warnings` 无告警。

本轮按领域设计优先执行，顺序固定为：

1. 建立不依赖 I/O 的领域模型、错误边界和 `RasterSource` 契约；
2. 用单元测试锁定 Global Geodetic 格网、瓦片范围、路径和 heightmap 二进制规格；
3. 仅在接口与测试策略稳定后，对纯 Rust GeoTIFF 解析库进行依赖 spike；
4. spike 结果写入本方案；若候选库无法满足首期所需能力，停止并请求技术决策，不擅自替换既定设计。

本轮不会修改已确定的首期产品边界（GeoTIFF、EPSG:4326、NoData 报错、heightmap 优先）。

## 11. 后续阶段路线图（2026-08-04）

### 已完成基线：P1a — 兼容 heightmap MVP

纯 Rust 的 EPSG:4326、north-up、单波段 GeoTIFF 到 CTB `heightmap-1.0` 已具备完整垂直链路。四个 CLI 可执行名均可用；`nearest`、`bilinear`、`average`，完整与受限 zoom 范围均已用本地原 CTB oracle 验证裸 payload 字节一致。该基线是后续每一阶段的不可回归条件。

### P1b — 兼容性矩阵与输入可靠性

目标：把当前单一最小 oracle 扩展为可复现的兼容性矩阵，先确认已声明支持的 GeoTIFF 行为，再扩展实现。

1. 固化带许可证、生成脚本和 checksum 的 fixture：整数/浮点、负高程、striped/tiled、DEFLATE、内部 overview、显式 NoData、损坏 TIFF、极点和 Antimeridian 边界。
2. 对 `nearest`、`bilinear`、`average` 的每个有效 fixture 运行原 CTB 与 Rust oracle；比较 tile 集合和解压 payload。NoData 与不支持元数据必须比较失败类型而非输出。
3. 补齐已支持 GeoTIFF feature 的真实性验证；BigTIFF、更多压缩和多 block 只在对应 fixture 已被 Rust 库实际读通后进入承诺范围。
4. 将 `ctb-info`、`ctb-export`、`ctb-extents` 的原版输出与错误路径加入进程级兼容测试。

完成标准：每个已宣布支持的输入特征至少有一份可复现 fixture 和 oracle；未支持 feature 的拒绝信息稳定、无 panic。

### P2 — 性能、大文件与可恢复写入

目标：在不改变 P1 字节输出的前提下，避免全图读取并支持大 DEM 的受控资源使用。

1. 将单像素 `RasterSource::read_window` 调用替换为带 halo 的块读取；引入有界 LRU 块缓存，缓存键包含 source、overview、band 与 block 坐标。
2. 依据输出 footprint 选择不比目标更粗且最接近的 internal overview，并与原始层输出做数值/字节回归。
3. 用受限工作队列实现并行 tile 写入，按确定顺序提交并保留原子 rename、`--resume` 语义。
4. 提供基准：峰值内存、吞吐、线程数和 fixture 尺寸；增加文件描述符、磁盘写入失败、损坏压缩流及取消/重跑测试。

完成标准：单线程与多线程解压 payload 一致；内存上限可配置；大 DEM 测试不会按整图尺寸线性占用内存。

### P3 — Quantized-Mesh 1.0

目标：复用既有 `TerrainSamplePlan`，增加独立的 Cesium Quantized-Mesh 输出，不影响 heightmap 格式和路径。

1. 先定义 `QuantizedMeshTile` 领域模型和严格 reader/writer：头部、量化顶点、规则网格三角形、high-water-mark index 编码及四边 edge index。
2. 用同一采样坐标保证相邻 tile 边界高度、顶点和 edge index 一致；第一版不做网格简化。
3. 生成 `layer.json`，在 `ctb-tile` 中以明确输出格式选择接入；补 `ctb-info --format` 检查能力。
4. 使用 terrain-server/Cesium 进行加载、层级切换和无裂缝 smoke test；skirt、法线、水面掩码和 metadata 扩展仅在核心格式稳定后迭代。

完成标准：同一 DEM 能输出 heightmap 或 Quantized-Mesh；独立 reader 与 Cesium 均能读取，跨 tile 边界无高度裂缝。

### P4 — CRS 与 Global Mercator

目标：以明确的登记式转换器扩展 EPSG:4326，而不引入 GDAL/PROJ FFI 或“任意 EPSG”承诺。

1. 引入纯 Rust CRS 转换边界及轴序策略（固定 GIS x/y），先实现并验证 EPSG:4326 <-> EPSG:3857。
2. 实现 `GlobalMercator` 格网和 `--profile mercator`，用控制点、世界边界、极区限制与原 CTB 对比锁定行为。
3. 只有在 datum、椭球、范围和精度阈值明确时，才按需求增加 UTM/WGS84 等投影。

完成标准：每个新增 CRS 都有正反变换精度、有效范围和拒绝行为测试；未知 CRS 从不被静默当作 WGS84。

### P5 — 格式生态与产品化

目标：在稳定的 `RasterSource` 边界上按需求增加输入和 I/O，而非复刻 GDAL 的任意驱动集合。

1. 依需求排序添加 HGT、ASCII Grid、DTED、ENVI 等原生 Rust 驱动，并为每个格式建立最小兼容矩阵。
2. 实现受限 mosaic/composite 描述；仅在存在明确互操作需求时评估 VRT XML reader。
3. 增加 COG HTTP Range I/O adapter、外部 overview 和可观测性；网络读取必须具备大小、超时和重试边界。
4. 引入 CI：MSRV、格式化、clippy、测试、依赖树/许可证/SBOM 检查；发布前提供性能报告与兼容性报告。

完成标准：每个驱动、I/O adapter 和新输出格式均不改变 P1 oracle；依赖树持续证明没有 GDAL、PROJ 或 C/C++ GIS FFI。

### 执行优先级

下一实施阶段为 P1b。P2 与 P3 可在 P1b 的 fixture/契约稳定后并行规划，但 Quantized-Mesh 的编码实现必须等待 P1b 的边界与测试资产完成。P4、P5 不得为当前实现引入预先的 FFI 或泛化依赖。

P1b 要点 1 验收：已建立 `tests/fixtures/MANIFEST.md`，为当前原 CTB oracle 输入记录了来源/许可、SHA-256、可执行的 GeoTIFF 生成命令、空间元数据与兼容性断言；`TEST_STRATEGY.md` 已将它设为权威 fixture 清单入口。后续 fixture 必须按同一字段录入，不能提交未声明来源或 checksum 的二进制 DEM。

### P1b 当前实施单元：可复现 CTB oracle 命令

将最小 `oracle-source-v1` fixture 的人工临时目录步骤收敛为仓库内脚本。脚本只用于开发者兼容性验收：通过环境变量接收原 CTB `ctb-tile` 可执行文件，在临时目录使用本地 `gdal_translate` 将 ASCII fixture 转为 GeoTIFF；随后以 `nearest`、`bilinear`、`average` 分别运行原版与 Rust `ctb-tile`，覆盖完整自动 zoom 与 `-s 1 -e 1` 受限范围，并逐个比较 gzip 解压后的 payload。它不是生产运行时依赖，也不进入没有 GDAL/原版 CTB 的常规 `cargo test`。

该单元的完成标准：调用方式、前置条件和失败信息稳定；12 个受限范围 payload 与全部自动范围 payload 均由 `cmp` 验证；临时文件无论成功或失败都会清理。

源码核对修正：原版 `ctb-tile` 虽解析 `-r/--resampling-method`，但 `Terrain` 路径构造 `TerrainTiler(poDataset, grid)` 时未传递 `TilerOptions`；因此 heightmap terrain 始终使用 `GDALTiler` 的默认 `GRA_Average`。该选项仅对原版的非 Terrain GDAL 输出生效。为字节兼容，纯 Rust heightmap CLI 必须保留并接受三个已声明值，但固定使用 `average`；`ResamplingMethod` 的三种领域实现仍保留，不再宣称其已通过 Terrain CLI 生效。

P1b oracle 命令验收：`scripts/verify-ctb-oracle.zsh` 已在本机 GDAL 与原 CTB executable 下通过。它对 `nearest`、`bilinear`、`average` 各执行自动 zoom 和 `-s 1 -e 1`，共六组原版/Rust 对比；所有 tile path 集合和解压 terrain payload 均一致。脚本结束时清理临时目录。常规 Rust 验收同步通过：40 tests、Clippy 无告警。

### P1b 当前实施单元：GeoTIFF 输入契约矩阵

以 Rust test runtime 生成小型 GeoTIFF，不提交二进制 DEM。扩展 `GeoTiffRasterSource` 的集成测试，覆盖 signed integer 的负高程、unsigned integer、`f32`、`f64`、显式 NoData 与截断 TIFF；每个成功 fixture 均断言其公开 `f64` 样本契约，每个失败 fixture 均断言结构化 `CtbError`。世界范围/Antimeridian 的格网语义继续保持在无 I/O 的 grid 测试中，不由 GeoTIFF decoder 暗自环绕坐标。

完成标准：被 decoder 声明支持的代表性数值类型均有测试；NoData 和损坏输入返回错误而非 panic；无需 GDAL 即可在 `cargo test` 中执行。

P1b 输入契约验收：runtime fixture 现覆盖 `f64`、`f32`、signed `i16`（含负高程）和 unsigned `u16`；这些值均经 `GeoTiffRasterSource` 读取并断言为公开的 `f64` 样本。带 `GDAL_NODATA` tag 的读取仍返回 `NoDataEncountered`，三字节截断 TIFF 返回 `RasterRead` 而无 panic。常规验收为 43 tests 与 Clippy 无告警。

### P1b 当前实施单元：压缩与内部 overview oracle

扩展开发期 oracle 脚本：从同一 ASCII source 生成基准 GeoTIFF 以及 `TILED=YES`、`COMPRESS=DEFLATE`、内部 average overview 的 GeoTIFF。对每种输入执行原版与 Rust terrain 输出比较；压缩/overview 源还须与基准源的 Rust payload 比较，从而验证解码路径不会改变采样结果。此单元只以当前已验证的单 block 2×2 fixture 作为能力回归，不据此承诺 BigTIFF、多 block 或外部 overview。

完成标准：脚本明确检查 `gdal_translate` 与 `gdaladdo` 前置条件；三种 `-r` 值、自动/受限 zoom，以及 plain/压缩 overview 源的所有 payload 均通过比较；临时文件保持清理。

P1b 压缩/overview 验收：oracle 脚本现会从 `oracle-source-v1` 派生 `TILED=YES`、`COMPRESS=DEFLATE`、内部 average overview GeoTIFF，并先比较原 CTB 与 Rust，再比较该 Rust 输出与 plain source。plain 与压缩 overview 两种输入下，`nearest`、`bilinear`、`average` 的自动和受限 zoom 共 12 组原版/Rust payload 已全部一致；6 组跨输入 Rust payload 亦一致。此结论仅覆盖本 fixture 的单 block 内部 overview 路径。
