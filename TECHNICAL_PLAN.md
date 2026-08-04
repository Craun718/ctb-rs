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

## 12. 全量原版 CTB 复刻计划（2026-08-04）

用户目标更新为覆盖原版 CTB 的全部公开 CLI 契约与核心行为。仍保持纯 Rust、禁止 GDAL/PROJ/C/C++ GIS FFI 的决策。原版的“任意 GDAL datasource / output driver”并非封闭的 CTB 格式，而是对安装时 GDAL 驱动集合的委托；纯 Rust 方案将以登记式 `RasterSource` / `RasterWriter` 驱动矩阵逐项覆盖其可观察接口，绝不把未知格式伪装成已支持。

实施顺序：

1. 完成 P1b：四个 CLI 的成功/错误输出矩阵、NoData 与损坏输入、可复现 oracle 资产。
2. P2：并行、缓存、overview 选择、`--thread-count`、`--quiet`、`--verbose`、`--resume` 的原版可观察行为。
3. P3：Quantized-Mesh 1.0、`layer.json` 及 Cesium 验收。
4. P4：EPSG:3857、Global Mercator、`--profile mercator` 与纯 Rust 4326/3857 转换。
5. P5：原版的格式接口矩阵；优先 GeoTIFF writer 以外的实际 CTB 常用输出，再按 HGT、ASCII Grid、DTED、ENVI、mosaic/受限 VRT、COG HTTP Range 交付。每个新 driver 都有输入/输出 oracle 与明确的 CLI `--output-format` 名称。
6. 最终兼容审计：列出原版每个参数、错误路径、格式与 profile 的状态；只在矩阵全绿时声明全量复刻完成。

每个子阶段结束后必须：核对其实施结果与本方案、更新 TODO、运行比例相称的验证、仅暂存该子阶段文件，再以 `commit-staged` 创建 Conventional Commit。

P1b 要点 1 验收：已建立 `tests/fixtures/MANIFEST.md`，为当前原 CTB oracle 输入记录了来源/许可、SHA-256、可执行的 GeoTIFF 生成命令、空间元数据与兼容性断言；`TEST_STRATEGY.md` 已将它设为权威 fixture 清单入口。后续 fixture 必须按同一字段录入，不能提交未声明来源或 checksum 的二进制 DEM。

### P1b 当前实施单元：可复现 CTB oracle 命令

将最小 `oracle-source-v1` fixture 的人工临时目录步骤收敛为仓库内脚本。脚本只用于开发者兼容性验收：通过环境变量接收原 CTB `ctb-tile` 可执行文件，在临时目录使用本地 `gdal_translate` 将 ASCII fixture 转为 GeoTIFF；随后以 `nearest`、`bilinear`、`average` 分别运行原版与 Rust `ctb-tile`，覆盖完整自动 zoom 与 `-s 1 -e 1` 受限范围，并逐个比较 gzip 解压后的 payload。它不是生产运行时依赖，也不进入没有 GDAL/原版 CTB 的常规 `cargo test`。

该单元的完成标准：调用方式、前置条件和失败信息稳定；12 个受限范围 payload 与全部自动范围 payload 均由 `cmp` 验证；临时文件无论成功或失败都会清理。

源码核对修正（已替换）：原版 `ctb-tile` 的 `TerrainBuild::setResampleAlg` 将 `-r/--resampling-method` 写入其 `tilerOptions`，并以 `TerrainTiler(poDataset, grid, tilerOptions)` 构造 heightmap 路径；terrain 会实际使用所选 GDAL algorithm。先前“固定 Average”的结论错误，Rust 不得再硬编码该值。

历史验收更正：此前 oracle 在 Rust 端硬编码 Average 时比较了原版多种 `-r` 值，不能证明 nearest/bilinear 的算法兼容；该结论已作废。后续 oracle 必须确认两端实际传入相同 algorithm。

### P1b 当前实施单元：GeoTIFF 输入契约矩阵

以 Rust test runtime 生成小型 GeoTIFF，不提交二进制 DEM。扩展 `GeoTiffRasterSource` 的集成测试，覆盖 signed integer 的负高程、unsigned integer、`f32`、`f64`、显式 NoData 与截断 TIFF；每个成功 fixture 均断言其公开 `f64` 样本契约，每个失败 fixture 均断言结构化 `CtbError`。世界范围/Antimeridian 的格网语义继续保持在无 I/O 的 grid 测试中，不由 GeoTIFF decoder 暗自环绕坐标。

完成标准：被 decoder 声明支持的代表性数值类型均有测试；NoData 和损坏输入返回错误而非 panic；无需 GDAL 即可在 `cargo test` 中执行。

P1b 输入契约验收：runtime fixture 现覆盖 `f64`、`f32`、signed `i16`（含负高程）和 unsigned `u16`；这些值均经 `GeoTiffRasterSource` 读取并断言为公开的 `f64` 样本。带 `GDAL_NODATA` tag 的读取仍返回 `NoDataEncountered`，三字节截断 TIFF 返回 `RasterRead` 而无 panic。常规验收为 43 tests 与 Clippy 无告警。

### P1b 当前实施单元：压缩与内部 overview oracle

扩展开发期 oracle 脚本：从同一 ASCII source 生成基准 GeoTIFF 以及 `TILED=YES`、`COMPRESS=DEFLATE`、内部 average overview 的 GeoTIFF。对每种输入执行原版与 Rust terrain 输出比较；压缩/overview 源还须与基准源的 Rust payload 比较，从而验证解码路径不会改变采样结果。此单元只以当前已验证的单 block 2×2 fixture 作为能力回归，不据此承诺 BigTIFF、多 block 或外部 overview。

完成标准：脚本明确检查 `gdal_translate` 与 `gdaladdo` 前置条件；三种 `-r` 值、自动/受限 zoom，以及 plain/压缩 overview 源的所有 payload 均通过比较；临时文件保持清理。

P1b 压缩/overview 验收：oracle 脚本现会从 `oracle-source-v1` 派生 `TILED=YES`、`COMPRESS=DEFLATE`、内部 average overview GeoTIFF，并先比较原 CTB 与 Rust，再比较该 Rust 输出与 plain source。plain 与压缩 overview 两种输入下，`nearest`、`bilinear`、`average` 的自动和受限 zoom 共 12 组原版/Rust payload 已全部一致；6 组跨输入 Rust payload 亦一致。此结论仅覆盖本 fixture 的单 block 内部 overview 路径。

### P1b 当前实施单元：Float32 负高程 oracle

由 plain source 通过 GDAL 生成 `Float32` 且重标定为负/正高程的临时 GeoTIFF。该输入对三种 CLI `-r` 值及自动/受限 zoom 与原 CTB 逐 payload 对比，用以同时验证原生 `f32 -> f64` 解码、高程 offset/scale 量化与负值路径；它不与 plain source 交叉比较，因为像元值刻意不同。

完成标准：6 组 Float32 原版/Rust payload 一致；manifest 记录派生命令和高度范围；常规 Rust 测试保持通过。

P1b Float32 验收：脚本以 `gdal_translate -ot Float32 -scale 100 400 -100 50` 生成样本范围为 -100 至 50 m 的输入。三种 `-r` 值与自动/受限 zoom 的 6 组原版/Rust payload 全部一致；结合已有 plain 和 tiled overview 路径，当前 oracle 共通过 18 组对比。

### P1b 当前实施单元：其余 CLI 的原版可观察契约

审计修正：`ctb-tile` 的 Terrain 输出固定 Average，故不存在“Terrain 的三算法 oracle 矩阵”这一原版功能；该 TODO 改为已完成的“接受原版列举值且固定 Average”契约。`ctb-extents` 的原版实现以 15 位科学计数法写 GeoJSON，包含空格和换行，并要求 `-o` 指向已存在目录（文件无法打开时仅报告并停止写入）。当前 Rust extents 自动创建目录、使用紧凑 JSON，与原版不一致；本单元应先锁定成功输出和目录失败行为，再将 writer 改为原版格式。`ctb-info` 和 `ctb-export` 也必须以原版的成功 stdout、参数缺失和无效 terrain 输入路径作为进程级契约，而不是只验证退出成功。

完成标准：四个 CLI 均有对应原版命令的成功/失败进程测试；`ctb-extents` 对同一输入产生可比较的原版 GeoJSON 文本；不新增原版未提供的参数、目录创建或输出格式。

浮点验收说明：CTB 的 `Grid::pixelsToCrs` 在其本机构建中可能因 C++ 编译器中间精度而将逻辑零边界写成约 `10^-15`；纯 Rust 的确定性 IEEE-754 `f64` 路径写为 `0`。该差异不对应源代码中的不同算法、tile 或几何边界，不能作为跨语言的字节文本契约。验收固定为同一 `Grid` 像素坐标算式、feature 遍历顺序、15 位科学计数法、空格/换行结构，以及数值在一个 `f64` ULP 内相同；terrain 二进制 payload 继续维持逐字节要求。

原版错误路径补充：`ctb-info` 读取 terrain 失败时打印 `Error:` 并返回 1。`ctb-export` 捕获同一读取错误后仍继续构造并导出默认的零 heightmap，同时打印 `Creating …` 并返回 0；Rust 必须以显式的全零 `HeightmapTerrain` 表示该输出，禁止复刻 C++ 未初始化内存的偶然内容。

P1b 阶段验收（2026-08-04）：常规 Rust 验收为 46 tests、`cargo clippy --all-targets -- -D warnings` 无告警。原版 CTB oracle 脚本共比较 plain、Float32 负/正高程、tiled/DEFLATE/internal overview 三种输入，三种原版列举 `-r` 值及自动/受限 zoom 共 18 组，所有 terrain 路径与 gzip 解压 payload 一致。额外以 GDAL 临时生成的 BigTIFF 和 512×512、128×128 block、LZW GeoTIFF 在 z=0 与原版逐 payload 一致；它们现列为已验证读取路径。JPEG、LERC、ZSTD、外部 overview 和更大 BigTIFF 仍未验证，不能宣称支持。

P1b 后原版功能审计：本阶段没有保留原版不存在的用户可见 CLI 参数或输出格式；移除了 Rust 自动创建 extents 输出目录这一额外行为。当前仍缺失且已按原版归入后续阶段的能力为：`ctb-tile` 的 `mercator` profile、并发/进度参数和任意 GDAL 输出格式；`ctb-extents` 的 mercator profile；以及原版 GDAL 可读取的非 GeoTIFF datasource。Quantized-Mesh 不是原版 CTB 功能，保留为独立产品扩展，不计入原版复刻完成度。下一大阶段转入 P2，首先复刻原版 `ctb-tile` 的并发、进度与 resume 可观察行为；不得提前接入未登记格式或 CRS。

## 13. P2 当前实施单元：`ctb-tile` 并发与进度契约

原版 `ctb-tile` 将 `-c/--thread-count` 的正值作为 worker 数，非正值取 CPU 数；每个 worker 独立打开 GDAL 数据集，并通过受 mutex 保护的全局 iterator 领取下一个 tile。`-q` 使用 GDAL dummy progress，不输出正常进度；`-v` 在每个 tile 完成时向 stdout 写 `[NN%] created PATH in thread ID`。默认 GDAL progress meter 不是稳定的文本接口，Rust 不模拟其控制字符，但默认模式不得额外输出 tile 行；最终汇总行保留现有 Rust CLI 契约。

Rust 需保持 tile payload、child mask、临时文件 rename 与 `--resume` 语义不变。`RasterSource` 的共享写入 API 可用于库调用；但 CLI 必须接收 source factory，并在每个 worker 内重新打开 `GeoTiffRasterSource`，复刻原版独立 dataset 的资源边界。任务分派使用单一有序队列，结果写入顺序可并发但 payload 必须与单线程相同。线程数为零或负数时 CLI 保持原版“使用 CPU 数”语义；不新增用户可见的 Rust 专有并行参数，也移除当前 Rust 额外的完成汇总行。

补充审计：原版在启动 worker 前验证 `--output-dir` 已存在且为目录；Rust 现有 writer 的 `create_dir_all` 仅能创建 `{z}/{x}` 子目录，CLI 不得借此自动创建根输出目录。P2 将在 CLI 入口拒绝不存在或非目录的根路径，并让进程测试显式建目录。

P2.1 验收：`ctb-tile` 现接受原版 `-c/-q/-v`；worker 的 source factory 先打开一次读取元数据，再为每个 worker 独立打开 GeoTIFF。单元测试确认双 worker 会执行三次 factory 调用（元数据 + 两 worker）。本地原版 CTB 与 Rust 对 `-c 1/-c 2` 的 z=1 输出逐 gzip 解压 payload 一致；quiet stdout 均为空，verbose 共享 `[NN%] created PATH in thread ID` 结构。原版 C++ 指针式 thread ID 与 Rust `ThreadId` 的具体表示不同，属于运行时标识而非格式契约。根输出目录不存在时 Rust 已改为拒绝，删除此前额外的自动创建行为。

### P2 当前实施单元：窗口块读取与有界缓存

原版 CTB 通过 GDAL warp/VRT 按 tile 读取，不会为每个目标样点执行一次独立文件窗口请求。Rust 的 `sample_at` 目前以 1×1 `read_window` 取样，虽结果正确但 I/O 粒度不符合原版路径。该单元在既有 `RasterSource::read_window` 契约上增加内部窗口块 cache：键为 source identity、overview、窗口块坐标；缺块时读取一个固定边长的 source window，双线性/average 所需 halo 必须被包含。缓存容量可配置且严格有界；它是实现细节，不能新增 CLI 参数。

NoData 边界：当前公开契约要求“参与输出的 NoData”报错，而 `read_window` 会在窗口中任意 NoData 时失败。为不扩大失败范围，声明 `RasterMetadata::no_data` 的 source 不执行宽窗口预取，保持精确 1×1 读取；无 NoData source 才使用块 cache。将来只有在 `RasterWindow` 增加逐像元 validity mask 并完成独立兼容测试后，才允许对 NoData source 预取。

完成标准：缓存开启/关闭的 heightmap payload 完全一致；对内存 test source 可断言读窗口次数下降；capacity 达到上限时按 LRU 驱逐；并发调用不返回错误或 panic。真实 GeoTIFF oracle 必须继续逐 payload 一致。

P2.2 验收：`CachedRasterSource` 已以固定 64×64 block、64 项 LRU 容量接入 CLI factory；相邻像元命中同一块、容量一项时正确驱逐、声明 NoData 时保留精确读取，均由单元测试覆盖。`cargo test` 通过 53 项，Clippy 无告警；启用缓存后再次运行 18 组原版 CTB oracle，所有解压 terrain payload 仍逐字节一致。

### P2 技术分歧：overview 几何层级（等待确认）

原版 `GDALTiler::createRasterTile` 在保持 base dataset 的 tiling 元数据后，对每个 tile 调用 `getOverviewDataset`；它以 `GDALSuggestedWarpOutput2` 的 target ratio 选择内部 overview，再重建 image-to-image transform。因此 base 分辨率继续决定 max zoom、tile range 与 child mask，而 overview 只决定该 tile 的 sampling source transform 和像素尺寸。

当前 Rust `RasterSource` 只有 `metadata()`，同时承担 tileset 规划与采样坐标映射，无法无损表达上述两层几何。将 metadata 替换为 overview 会错误改变 max zoom；在 adapter 内把 base window 隐式缩放为 overview window 则会让 nearest/bilinear/average 使用错误的像元中心与 footprint。该问题不能通过局部条件分支正确解决。

待确认的最小接口演进：保留 `RasterSource::metadata()` 作为 base planning metadata，并新增按 overview 返回的 `SamplingLevel { metadata, level }` 与 `read_sampling_window(level, request)`；`TerrainSamplePlan`/采样器在每个 tile 先请求原版 ratio 规则选定的 level，再只在该 level 的 metadata 中计算坐标。这样不改变 CLI 或输出格式，并直接对应原版 base dataset + overview dataset 双对象模型。确认前不实现 overview 自动选择，也不宣称 P2 完成。

产品确认（2026-08-04）：采用上述最小接口演进。实现必须让 base planning metadata、tile range、max zoom 和 child mask 保持 base dataset 语义；overview 只能作为每 tile 的 sampling source。先以不重投影的 EPSG:4326 north-up 情形复刻 `getOverviewDataset` 的 ratio 选择，再以原 CTB 的内部 overview fixture 比较 payload。

落地顺序：先将 `SamplingLevel` 加入领域接口，默认实现只暴露 base level，以避免改变既有内存 source 的行为；随后 GeoTIFF adapter 为内部 overview 构造保持同一左上原点、按宽高缩放像元尺寸的采样 metadata。`TerrainSamplePlan` 每 tile 使用 base GeoTransform 的 `1 / pixel_width` 作为无重投影时 `GDALSuggestedWarpOutput2` ratio 的等价输入，并按原版的相邻 overview 阈值循环选择 level；nearest、bilinear 与 average 都只能使用被选 level 的 metadata 和窗口。缓存键继续含 level，block 尺寸从该 level 的 metadata 取得。最后以现有 internal overview oracle fixture 检查 terrain 解压 payload。

实施诊断（2026-08-04）：接口与 adapter/read path 已能编译，原有 18 组 oracle 继续通过；但为强制触发 overview 选择而生成的高分辨率输入，暴露出既有 Rust `Grid` 对数据集精确边界的 tile range 与原版 CTB 不同，且在避开全球边界的输入中仍未完成三种 resampling 的全量 payload 对照。因此“`1 / base pixel_width` 等价于 GDALSuggestedWarpOutput2”的假设尚未被 oracle 证实；不得将 overview 单元标记完成或暂存。下一步必须先从 GDAL 的 `GDALSuggestedWarpOutput2` / GenImgProj transformer 提取可复现的 target ratio 和 overview VRT GeoTransform oracle，再据该 oracle 修正或否定该纯 Rust 等价式。

恢复执行（2026-08-04）：先用最小独立 C++ oracle 对同一 GeoTIFF 调用 CTB 所用的 `GDALCreateGenImgProjTransformer2`、`GDALSuggestedWarpOutput2` 与 `GDALCreateOverviewDataset`，记录 suggested GeoTransform、ratio、selected overview 和 overview dataset GeoTransform；该程序仅写入临时目录，不进入产品依赖。随后把已验证的数值关系转成 Rust 领域测试，再运行原版 CTB 的高分辨率内部 overview payload 对照。仅当路径集合与 payload 都一致，才关闭 P2 overview todo 并执行暂存/`$commit-staged`。

GDAL oracle 结论：720×360、bounds `[-179.9,-89.9,179.9,89.9]`、单层 2× internal overview 的 `GDALSuggestedWarpOutput2` 输出 pixel width 为 `0.49972222222222223`，故 target ratio 为 `2.0011117287381879`，CTB 选 overview 0；其 overview GeoTransform 的像元尺寸正好按 base 宽高同比放大。此前路径集合差异不是 ratio 假设失效，而是 Rust `tile_range_for_area` 额外使用 `strict_upper_index` 排除了上/右边界；原版 `Grid::crsToTile` 对 lower-left 与 upper-right 都直接经 `i_pixel` 截断再除 tile size。先移除该额外排除规则并添加边界回归，然后继续 overview payload oracle。

边界补充：CTB 的 `crsToPixels` / `pixelsToTile` 不把世界上/右边界 clamp 到名义 extent 内；精确 `180` 或 `90` 可形成 z=0 的 `x=2` 或 `y=1`，并由 iterator 生成其 tile。Rust 的 `index_at` clamp 和 `validate_tile` 的 `< count` 同样是原版不存在的限制，必须改为直接 `floor` / 整数截断及允许 `index == count`。这是一项原版兼容修复，不改变 Grid 算法或产品接口。

核对状态：修正 Grid 后，既有 18 组 oracle 以及高分辨率输入的 tile 路径集合均与 CTB 相同；GDAL oracle 也确认 Rust 选择了 CTB 所选的 overview 0。高分辨率 overview 的首个 terrain payload 仍不一致，故问题已收敛为 sampling/warp 数值，而非规划、ratio 或 overview GeoTransform。下一轮必须把同一高分辨率 DEM 的无 overview 与有 overview 分开完成 nearest/bilinear/average 逐 tile 比较；若无 overview 已不一致，先为高采样倍率补齐 CTB VRT sampling oracle；若只在 overview 不一致，则记录 GDAL overview band 的实际样值和 VRT 采样坐标后修正 level-aware sampler。

VRT sampling oracle：同一 DEM 的无 overview 基线逐 payload 一致；overview IFD 的 Rust/GDAL 窗口值也一致。差异从 terrain raw offset 132（第 66 个 `u16`，即第 2 行第 1 列）开始：CTB 仍取 overview row 89，Rust 取 row 90。结合 `TerrainTiler::terrainTileBounds`（west/north 扩展一个 resolution）与 VRT 的 RasterIO 坐标，结论是当前 Rust 的 `row - 0.5` / `column - 0.5` centre 解释不符合原版实际采样；应改为以 VRT overlapped destination pixel corner 计算，`world_x = tile.min_x + cell_width * (column - 1)`、`world_y = tile.max_y + cell_height * (1 - row)`，footprint 也相应以该 corner 为起点。该修正必须同时通过高分辨率 base 与 overview oracle，不能只以原有低分辨率 fixture 验收。

反证更新：corner 版本保持高分辨率 base payload 一致，却未改变 overview payload 差异，故该坐标解释不构成已验证修复，已从代码撤回。下一步不再改变 sampling 几何；先在 `write_heightmap_tileset_with_factory` 的实际 worker 路径上加入无 I/O test source，断言 target ratio、返回 level 和 `read_sampling_window` 所接收 level。若 worker 确实读取 level 1，再以 CTB/GDAL dataset 的 overview 生命周期（内部 IFD、external `.ovr`、相关 metadata）逐项对照。

本轮执行：先以临时调试程序直接调用与 writer 相同的 `TerrainSamplePlan::sample_heights`，读取第 66 个样本并与写出的 terrain 第 66 个值比较；该观察优先于继续修改接口或采样数学。随后将确定的 level 传递约束写为常规无 I/O 单元测试。

第 66 样本测量：实际 worker 路径确实选择 level 1，`sample_heights[66]` 为 300，并被量化为最终 terrain 的 6500；CTB 同位为 100 / 5500。Rust overview adapter 的 row 90 值是 300，row 89 值是 100。将 x、y 同时改为 corner 会因 x=`-180` 落到 source 之外而错误取 0；保持现有 x centre `-179.296875`、仅将 y 调为 north-overlap 边缘 `0` 时正好取 CTB 的 row 89。下一次实现只调整 y 轴采样坐标/footprint，上述 x 轴保持不变，并重新执行高分辨率 base、overview 对照。

审计更正：原版 `ctb-tile.cpp` 的 `setResampleAlg` 会将 `-r` 的 GDAL enum 传入 `TilerOptions`；Rust CLI 当前将字段命名为 `_resampling_method` 并硬编码 `Average`，这是原版兼容缺失。此前以原版 `nearest` 对 Rust 作 overview 比较无效，因为两端并未使用同一算法。撤回以该比较作出的 overview sampling 结论和 y-axis 改动；先以双方 `Average` 验证 overview，再单列实现 `nearest/bilinear/cubic/cubicspline/lanczos/average/mode/max/min/med/q1/q3` 的原版 CLI 与 GDAL resampling 语义。

VRT oracle 进展：直接链接原版 `libctb`、调用 `TerrainTiler::createRasterTile(1,0,0)` 后 `RasterIO` 得到 `values[65]=0, values[66]=100`，与原版 terrain 相符。手工复刻 `getOverviewDataset`、overview GeoTransform、destination GeoTransform、`GRA_Average` 与 `NUM_THREADS=ALL_CPUS` 的纯 GDAL VRT 却得到 `values[66]=300`。故差异不在 Rust adapter/level 传递，也不能以简单 world-coordinate 采样修复；下一步必须对比两份 VRT 完整 XML（transformer 参数、source window、nodata/density）并将可观察的隐含 GDAL状态写成 oracle，再设计纯 Rust 等价物。

完整 VRT XML 对比：native CTB 额外使用 `ApproxTransformer`，来源是 `TilerOptions::errorThreshold = 0.125`（默认 gdalwarp 值）；Rust 当前没有该层。手工 VRT 加入 `GDALCreateApproxTransformer(..., 0.125)`，以及 CTB 在 `TerrainTiler` 中对 VRT public GeoTransform 的后设操作后，`values[66]` 仍为 300。故 ApproxTransformer 是原版必须复刻的配置，但不是该输入上 100/300 差异的充分解释；在完成 native VRT 内部 transformer/source-window 状态审计前，不引入未经验证的 Rust approximate transform。

补充排除：手工 oracle 也执行了 CTB 在 `GDALCreateWarpedVRT` 后的 `GDALSetProjection`，结果仍为 300。至此已复现 `createRasterTile` 中所有可从公开 C++ 代码直接观察到的调用及其 XML 状态，却无法复现 native `TerrainTiler` VRT 的 100；该内部 GDAL 生命周期差异不能安全外推为 Rust 算法。停止在此处继续试错，overview 单元保持未完成；并行恢复原版 `-r` 的参数/算法兼容审计，避免阻塞其他登记功能。

`-r` 恢复单元：原版 `ctb-tile` 接受 `nearest; bilinear; cubic; cubicspline; lanczos; average; mode; max; min; med; q1; q3`，`TerrainBuild::setResampleAlg` 将每个值对应到 GDAL enum 并写入 `TilerOptions::resampleAlg`。Rust 必须先使 CLI 枚举完整且将选择传入 writer；然后以 GDAL/CTB 生成的小型非平坦 fixture 分别记录各算法 payload。实现顺序按 GDAL 的算法族：nearest、bilinear、cubic/cubicspline/lanczos 插值族；average；mode/min/max/med/q1/q3 聚合族。不得把未知值静默映射为 Average，也不得仅为通过 parsing test 宣称算法支持。

接口完成状态：Rust 的 `ResamplingMethod`、Clap `ResamplingArgument` 和 `HeightmapTilesetOptions` 现可表达并传递全部 12 个原版名称；单元测试遍历全部名称。已有 nearest/bilinear/average 路径维持实现，其余九种当前返回明确 `UnsupportedRaster`，不计为算法实现或 CLI 完成。后续每添加一族必须将该错误替换为经 oracle 验证的计算，而不是回退为 Average。

下一算法单元（max/min）：先从 `gdalwarpkernel.cpp` 的 `GRA_Max`、`GRA_Min` 分支记录 source-window 计算、NoData 跳过和 extrema 取值规则；用原版 CTB 及非平坦 GeoTIFF 将若干 destination samples/payload 固化为 oracle。Rust 仅在该 oracle 覆盖的 north-up EPSG:4326 无重投影路径实现同样的 footprint extrema，随后比较完整 terrain payload；mode/quantile 仍独立等待其 GDAL 规则。

max/min 当前证据：GDAL `GRA_Max/GRA_Min` 在 source window 内跳过 invalid/NoData，取全部有效样本的 strict extrema；Rust 以同一 level metadata 的 footprint 覆盖像元遍历实现，空 source window 返回 CTB destination 初始化值 0。非平坦 plain `oracle-source.asc` 的原版 CTB 与 Rust 自动 zoom 全部 terrain 路径和 gzip 解压 payload 均一致，领域测试覆盖完整 2×2 footprint extrema。扩展 oracle 改为真实传递 nearest/bilinear 后在首个 plain nearest 失败，因而 max/min 的 Float32/tiled/overview 扩展验收尚未完成，不能标记整个算法单元完成。

nearest 诊断单元：以直接链接原版 `TerrainTiler::createRasterTile` 的 VRT `RasterIO` 输出为准，选择与 Rust 首个不同 terrain index 对齐；同时记录 target world coordinate、source pixel coordinate和量化前 `f32` 值。随后只修正可由该 oracle 证明的 nearest round/floor、边界或 default-destination 行为，再跑 plain payload；禁止以 Average 结果推断 nearest。

nearest 边界 oracle：plain z0 tile `(0,0,0)` 的 raw index 2144（row 32, col 64）为原版 100 / Rust 0；对应 Rust world x 为 `-1.40625`，source bounds 为 `[-1,1]`、pixel width 1。原版 native VRT 输出 100，表明 GDAL nearest 接受 source 边缘外半个像元的 support 并 clamp 到 edge pixel；Rust 目前在 `sample_at_level` 的 strict dataset bounds 检查中过早返回 0。修复限定于 nearest：将可采样 bounds 向四周扩展半个 source pixel，再保留现有 `floor` 和 clamped index；超出该 support 的点仍按 CTB INIT_DEST=0 输出。

nearest plain 验收：上述半像元 support 修正后，plain 非平坦 fixture 的原版/Rust 自动 zoom tile path 和全部 gzip 解压 payload 一致；领域测试覆盖 support 内 clamp 与 support 外 0。nearest 尚未完成 Float32、tiled/internal-overview 和受限 zoom 的扩展验收。

bilinear 诊断单元：扩展 oracle 的 plain bilinear 首先在 tile `(0,0,0)` 失败。以同一 native `TerrainTiler` VRT 的首个 raw mismatch 对齐，分别记录 GDAL source support 边界、四邻域缺失时的权重归一化/edge clamp 行为；仅在 oracle 证明后调整 bilinear 的 bounds 或 kernel。

bilinear 边界 oracle：首个 mismatch 同为 `(0,0,0)` raw index 2144，CTB native VRT 为 100、Rust 为 0，world x 同为 `-1.40625`。在该点，半像元 support 后 Rust bilinear 的两个 x neighbour 都被既有 clamp 收敛到 edge pixel，得到 oracle 的 100；修复限定为对 bilinear 采用半 source pixel support，插值权重和 neighbour clamp 保持不变。

bilinear 反证：将 nearest 的半像元 support 应用于 bilinear 后虽消除了 index 2144，却在 raw index 63 产生更早的不一致，表明 GDAL bilinear 的边缘核并非简单的 support 扩展加 neighbour clamp。该探索性修改已撤回；bilinear 继续使用严格 bounds，待以 native VRT 中多个 edge/interior 控制点建立完整的 source-coordinate/weight oracle 后再实现。

bilinear 控制点方法：不再以 terrain 的单一压缩-byte 差异推断算法。临时 native CTB oracle 将输出指定 tile 全部 65×65 VRT float values；Rust 侧以相同 `TerrainSamplePlan` 坐标记录 sample 值、source coordinate 和四邻域。比较至少覆盖 source 外、半像元 support、source corner、水平/垂直边和内部位置；只有这些点形成一致的 kernel/edge 规则后才再次修改代码。

最终源码复核（优先于本节此前相反的诊断）：`tools/ctb-tile.cpp` 在 Terrain output 分支调用 `TerrainTiler(poDataset, *grid)`，没有传入 `command->tilerOptions`；只有非 Terrain GDAL output 分支调用 `RasterTiler(poDataset, *grid, command->tilerOptions)`。因此 Terrain heightmap 的 `-r` 仅解析兼容，实际恒为默认 `GRA_Average`。撤销本节中将 nearest/bilinear/max/min 与 Terrain payload 视为原版功能缺失的判断：Rust Terrain CLI 必须恢复固定 Average，同时保留 12 个名称解析。各算法的实现和 oracle 移至未来 RasterTiler output formats 阶段。

Terrain `-r` 验收（2026-08-04）：Rust 现接受全部 12 个原版名称，heightmap writer 固定 Average。`verify-ctb-oracle.zsh` 对 plain、Float32 和 tiled/internal-overview 三种输入下的 `nearest/bilinear/average/max/min`、自动/受限 zoom 共 30 组比较均通过；这证明 Terrain 分支的选择忽略与原版一致，不证明未来 RasterTiler formats 的各算法实现。常规 Rust 验收为 56 tests 与 Clippy 无告警。

下一原版功能审计：进入 `ctb-tile` 的 `--output-format` 非 Terrain 分支。先从 `tools/ctb-tile.cpp`、`RasterTiler` 和 GDAL driver metadata 建立“原版 format 名称 → creation option → 文件名/扩展名 → RasterIO data type/GeoTransform”矩阵；再选择首个能由现有纯 Rust GeoTIFF writer 复刻的原版 format 作为实现单元。此审计不改变当前 Terrain CLI，也不擅自把 GDAL driver 名称暴露为未实现参数。

GTiff 首单元：以原版 `ctb-tile -f GTiff` 的 plain EPSG:4326 source 为 oracle，记录 `{z}/{x}/{y}.tif` 文件集合、band count/type、size、GeoTransform、projection、NoData、压缩/creation option 传递和裸样本。Rust 仅在这些契约被确认后增加 `-f GTiff`，复用现有纯 Rust GeoTIFF writer；其他任意 GDAL driver 名称在未实现前必须明确拒绝，不能伪装为通用 GDAL 输出。

GTiff oracle 观察：plain source 的 z0 `(0,0,0)` 输出为 `{z}/{x}/{y}.tif`、65×65、单 band `Int32`、NoData `-9999`、EPSG:4326、GeoTransform `[-180, 180/65, 0, 90, 0, -180/65]`，并由 GTiff driver default strip layout 写出。RasterTiler CreateCopy 继承 VRT/source storage type；现有 Rust `RasterSource` 将样本归一为 `f64`，不足以决定目标 TIFF storage type。GTiff 实现前需在 `RasterMetadata` 增加内部 `RasterSampleType`（signed/unsigned/float 与 bits），GeoTiff adapter 从 base IFD 的 `SampleFormat` + `BitsPerSample` 填充，sampling 继续使用 `f64`，writer 根据 metadata 选择对应纯 Rust typed encode。该类型信息不新增 CLI，而是复刻 GDAL VRT/CreateCopy 数据模型。

GTiff storage-type 实施状态（2026-08-04）：`RasterMetadata` 已增加内部 `RasterSampleType`，`GeoTiffRasterSource::open` 从 base IFD 的 `SampleFormat` 与 `BitsPerSample` 确定 8/16/32-bit signed/unsigned 及 32/64-bit float。GeoTIFF adapter 继续向采样层暴露 `f64`；新增 Float64、Float32、Signed16、Unsigned16 fixture 断言，以防 writer 前的数据类型信息丢失。多波段编码不一致及超出上述类型组合的 TIFF 继续明确拒绝；typed GTiff 写出尚未开始。

RasterTiler 领域设计：原版 `GDALTiler::createRasterTile(coord)` 为 `Grid::tileBounds(coord)` 创建 north-up VRT GeoTransform `{min_x, resolution, 0, max_y, 0, -resolution}`，尺寸严格等于 `grid.tileSize()`；`RasterTiler` 不修改此 VRT。Rust 因而先定义独立的 `RasterTileSamplePlan`：每个 destination cell 使用中心 `(min_x + (column + 0.5) * resolution, max_y - (row + 0.5) * resolution)`，footprint 是该 cell 的完整 corner bounds。它与 65×65 heightmap 的 edge-overlap `TerrainSamplePlan` 不共用，且仅调用已有按 footprint 的 resampling 边界。首期只建立 plan 与 north-up/行列顺序/相邻 tile GeoTransform 测试；在 GTiff 样本 oracle 明确前，不得假定 `f64 → source type` 的 GDAL saturate/round 行为，也不得写出文件。

RasterTiler 选项边界：非 Terrain 原版将 `-r` 传给 `TilerOptions::resampleAlg`；`-t` 决定 Grid tile size；`-n` 原样交给 GDAL driver；`-p` 仍由 Grid/profile 决定。纯 Rust 的首个 GTiff 单元会只接受已被 writer 明确复刻的 creation options，其他 `-n` 值报不支持而非静默忽略；待实测 GTiff 默认与 `COMPRESS` 等选项矩阵后扩展。`-c/-q/-v/-R`、源独立打开、原子 `.tmp` rename 和路径顺序复用现有 worker/writer 契约。

RasterTiler sampling-plan 实施状态（2026-08-04）：已实现 `RasterTileSamplePlan`，它保存 tile bounds/resolution，并为每个 row-major destination cell 输出 pixel centre 与完整 footprint。单元测试使用 4×4 geodetic grid 固化 `(0,0)` VRT 的顶左/底右中心、north-up footprint 和横向相邻 tile 的连续边界；它没有接入 CLI、采样文件写出或整数转换。常规验证为 58 tests 与 Clippy 无告警。

GTiff oracle 环境状态（2026-08-04）：为复现原版，已在 `/private/tmp/ctb-original-build` 以 CTB 源码配置 CMake。现代 CMake 需 `-DCMAKE_POLICY_VERSION_MINIMUM=3.5`；通过该兼容设置后，CTB 自带的旧 `FindGDAL` 在本机 GDAL 3.13.2 上将 `GDALOpenEx` 错判为不可用并中止配置。该临时构建目录不属于仓库，未改动 CTB 源码或 Rust 依赖。后续 GTiff 样本/creation-option oracle 必须使用可工作的原版 `ctb-tile` 可执行文件；在此之前只推进不依赖该 oracle 的领域单元，不实现未验证的 `-f GTiff`。

GTiff oracle 恢复（2026-08-04）：在 `/private/tmp/ctb-gdal313-oracle` 的 CTB 源码临时副本上，仅适配 GDAL 3.13 的 `GDALOverviewDataset::GetGeoTransform`、`GetMetadata` C++ 虚方法签名后成功构建 `ctb-tile`；原始 CTB 工作树未修改。以 `oracle-source.asc → Int32 EPSG:4326 GeoTIFF` 运行 `ctb-tile -q -f GTiff`，得到 8 个 `{z}/{x}/{y}.tif` 文件。z0 `(0,0,0)` 为 65×65、`Int32`、NoData `-9999`、EPSG:4326、GeoTransform `[-180, 180/65, 0, 90, 0, -180/65]`、block `65×31`。这确认源码 main 的实际 geodetic default tile size 始终为 65；其 help 中“other GDAL formats 256”的文字与实现不符，不得据此改变 Rust geodetic 默认值。`-t 4` oracle 确认 VRT/GTiff 变为 4×4、`[-180,45,0,90,0,-45]`，并在 source 覆盖的目标 cell 写出 Average 值、其他 cell 保留 0。

Typed GTiff writer 实施状态（2026-08-04）：已新增纯 Rust `write_raster_tile_as_geotiff` 文件边界。它接收 `RasterTileSamplePlan`、metadata 与 row-major `f64` samples，使用 `RasterSampleType` 选择 GeoTIFF typed encoder，写入 EPSG:4326、north-up transform、NoData 和无压缩默认 layout。Signed32 的 4×4 unit oracle 覆盖类型、样本、NoData 与 GeoTransform。由于原版对 integer VRT 的 fractional result 的写入 round/saturate 规则和 GTiff creation options 尚未固定，整数 writer 只接受有限的整值和范围内值；float 输出接受有限且可表示的值。这是明确的领域边界，尚未接入 `ctb-tile -f GTiff`，也不得将该拒绝当作最终行为。常规验证为 60 tests 与 Clippy 无告警。

Integer conversion oracle（2026-08-04）：将 2×2 source 放入 z0/tile-size 4 的单一 destination cell，令 Int32 值为 `100,101,102,103`，Average 为 `101.5`，原版 GTiff 写出 `102`；负值 `-100,-99,-98,-97` 的 Average `-98.5` 写出 `-98`。这与本地 GDAL `gdalwarpkernel.cpp::ClampRoundAndAvoidNoData<T>` 一致：integer destination 先 clamp 到 `lowest..max`，再以 `floor(value + 0.5)` 转换（unsigned 的非负范围具有同样结果）。因此 typed writer 应将此前“fractional 明确拒绝”替换为该 clamp/round 规则；NaN/Infinity 没有 source oracle，仍明确拒绝。

GTiff 接入设计：新增独立 `RasterTilesetOptions` 与 `write_raster_geotiff_tileset_with_factory`，复用 `TilesetPlan` 的 zoom/path traversal、独立 source-per-worker、`-R`、first-error 和 progress 契约，但不生成 child mask 或 terrain gzip。每个 tile 创建 `RasterTileSamplePlan`，按非 Terrain `-r` 传入的 resampling 采样，随后以 typed writer 原子写入 `{z}/{x}/{y}.tif`。CLI 仅对精确名称 `GTiff` 走此路径；`Terrain` 保持原版的固定 Average 分支，其他 GDAL driver 名称继续明确拒绝。首期 `-n` 只接受空列表；其后由已记录的 COMPRESS oracle 增量开启，不得忽略或伪造任何 creation option。

GTiff CLI baseline（2026-08-04）：`ctb-tile -f GTiff` 已接入上述 writer；进程测试覆盖 `-t 4`、路径、尺寸、EPSG:4326 和 GeoTransform。对同一 Int32 `oracle-source.tif`，原版与 Rust 输出 10 个 `.tif` 相对路径完全一致，z0 tile 的 `gdal_translate -of XYZ` 样本矩阵一致；两者裸 TIFF SHA-256 不同，原因是独立 writer 的 IFD/strip 容器布局，非语义不兼容。原版 `-n COMPRESS=DEFLATE` 保持 65×31 strips、仅将 image structure 标记为 DEFLATE；`-n TILED=YES` 在本机 GDAL 3.13 输出 256×256 COG layout，不可由普通 writer 冒充。下一增量只支持精确 `COMPRESS=DEFLATE`（及默认/`COMPRESS=NONE`），其他 option 明确拒绝。

GTiff DEFLATE 实施状态（2026-08-04）：typed writer 与 RasterTileset options 现支持 `COMPRESS=DEFLATE`，默认及 `COMPRESS=NONE` 使用无压缩；未知 `-n` 值仍返回错误。进程级测试确认 DEFLATE 分支成功写出 tile。`TILED=YES`、PREDICTOR、block size、BigTIFF 等未实现，不得作为已兼容的 GDAL creation options 宣传。常规验证为 61 tests 与 Clippy 无告警。

RasterTiler nearest 边界 oracle（2026-08-04）：以 plain Int32 source 的 z0 GTiff 对照，原版在 world `(-1.384615384615389, 0)` 输出 0，而 Rust 输出 100。此前 terrain 调试为 `sample_at_level(Nearest)` 加入的半 source-pixel support 会将该 RasterTiler destination point clamp 到 source 边缘；原版 non-Terrain VRT 不采用此规则。故 `RasterTileSamplePlan` 不得继续调用 terrain-oriented `sample_with_footprint` 的 nearest 分支：需引入 RasterTiler 严格 dataset bounds 采样入口，nearest/bilinear 在 source bounds 外返回 CTB initial destination 0，而 terrain 的既有 helper 保持不变，待其各自 oracle 证明。

RasterTiler strict-boundary 实施状态（2026-08-04）：新增 `sample_with_footprint_raster_tiler`，Average/Max/Min 维持 footprint 路径，Nearest/Bilinear 通过 strict bounds path 调用，不再使用 terrain 的 nearest half-pixel support。重跑 plain Int32 z0 后，原版/Rust 的 Nearest 和 Bilinear 解码 XYZ 矩阵均一致；进程回归固定 z0 row 32/column 64 的 nearest 值为 0。常规验证为 62 tests 与 Clippy 无告警。该结论只覆盖 plain z0 的 nearest/bilinear；其余 zoom、source types、overview 和其余 resampling 算法仍需各自 oracle。

GTiff storage-type z0 matrix（2026-08-04）：对 Int16、无 NoData 的 UInt16、及缩放后的 Float32 source，原版/Rust 的 z0 path 和 `nearest/bilinear/average` 解码样本值均一致；由 `RasterSampleType` 选择的 output types 可读回对应 storage type。将 `-9999` NoData 保留在 UInt16 source 时，原版 GDAL 仍在输出 UInt16 TIFF 写入字面 `NoData=-9999` tag；`geotiff-writer` 正确拒绝该不具表示性的 tag，Rust 当前因此返回错误。这是 writer 能力与原版宽容 metadata 行为的缺口，不能通过删除或改写 NoData 隐藏。z1 的初步对照显示 path 集一致；XYZ 第一、二列仅有 GeoTransform 浮点文本 ULP 差异，后续需以第三列 values 比较完整 z1 matrix。

原版范围校正（2026-08-04）：`cesium-terrain-builder` README 的 “Limitations and TODO” 明确将 Quantized-Mesh 1.0 列为“Add support”未来项；源树中不存在 Quantized-Mesh、`layer.json` 或相关 writer。因而 CTB 0.4.1 的“全部功能和接口”不包含 Quantized-Mesh，Rust 不得将其作为原版已具备的兼容要求。原版真实的非 Terrain 输出是任意 GDAL `CreateCopy` driver（README 示例 JPEG、VRT），以及 `GlobalMercator`/`--profile mercator`；后续矩阵和阶段以这些已存在的路径为准。

GlobalMercator 领域设计：原版 `GlobalMercator(tileSize=256)` 是通用 `Grid` 的 extent `[-originShift,+originShift]^2`，`originShift = π * 6378137`，root tile count 1，zoom factor 2，EPSG:3857。故 z0 resolution 为 `2π*6378137/tileSize`，tile bounds 仍通过底左 PixelToCRS 生成，tile x/y 均为 TMS 自南向北。先引入独立 `GlobalMercatorGrid`，测试 origin shift、z0/z1 resolution、bounds 与边界 tile mapping；它不改变现有 geodetic grid。随后将 `Crs`/GeoTIFF adapter 扩展至 EPSG:3857 direct-source，RasterTiler GTiff 可先支持同 CRS no-reprojection；EPSG:4326 → 3857 的 Web Mercator forward transform、反向 bounds/采样和任意 GDAL CRS 仍按原版 GDAL transformer 另立 oracle，不能仅因存在公式而擅自假定像素对齐。

GlobalMercatorGrid 实施状态（2026-08-04）：已实现独立的纯领域 grid，常量为 `SEMI_MAJOR_AXIS=6378137` 与 `ORIGIN_SHIFT=π×6378137`，默认外部调用可传 CTB 的 256 tile size。控制点覆盖 root tile 1×1、z0/z1 resolution、z0 extent 与 z1 north-east bounds，以及 TMS 自南向北坐标。它未接入 existing geodetic `TilesetPlan`、RasterTiler 或 GeoTIFF adapter；当前不改变任何 I/O/CRS 行为。常规验证为 64 tests 与 Clippy 无告警。

Mercator direct-source oracle（2026-08-04）：以 GDAL 将 plain Int32 EPSG:4326 source warp 为 EPSG:3857，再运行原版 `ctb-tile -q -f GTiff -p mercator`。输出路径为 z0 `0/0/0.tif` 及 z1 全 2×2 tiles；z0 为 256×256 `Int32`、NoData `-9999`、EPSG:3857，GeoTransform `[-20037508.342789244, 156543.03392804097, 0, 20037508.342789244, 0, -156543.03392804097]`、block `256×8`。这证实 cpp CTB 的 Mercator default tile size 256（不同于 geodetic 的 65）且 target CRS 确为 3857。下一实现单元须抽出“Grid/target CRS/metadata bounds”领域边界，使 existing `TilesetPlan`/`RasterTileSamplePlan` 既能表达 geodetic 也能表达 Mercator；禁止把 EPSG:3857 metres 塞入现有 `GlobalGeodeticGrid`。

通用 Grid 接口设计：cpp 的 `Grid` 是所有 tiler 的共同参数，持有 tile size、extent、SRS、initial resolution、origin shift 和 zoom factor；`GlobalGeodetic`、`GlobalMercator` 都以其 `resolution`、`crsToTile`、`tileBounds` 实现 RasterTiler。Rust 增加同等的 `TileGrid` trait（`crs`、`tile_size`、`resolution`、`zoom_for_resolution`、`tile_bounds`、`coordinate_to_tile`、tile counts），由现有两个 concrete grids 实现。首个接口单元只增加 trait 与无副作用控制点；现有 public concrete methods 保持不变。下一单元才让 `TilesetPlan` 与 `RasterTileSamplePlan` 接受 `&dyn TileGrid`，并明确 metadata CRS 必须等于 target CRS 之后才允许 direct-source sampling。

TileGrid 接口实施状态（2026-08-04）：已增加 `TileGrid` trait，并由 GlobalGeodetic/GlobalMercator 实现，trait 覆盖 cpp `Grid` 的 target CRS、tile size、resolution/zoom、tile counts、bounds 与 CRS-to-tile 操作。控制点还固定 root `(0,0)` 的 profile 差异：geodetic root 有两个横向 tiles，原点属于 `(x=1,y=0)`；Mercator root 唯一 tile，原点属于 `(x=0,y=0)`。现有 concrete public methods 保持原样，接口尚未传入 `TilesetPlan` 或任何 writer。常规验证为 65 tests 与 Clippy 无告警。

### P2 当前实施单元：可复现大 DEM 基准（不依赖 overview）

在 overview 接口确认前，补充一个开发者 benchmark 脚本：用本地 GDAL 从版本控制的最小 ASCII fixture 派生固定尺寸 EPSG:4326、tiled/DEFLATE GeoTIFF，运行 Rust `ctb-tile` 的 `-c 1` 与指定 worker 数，记录 wall-clock 时间、输出 tile 数和解压 payload 一致性。脚本不进入常规 `cargo test`、不提交大二进制 fixture、不以基准数值作为跨机器阈值；它只提供后续 P2 的可复现测量入口。

基准入口验收：`scripts/benchmark-ctb-tile.zsh 512 2` 已在本机完成；它从 `oracle-source.asc` 派生临时 tiled/DEFLATE GeoTIFF，运行单 worker 与双 worker，并在清理前比较每个解压 payload。耗时仅在脚本 stdout 中按本机记录，不作为版本控制的性能承诺。

完成标准：`-c 1`、`-c 2` 与默认模式的 tile 路径及解压 payload 一致；`-R` 不重写既有最终文件；`-q` 无正常 stdout，`-v` 有原版形状的创建日志；worker 的输入失败与写入失败能传播而不 panic。
