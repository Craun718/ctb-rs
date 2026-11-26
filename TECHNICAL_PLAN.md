# ctb-rs Technical Plan

## 1. Project scope

`ctb-rs` is a pure Rust rewrite of C++ Cesium Terrain Builder 0.4.1. Its
behavioral reference is the upstream C++ implementation. The project generates
Cesium heightmap-1.0 terrain tiles and provides the `ctb-tile`, `ctb-info`,
`ctb-export`, and `ctb-extents` command-line tools. It does not publish tiles
to a web server.

Implementation and compatibility claims must remain grounded in the current
source, fixtures, and C++ oracle results. Documentation must not imply support
or verification beyond that evidence.

## 2. Documentation and language

The repository's top-level `README.md` is the English entry point. The complete
Chinese translation lives in `README.zh-CN.md`. Both files link to each other
near the title. Keep feature descriptions, command examples, limitations,
version claims, and project status aligned between the two files.

The GitHub repository description is a concise English summary of the same
project scope. It should describe the Rust terrain-tile generator without
claiming unverified compatibility or output formats.

## 3. Documentation work order

1. Record the README language and cross-link requirements here.
2. Add a prioritized documentation task to `TODO.md`.
3. Translate the existing README content into English and move its Chinese
   counterpart to `README.zh-CN.md`, adding reciprocal links.
4. Update the GitHub repository description in English and verify the result.
5. Review both README files for matching scope and links.

## 4. Terrain average reprojection at low zoom

Destination pixels outside the source projection domain are invalid warp
samples. Exclude them from the source window and return the existing zero
fill for pixels with invalid corners. Probe the tile interior when the edge
contains invalid points, and transform scanlines directly when interpolation
anchors are invalid. Keep CRS resolution and forward bounds planning strict.

## 5. Cesium terrain metadata

Expose an optional layer.json writer for completed heightmap tilesets. Use the
grid CRS and the actual tileset plan to describe TMS tile URLs and availability.
Keep tile generation and the existing CLI behavior unchanged. Verify both
supported profiles, missing lower levels and output errors.

## 6. CLI terrain metadata option

Generate layer.json after successful Terrain output by default. The
`--no-layerjson` flag disables this step. GTiff output remains unchanged.
Verify both flag states and their generated files through the CLI.

## 8. Desktop mesh sampling at low zoom

Expose the heightmap warp coordinate transform for desktop mesh vertex
sampling. A coordinate outside the projection domain returns NaN for both
ordinates. Other transform errors propagate to the caller. For mesh vertices,
encode invalid warp coordinates as the existing 0 m NoData value without
reading the raster. Continue bilinear sampling for valid coordinates.
Tile bounds, tileset planning and raster reads retain strict error handling.

Verify level 0 and detail mesh elevations from the same EPSG:4545 GeoTIFF
with the WGS84 and Mercator grids. Check that unknown CRS and raster read
errors fail the export, and verify heightmap regression coverage and desktop
export metadata.
