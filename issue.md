# Rust 版 `ctb-tile` 低 zoom 性能问题

## 现象

本机单次复测（release 构建，同一 Copernicus DEM fixture）：

| 范围 | Rust | C++ | 倍数 |
| --- | ---: | ---: | ---: |
| z14 | 1.85 s | 1.04 s | 1.8x |
| z13 | 1.48 s | 0.32 s | 4.6x |
| z8 | 0.68 s | 0.08 s | 8.5x |
| z0 | 50.97 s | 0.18 s | 283x |
| z14->z0 全量 | 91.68 s | 1.46 s | 62.8x |

输入文件为 `tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`：

- 3600x3600，Float32
- base band block = 1024x1024
- DEFLATE，PREDICTOR=3
- 三级 overview：1800x1800、900x900、450x450

profile 显示约 93% 的采样时间集中在：

```text
CachedRasterSource::read_sampling_window
  -> GeoTiffRasterSource::read_samples
    -> oxigeo read_window_into_typed
      -> CogReader::read_tile_into
        -> oxiarc_deflate inflate
```

平均算法和 terrain/gzip 写出不是热点。

## 源码调用链

- `src/bin/ctb-tile.rs:215-219`：每个 worker 的 `source_factory` 都创建
  `CachedRasterSource::new_with_nodata_cache(source, 64, 64)`。
- `src/tileset.rs:400-413`：每个 worker 线程调用 `source_factory()`，因此每个 worker
  持有独立的 source 和 cache。
- `src/cache.rs:82-111`：cache miss 时把请求对齐到 64x64，然后请求底层 source。
- `src/cache.rs:137-184`：窗口读取逐像素调用 `cached_block`，命中 64x64 块时从内存取数，
  跨块时重新请求新的 64x64 block。
- `src/terrain_sampling.rs:166-175`：低 zoom 的 pooled source window 会请求完整 base
  band 窗口。
- `src/geotiff.rs:160-182`：`GeoTiffRasterSource::read_samples` 每次调用 OxiGeo
  `read_window_into_typed::<f64>`。
- OxiGeo `band_read.rs:1320-1334`：每次调用重建 `ReadPlan`，再进入 scatter/decode。
- OxiGeo `band_read.rs:19-24`：说明只复用 scratch buffer，不保存已解码 block。
- OxiGeo `cog/mod.rs:811-870`：`read_tile_into` 每次从文件读取 raw block，并执行
  deflate/predictor 解码。

本机依赖源码位置：

```text
~/.cargo/registry/src/index.crates.io-*/oxigeo-geotiff-0.2.3/src/
```

C++ 对照源码位于：

```text
/Users/sander/coding/clone-github/cesium-terrain-builder
```

## 根因

### 1. 低 zoom 实际仍读取整张 base band

COG 虽然有 overviews，但 `src/geotiff.rs:301-311` 明确保留了 C++ oracle 的行为：`level`
固定为 0，只把 overview 元数据用于坐标计算，实际仍从 base band 读取。因此 z0 的
`CachedRasterSource::read_sampling_window` 会请求接近 3600x3600 的完整 base 窗口。

### 2. 应用层缓存粒度与真实 TIFF block 不一致

当前缓存粒度为 64x64，而真实 COG block 是 1024x1024。每次 64x64 cache miss 都会让
OxiGeo 重新读取并 inflate 其覆盖到的完整 1024x1024 tile。

对一个完整 base band 窗口来说，一个内部 1024x1024 block 会被拆成 16x16 = 256 个
64x64 请求；如果 OxiGeo 没有 decoded block cache，同一个 1024x1024 block 在一轮全图请求中
就可能被 inflate 256 次。

### 3. OxiGeo 当前没有 decoded block cache

OxiGeo 的 scratch 只复用一个临时缓冲区，`read_tile_into` 每次都会读取压缩数据并重新解码。
因此 64x64 应用层缓存命中只避免了重复的 OxiGeo 调用，无法避免底层重复 inflate。

### 4. worker 之间也没有共享缓存

每个 Rust worker 都有独立的 `GeoTiffRasterSource` 和 `CachedRasterSource`，即使同一
1024x1024 block 被多个 worker 反复需要，也没有跨 worker 复用机制。

C++ 侧通过 GDAL 读取。GDAL 自带按真实 TIFF block 的 block cache，本机基准使用 819 MiB
缓存；base band 解码后约 49 MiB，完全可以驻留，所以同一个 block 解压一次后后续读取直接命中。

## 影响

- zoom 越高，每个 tile 的 source window 越小，重复 inflate 的放大倍数越小，所以 z14 只有约
  1.8x 差距。
- zoom 越低，source window 越接近整张 base band，64x64 请求数量激增，底层同一
  1024x1024 block 被反复 inflate，z0 放大到约 283x。
- 全量 z14->z0 的性能主要由低 zoom 支配，所以总耗时约 62.8x。

## 待决策

当前只记录问题，尚未改生产代码。候选方向：

- 在 `GeoTiffRasterSource` 层按 `(level, tile_x, tile_y)` 缓存解码后的 native block，
  而不是在应用层缓存 64x64 的 f64 窗口。
- 缓存对象建议是解码后的 Float32 block（约 4 MiB/block），不要直接沿用当前 f64
  `Vec<f64>`；否则 1024x1024 缓存会达到 8 MiB/block，容量控制需要重新评估。
- 对完整 base 窗口的大请求，可考虑直接按真实 TIFF block 批量读取，避免 3600x3600 被
  拆成 3249 个 64x64 OxiGeo 调用。
- 如果要完全对齐 GDAL，需要决定缓存是否跨 worker 共享，以及容量/内存上限。

这些方向在 P18 落地前记录如下；落地结果见下方“已实施修复”。

## 已实施修复（P18）

P18 已在本项目 `src/geotiff.rs` 落地，不改 OxiGeo、Cargo 依赖、应用层缓存或
采样算法。它在 `GeoTiffRasterSource` 层按真实 TIFF block 缓存已解码原生字节，
key 为 `(level, tile_x, tile_y)`，按 tiled/striped IFD 几何读取，64 MiB 预算 +
LRU 淘汰；窗口片段用 `convert_raw_into` 保持与 `read_window_into_typed::<f64>`
相同的转换语义。因此这个问题是 ctb-rs 自己的读取路径问题，不是必须修改
OxiGeo 才能解决，也不需要为提速牺牲 C++ 兼容性。

所谓“应用层缓存颗粒度不一致”，是指 `CachedRasterSource` 以 64×64 f64 窗口为
缓存单位，而真实 COG block 是 1024×1024；64×64 缓存命中只能避免重复的 OxiGeo
调用，不能阻止同一个真实 block 被反复读取、inflate 和转换。P18 把缓存单位
对齐到真实 TIFF block 后，这个问题已消除。

修复后同一 Copernicus DEM 实测（release，同一台机器）：

| 范围 | Rust 修复后 | C++ | 修复前 Rust |
| --- | ---: | ---: | ---: |
| z0 | 4.24 s | 0.78 s | 50.97 s |
| z14->z0 | 8.18 s | 1.51 s | 91.68 s |

低 zoom 从约 283x 降到约 5.4x，全量从约 62.8x 降到约 5.4x。输出回归：
geodetic 11391/11391、Mercator 38/38，路径与解压后 payload 差异均为 0。
剩余差距不是采样算法差异，后续若继续优化，可评估跨 worker 共享 block 缓存、
大窗口按真实 block 批量读取，以及是否进一步降低 OxiGeo 单次窗口读取开销。
