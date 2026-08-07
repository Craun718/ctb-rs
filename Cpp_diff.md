# Rust vs C++ CTB Terrain 对齐总结

## 1. 背景

本项目以 C++ 版 `cesium-terrain-builder`（CTB）及其使用的 GDAL 行为作为 oracle，
对齐 Rust 版 `ctb-rs` 的 Terrain 输出。比较对象是 `ctb-tile` 生成的 `.terrain`
文件路径集合，以及 gzip 解压后的 heightmap payload；压缩容器字节差异不作为行为差异。

本次对齐覆盖 Terrain + Average 路径，包含：

- 真实 Copernicus DSM COG 的 geodetic 全量输出。
- `world3857/source.tif` 的 Mercator Terrain VRT block pooled 输出。

## 2. 测试数据

### 2.1 Copernicus geodetic

- 输入：`tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`（Git LFS；原始
  路径 `/Users/sander/coding/demo/download-data/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`）
- 命令范围：`ctb-tile -q -c 4 -s 14 -e 0`
- C++ oracle 路径数：11391
- Rust 修复后路径数：11391

### 2.2 Mercator pooled

- 输入：`/private/tmp/ctb-mercator-pooled-check/world3857/source.tif`
- 数据：720×720、EPSG:3857、Int32
- C++ oracle 路径数：38
- Rust 修复后路径数：38

## 3. 修复前的可观察差异

### 3.1 Copernicus geodetic

路径集合一致，但解压后 payload 差异较大：

| 项 | 修复前 |
|---|---:|
| Terrain 文件总数 | 11391 |
| payload 相同 | 32 |
| payload 不同 | 11359 |
| 最大 u16 高度差 | 1919 |
| 最大高度差 | 383.8 m |

### 3.2 Mercator pooled

路径集合一致，38 个文件中有 10 个 payload 不同：

```text
0/0/0、2/0/0、2/0/1、2/0/2、2/1/0、2/1/1、
2/1/2、2/1/3、2/2/0、2/2/2
```

## 4. 差异点与根因

### 4.1 geodetic 真实 COG 的 overview warp 混合行为

C++ 的 transformer 使用 GDAL 选出的 overview GeoTransform 做坐标数学，但
`psWarpOptions->hSrcDS` 仍指向主数据集，因此最终 source 窗口按 base 数据尺寸
夹取并读取。Rust 不能只按 base 数据尺寸或只按 overview 尺寸计算，需要保留
`level: 0 + overview metadata` 的采样语义，并用 overview GT 计算 pooled
source window。

### 4.2 average margin 来源

GDAL 的 `average_margin` 由实际 pooled source window 宽高推导，而不是由固定
`HEIGHTMAP_TILE_SIZE=65` 或原始 transform ratio 推导。真实 COG 在低 zoom
会出现很大的 pooled margin；修正后才与 C++ 的 `WarpRegion` 行为一致。

### 4.3 Mercator VRT block 尺寸

C++ 的 Mercator Terrain 流程：

1. `TerrainTiler` 用 `mGrid.tileSize()` 创建 Mercator VRT，尺寸为 256×256。
2. 只 `RasterIO` 读取左上 65×65 高度样本。
3. `VRTWarpedDataset` 默认 block 尺寸为
   `min(nXSize,512) × min(nYSize,128)`，因此实际 warp destination 是
   256×128。

Rust 之前按 65×65 计算 pooled source window 和 margin，导致 Mercator 首 block
与 C++ 不一致。修复后 `TerrainSamplePlan` 保存：

- geodetic：65×65
- mercator：256×128

输出仍固定为 65×65，只影响 pooled window、margin 和行坐标计算。

### 4.4 整行 GDALApproxTransform

C++ 的 `GWKAverageOrModeComputeLineCoords` 对整行 warp block 像素调用
`GDALApproxTransform`，Mercator 每次变换 256 个点，而不是逐像素调用精确
`GDALGenImgProjTransform`。

Rust 之前使用逐像素精确变换，递归近似分支产生的坐标与 C++ 有约 `1e-14`
级误差；当 source 坐标位于 `0.5` 边界附近时，会表现为 ±1 的整数舍入差。
修复后移植了 `GDALApproxTransformInternal` 的等价逻辑。

### 4.5 FMA contraction

本机 C++ GDAL 构建会把以下表达式收缩为 FMA：

- `origin + pixel * pixel_size`
- `GDALApproxTransformInternal` 的插值和误差表达式

Rust 使用普通 `+`/`*` 时末位浮点结果不同。为逐位匹配 C++ oracle，坐标计算
改为 `f64::mul_add`。

### 4.6 base transform 切片长度

`GDALApproxTransformInternal` 的 half-2 和 fallback 分支只对
`nPoints - nMiddle - 2` 个点执行 base transform；最后一个点会被
Start/Middle/End（SME）结果覆盖。

Rust 修正为：

```text
dst_x[n_middle + 1..n_points - 1]
dst_y[n_middle + 1..n_points - 1]
```

避免把 C++ 随后覆盖的点当作独立精确变换输入。

## 5. 修复后结果

| 场景 | 路径一致 | 解压后 payload 差异 |
|---|---:|---:|
| Mercator `world3857/source.tif` | 38/38 | 0 |
| Copernicus geodetic 全量 | 11391/11391 | 0 |

验证门禁：

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
  - 88 个 lib 单元测试
  - 9 个 `ctb-tile` 参数测试
  - 12 个 CLI 测试
  - 1 个 `ctb-info` 测试
- `cargo build --release`
- Mercator 38 个 Terrain payload 与 C++ oracle 逐文件比较
- Copernicus 11391 个 Terrain payload 与 C++ oracle 逐文件比较

## 6. 代码位置

- `src/terrain_sampling.rs`：Terrain pooled Average、source window、margin、
  `GDALApproxTransform` 行坐标和 FMA 对齐。
- `TECHNICAL_PLAN.md`：P14/P15 的根因、实施范围和验证记录。
- `TEST_STRATEGY.md`：C++/Rust oracle 测试策略和实测结果。
- `TODO.md`：P14/P15 任务状态。

## 7. 范围说明

本次总结针对 Terrain + Average 路径，重点修复真实 COG geodetic 和 Mercator
Terrain VRT block pooled 行为。RasterTiler、非 Average resampling 的完整 C++
差分不属于本次 P15 范围。
