# ctb-rs

**English** | [简体中文](README.zh-CN.md)

`ctb-rs` is a pure Rust rewrite of [Cesium Terrain Builder](https://github.com/ahuarte47/cesium-terrain-builder) (C++ CTB 0.4.1). It generates [heightmap-1.0](https://cesium.com/learn/cesiumjs/ref-doc/HeightmapTerrainData.html) terrain tiles for Cesium's `CesiumTerrainProvider`. Its library and four command-line tools—`ctb-tile`, `ctb-info`, `ctb-export`, and `ctb-extents`—follow the original implementation's interfaces, output, and error behavior.

This project generates terrain tiles; it does not publish them to a browser. For serving tiles, see [Cesium Terrain Server](https://github.com/geo-data/cesium-terrain-server).

## Development roadmap

- [x] Reproduce the original implementation function by function
- [x] Optimize without changing output
- [ ] Match the C++ implementation's performance
- [x] Provide a library API to generate `layer.json`
- [ ] Support quantized-mesh output

## Features

Library callers can use `tileset::write_layer_json` after writing terrain tiles to generate Cesium metadata from the grid and tileset plan.
The `ctb-tile` Terrain output generates this file by default. Pass `--no-layerjson` to disable it.

- **Pure Rust, with no GDAL/PROJ FFI:** No GDAL, PROJ, or other C/C++ GIS FFI is linked. GeoTIFF and VRT input use `geotiff-reader`; GeoTIFF output uses `geotiff-writer`; standard VRT XML is parsed by an in-project compatibility layer; general EPSG coordinate transformations use pure Rust `proj4rs`.
- **Behavior aligned with the reference:** Tile generation uses C++ CTB as its behavioral reference. The optional `layer.json` output is a ctb-rs extension.
- **Two TMS profiles:** Global Geodetic (EPSG:4326) and Global Mercator (EPSG:3857), with y increasing from south to north. The Geodetic root level is `2x1`; the Mercator root level is `1x1`.
- **12 resampling algorithms:** nearest, bilinear, cubic, cubicspline, lanczos, average, mode, max, min, med, q1, and q3.
- **`#![forbid(unsafe_code)]`:** Library code contains no unsafe code.

## Command-line tools

All four tools use the original names. `--version` reports the current project version (`0.1.0`); the C++ CTB reference program remains fixed at `0.4.1`. Output equivalence between ctb-rs and C++ CTB is guaranteed only within `v0.1.x`; the version number does not claim broader Rust/C++ compatibility.

### `ctb-tile`

Generates `{z}/{x}/{y}.terrain` tiles from a GeoTIFF or VRT DEM whose EPSG code is supported by proj4rs. It derives the maximum zoom from the source resolution and generates overlapping tiles from highest to lowest zoom. GeoTIFF (GTiff) output is also supported. EPSG:4326 and EPSG:3857 use built-in formulas; other supported EPSG inputs are reprojected to the target CTB profile with proj4rs. Arbitrary WKT input is not currently supported.

```sh
ctb-tile --output-dir ./terrain-tiles dem.tif
```

Main options:

```text
-o, --output-dir <dir>          Output directory (default: current directory)
-f, --output-format <format>    Terrain (default) or GTiff
-p, --profile <profile>         geodetic (default) or mercator
-t, --tile-size <size>          Pixel edge length; Terrain is fixed at 65, raster default is 256
-s, --start-zoom <zoom>         Starting (highest) zoom; defaults to the source-derived maximum
-e, --end-zoom <zoom>           Ending (lowest) zoom; default: 0
-r, --resampling-method <name>  Resampling algorithm; default: average
-n, --creation-option <opt>     GDAL creation option in NAME=VALUE form; may be repeated
-z, --error-threshold <px>      Approximate transformation error threshold; default: 0.125
-m, --warp-memory <bytes>       Warp memory limit; 0 uses the default
-c, --thread-count <count>      Worker count; non-positive values use the CPU count
-R, --resume                    Do not overwrite tiles already written
    --no-layerjson              Do not generate layer.json for terrain output
-q, --quiet                     Print errors only
-v, --verbose                   Print each completed tile
```

### `ctb-info`

Prints information about a `.terrain` file, including child availability, water/land classification, and optionally an ASCII elevation matrix.

```sh
ctb-info --show-heights ./terrain-tiles/0/0/0.terrain
```

### `ctb-export`

Exports a `.terrain` tile to GeoTIFF. Terrain files do not contain their own location, so zoom and TMS coordinates must be provided.

```sh
ctb-export -i ./0/0/0.terrain -z 0 -x 0 -y 0 -o tile.tif
```

### `ctb-extents`

Writes the tile extents covered by a source DEM, grouped by zoom, to GeoJSON files.

```sh
ctb-extents --output-dir ./extents dem.tif
```

## Raster formats

Input supports GeoTIFF (including BigTIFF) and standard VRT XML. Output supports CTB Terrain and GeoTIFF. Other raster inputs, including `.nc`, `.h5`, and `.jp2`, return an unsupported-format error before any tiles are written.

## Build

This project uses Rust 2024 edition and requires a recent Rust toolchain (1.85 or later is recommended).

```sh
cargo build --release
```

The four binaries—`ctb-tile`, `ctb-info`, `ctb-export`, and `ctb-extents`—are placed in `target/release/`. To install them to the user's binary directory:

```sh
cargo install --path .
```

## Tests

Integration tests cover core paths for all four CLIs and generate temporary GeoTIFF/VRT inputs:

```sh
cargo test
```

The real Copernicus DEM fixture is managed with Git LFS at `tests/Copernicus_DSM_COG_10_N22_00_E108_00_DEM.tif`. If the LFS object was not fetched during clone, run `git lfs pull`.

Scripts under `scripts/` compare results against the C++ reference:

- `verify-ctb-oracle.zsh`: compares generated tiles against C++ CTB output.
- `benchmark-ctb-tile.sh`: benchmarks `ctb-tile`.
- `benchmark-ctb-cpp-rust-timeout.zsh`: records the C++ wall time, then gives Rust twice that time as its timeout and compares shared `.terrain` outputs.

## How it works

Terrain elevation uses the C++ encoding `uint16_t((Float32_height + 1000) * 5)`, truncating toward zero within the valid range. Areas not covered by the source start at `0.0` m. The `ctb-tile` Terrain path ignores the CLI resampling algorithm and always uses the original Average path; the GTiff path uses the selected GDAL algorithm. Terrain Average filters `NaN`, `±inf`, and samples exactly equal to the NoData value declared for the current sampling level. If the source window contains no valid samples, it outputs sea level (`0 m`) and keeps the default all-land water mask. Numeric NoData declared by GeoTIFF and VRT is handled by the same rule. Tile grid and traversal behavior is implemented in [`src/grid.rs`](src/grid.rs) and [`src/tileset.rs`](src/tileset.rs).

## Status

This project is an ongoing behavioral-alignment port. The original library modules and all four CLI paths have been implemented. Differential verification uses C++ CTB fixed at commit `d9c29b2` with GDAL 3.11.4. See [TODO.md](TODO.md) for current work and [TECHNICAL_PLAN.md](TECHNICAL_PLAN.md) for project scope and documentation conventions.

## Acknowledgements

This project is a rewrite based solely on the C++ implementation in [ahuarte47/cesium-terrain-builder](https://github.com/ahuarte47/cesium-terrain-builder), not `geo-data/cesium-terrain-builder`.

## License

MIT
