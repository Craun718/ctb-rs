use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{
    CtbError,
    grid::{Bounds, GlobalGeodeticGrid, TileCoord, TileGrid},
    raster::{RasterMetadata, RasterSource, transform_bounds},
    sampling::ResamplingMethod,
    terrain::{ChildMask, HeightmapTerrain},
    terrain_sampling::TerrainSamplePlan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilesetLevel {
    pub zoom: u8,
    pub tiles: Vec<TileCoord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilesetPlan {
    pub max_zoom: u8,
    pub levels: Vec<TilesetLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeightmapTilesetOptions {
    pub resume: bool,
    pub start_zoom: Option<u8>,
    pub end_zoom: Option<u8>,
    pub resampling: ResamplingMethod,
    /// Number of workers consuming the ordered tile queue. Zero is normalized
    /// to one for library callers; the CLI applies CTB's CPU-count default.
    pub worker_count: usize,
}

impl Default for HeightmapTilesetOptions {
    fn default() -> Self {
        Self {
            resume: false,
            start_zoom: None,
            end_zoom: None,
            resampling: ResamplingMethod::Average,
            worker_count: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileWriteProgress {
    pub tile: TileCoord,
    pub completed: usize,
    pub total: usize,
}

impl TilesetPlan {
    pub fn from_raster(
        metadata: &RasterMetadata,
        grid: GlobalGeodeticGrid,
    ) -> Result<Self, CtbError> {
        Self::from_raster_with_zoom_range(metadata, grid, None, None)
    }

    pub fn from_raster_with_zoom_range(
        metadata: &RasterMetadata,
        grid: GlobalGeodeticGrid,
        start_zoom: Option<u8>,
        end_zoom: Option<u8>,
    ) -> Result<Self, CtbError> {
        let bounds = metadata.transform.bounds(metadata.width, metadata.height)?;
        let available_maximum = grid.zoom_for_resolution(metadata.transform.pixel_width)?;
        let max_zoom = start_zoom.unwrap_or(available_maximum);
        let min_zoom = end_zoom.unwrap_or(0);
        if max_zoom > available_maximum || min_zoom > max_zoom {
            return Err(CtbError::InvalidZoomRange {
                start: max_zoom,
                end: min_zoom,
                maximum: available_maximum,
            });
        }
        let mut levels = Vec::with_capacity(usize::from(max_zoom - min_zoom) + 1);
        for zoom in min_zoom..=max_zoom {
            let range = grid.tile_range_for_area(bounds, zoom)?;
            let mut tiles = Vec::new();
            for x in range.lower_left.x..=range.upper_right.x {
                for y in range.lower_left.y..=range.upper_right.y {
                    tiles.push(TileCoord { zoom, x, y });
                }
            }
            levels.push(TilesetLevel { zoom, tiles });
        }
        Ok(Self { max_zoom, levels })
    }

    /// Plan RasterTiler coverage against any built-in CTB Grid profile,
    /// transforming the source bounds through the supported CRS registry.
    pub fn from_raster_with_tile_grid(
        metadata: &RasterMetadata,
        grid: &dyn TileGrid,
        start_zoom: Option<u8>,
        end_zoom: Option<u8>,
    ) -> Result<Self, CtbError> {
        let source_bounds = metadata.transform.bounds(metadata.width, metadata.height)?;
        let transformed_bounds =
            crate::raster::transform_bounds(source_bounds, &metadata.crs, &grid.crs())?;
        let bounds = transformed_bounds
            .intersection(grid_bounds(grid)?)
            .ok_or(CtbError::RasterOutsideGrid)?;
        let source_resolution = if metadata.crs == grid.crs() {
            metadata.transform.pixel_width.abs()
        } else {
            bounds.width() / f64::from(metadata.width)
        };
        let available_maximum = grid.zoom_for_resolution(source_resolution)?;
        let max_zoom = start_zoom.unwrap_or(available_maximum);
        let min_zoom = end_zoom.unwrap_or(0);
        if max_zoom > available_maximum || min_zoom > max_zoom {
            return Err(CtbError::InvalidZoomRange {
                start: max_zoom,
                end: min_zoom,
                maximum: available_maximum,
            });
        }
        let mut levels = Vec::with_capacity(usize::from(max_zoom - min_zoom) + 1);
        for zoom in min_zoom..=max_zoom {
            let lower_left = grid.coordinate_to_tile(bounds.min_x, bounds.min_y, zoom)?;
            let upper_right = grid.coordinate_to_tile(bounds.max_x, bounds.max_y, zoom)?;
            let mut tiles = Vec::new();
            for x in lower_left.x..=upper_right.x {
                for y in lower_left.y..=upper_right.y {
                    tiles.push(TileCoord { zoom, x, y });
                }
            }
            levels.push(TilesetLevel { zoom, tiles });
        }
        Ok(Self { max_zoom, levels })
    }

    pub fn child_mask_for(&self, parent: TileCoord) -> ChildMask {
        let Some(child_level) = self
            .levels
            .iter()
            .find(|level| level.zoom == parent.zoom + 1)
        else {
            return ChildMask::empty();
        };
        let children = child_level.tiles.iter().copied().collect::<BTreeSet<_>>();
        child_mask_from_tiles(parent, &children)
    }
}

fn grid_bounds(grid: &dyn TileGrid) -> Result<crate::grid::Bounds, CtbError> {
    let last_x = grid.tiles_x(0)?.saturating_sub(1);
    let last_y = grid.tiles_y(0)?.saturating_sub(1);
    let lower = grid.tile_bounds(TileCoord {
        zoom: 0,
        x: 0,
        y: 0,
    })?;
    let upper = grid.tile_bounds(TileCoord {
        zoom: 0,
        x: last_x,
        y: last_y,
    })?;
    crate::grid::Bounds::new(
        lower.min_x.min(upper.min_x),
        lower.min_y.min(upper.min_y),
        lower.max_x.max(upper.max_x),
        lower.max_y.max(upper.max_y),
    )
}

/// Match the C++ TerrainTiler::createTile child mask computation: check
/// whether the dataset bounds (in grid CRS) strictly overlap each quadrant
/// of the tile bounds.  C++ `Bounds::overlaps` uses strict `<`, so bounds
/// that merely touch at an edge do not count as overlapping.
fn terrain_child_mask(
    source_bounds: Bounds,
    grid: &dyn TileGrid,
    tile: TileCoord,
    max_zoom: u8,
) -> Result<ChildMask, CtbError> {
    if tile.zoom >= max_zoom {
        return Ok(ChildMask::empty());
    }
    let tile_bounds = grid.tile_bounds(tile)?;
    if !strict_overlaps(source_bounds, tile_bounds) {
        return Ok(ChildMask::empty());
    }
    let mid_x = (tile_bounds.min_x + tile_bounds.max_x) / 2.0;
    let mid_y = (tile_bounds.min_y + tile_bounds.max_y) / 2.0;
    let mut mask = ChildMask::empty();
    if strict_overlaps(
        source_bounds,
        Bounds::new(tile_bounds.min_x, tile_bounds.min_y, mid_x, mid_y)?,
    ) {
        mask.set(ChildMask::SOUTH_WEST, true);
    }
    if strict_overlaps(
        source_bounds,
        Bounds::new(mid_x, tile_bounds.min_y, tile_bounds.max_x, mid_y)?,
    ) {
        mask.set(ChildMask::SOUTH_EAST, true);
    }
    if strict_overlaps(
        source_bounds,
        Bounds::new(tile_bounds.min_x, mid_y, mid_x, tile_bounds.max_y)?,
    ) {
        mask.set(ChildMask::NORTH_WEST, true);
    }
    if strict_overlaps(
        source_bounds,
        Bounds::new(mid_x, mid_y, tile_bounds.max_x, tile_bounds.max_y)?,
    ) {
        mask.set(ChildMask::NORTH_EAST, true);
    }
    Ok(mask)
}

/// C++ `Bounds::overlaps` semantics: strict `<` on all four comparisons.
fn strict_overlaps(a: Bounds, b: Bounds) -> bool {
    a.min_x < b.max_x && b.min_x < a.max_x && a.min_y < b.max_y && b.min_y < a.max_y
}

/// Write CTB heightmap terrain tiles, processing child levels before parents.
pub fn write_heightmap_tileset(
    source: &dyn RasterSource,
    grid: GlobalGeodeticGrid,
    output_directory: impl AsRef<Path>,
    resume: bool,
) -> Result<TilesetPlan, CtbError> {
    write_heightmap_tileset_with_options(
        source,
        grid,
        output_directory,
        HeightmapTilesetOptions {
            resume,
            ..HeightmapTilesetOptions::default()
        },
    )
}

/// Write CTB heightmap terrain tiles with an explicit CTB-compatible subset of options.
pub fn write_heightmap_tileset_with_options(
    source: &dyn RasterSource,
    grid: GlobalGeodeticGrid,
    output_directory: impl AsRef<Path>,
    options: HeightmapTilesetOptions,
) -> Result<TilesetPlan, CtbError> {
    write_heightmap_tileset_with_progress(source, grid, output_directory, options, None)
}

/// Write CTB heightmap terrain tiles through an ordered shared work queue.
///
/// `progress` is invoked from the worker that completed each planned tile,
/// including tiles skipped by `resume`, matching CTB's post-iteration progress
/// position. The callback must therefore be thread-safe.
pub fn write_heightmap_tileset_with_progress(
    source: &dyn RasterSource,
    grid: GlobalGeodeticGrid,
    output_directory: impl AsRef<Path>,
    options: HeightmapTilesetOptions,
    progress: Option<&(dyn Fn(TileWriteProgress) + Sync)>,
) -> Result<TilesetPlan, CtbError> {
    let metadata = source.metadata();
    let plan = TilesetPlan::from_raster_with_zoom_range(
        metadata,
        grid,
        options.start_zoom,
        options.end_zoom,
    )?;
    let source_pixel_bounds = metadata.transform.bounds(metadata.width, metadata.height)?;
    let source_bounds = transform_bounds(source_pixel_bounds, &metadata.crs, &grid.crs())?;
    // C++ TerrainTiler::createTile gates child masks on maxZoomLevel(),
    // which is the dataset's natural max zoom, independent of any
    // user-specified zoom range.  Compute it the same way.
    let max_zoom = grid.zoom_for_resolution(metadata.transform.pixel_width)?;
    let output_directory = output_directory.as_ref();
    let tiles = plan
        .levels
        .iter()
        .rev()
        .flat_map(|level| level.tiles.iter().copied())
        .collect::<Vec<_>>();
    let next_index = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let first_error = Mutex::new(None::<CtbError>);
    let total = tiles.len();
    let worker_count = options.worker_count.max(1).min(total.max(1));

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    if first_error.lock().is_ok_and(|error| error.is_some()) {
                        return;
                    }
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(tile) = tiles.get(index).copied() else {
                        return;
                    };
                    let path = terrain_path(output_directory, tile);
                    let outcome = if options.resume && path.exists() {
                        Ok(())
                    } else {
                        let heights = TerrainSamplePlan::new(grid, tile).and_then(|sample_plan| {
                            sample_plan.sample_heights(source, ResamplingMethod::Average)
                        });
                        heights
                            .and_then(|heights| {
                                let child_mask =
                                    terrain_child_mask(source_bounds, &grid, tile, max_zoom)?;
                                HeightmapTerrain::from_sampled_meters(&heights, child_mask)
                            })
                            .and_then(|terrain| write_terrain_atomically(&terrain, &path))
                    };
                    if let Err(error) = outcome {
                        if let Ok(mut slot) = first_error.lock()
                            && slot.is_none()
                        {
                            *slot = Some(error);
                        }
                        return;
                    }
                    let finished = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(progress) = progress {
                        progress(TileWriteProgress {
                            tile,
                            completed: finished,
                            total,
                        });
                    }
                }
            });
        }
    });
    if let Ok(mut slot) = first_error.lock()
        && let Some(error) = slot.take()
    {
        return Err(error);
    }
    Ok(plan)
}

/// Write tiles through a factory that opens an independent source per worker.
///
/// This is the CLI-facing counterpart of the shared-source API. It mirrors
/// original CTB, whose worker threads each call `GDALOpen` for the input.
pub fn write_heightmap_tileset_with_factory(
    source_factory: &(dyn Fn() -> Result<Box<dyn RasterSource>, CtbError> + Sync),
    grid: &dyn TileGrid,
    output_directory: impl AsRef<Path>,
    options: HeightmapTilesetOptions,
    progress: Option<&(dyn Fn(TileWriteProgress) + Sync)>,
) -> Result<TilesetPlan, CtbError> {
    let metadata_source = source_factory()?;
    let source_metadata = metadata_source.metadata();
    let plan = TilesetPlan::from_raster_with_tile_grid(
        source_metadata,
        grid,
        options.start_zoom,
        options.end_zoom,
    )?;
    let source_pixel_bounds = source_metadata
        .transform
        .bounds(source_metadata.width, source_metadata.height)?;
    let source_bounds = transform_bounds(source_pixel_bounds, &source_metadata.crs, &grid.crs())?;
    // C++ TerrainTiler::createTile gates child masks on maxZoomLevel(),
    // which is the dataset's natural max zoom from its resolution.  Match
    // the same formula as TilesetPlan::from_raster_with_tile_grid.
    let max_zoom = {
        let grid_crs = grid.crs();
        if source_metadata.crs == grid_crs {
            grid.zoom_for_resolution(source_metadata.transform.pixel_width.abs())?
        } else {
            grid.zoom_for_resolution(source_bounds.width() / f64::from(source_metadata.width))?
        }
    };
    drop(metadata_source);

    let output_directory = output_directory.as_ref();
    let tiles = plan
        .levels
        .iter()
        .rev()
        .flat_map(|level| level.tiles.iter().copied())
        .collect::<Vec<_>>();
    let next_index = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let first_error = Mutex::new(None::<CtbError>);
    let total = tiles.len();
    let worker_count = options.worker_count.max(1).min(total.max(1));

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                let source = match source_factory() {
                    Ok(source) => source,
                    Err(error) => {
                        if let Ok(mut slot) = first_error.lock()
                            && slot.is_none()
                        {
                            *slot = Some(error);
                        }
                        return;
                    }
                };
                loop {
                    if first_error.lock().is_ok_and(|error| error.is_some()) {
                        return;
                    }
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(tile) = tiles.get(index).copied() else {
                        return;
                    };
                    let path = terrain_path(output_directory, tile);
                    let outcome = if options.resume && path.exists() {
                        Ok(())
                    } else {
                        let heights =
                            TerrainSamplePlan::from_grid(grid, tile).and_then(|sample_plan| {
                                sample_plan
                                    .sample_heights(source.as_ref(), ResamplingMethod::Average)
                            });
                        heights
                            .and_then(|heights| {
                                let child_mask =
                                    terrain_child_mask(source_bounds, grid, tile, max_zoom)?;
                                HeightmapTerrain::from_sampled_meters(&heights, child_mask)
                            })
                            .and_then(|terrain| write_terrain_atomically(&terrain, &path))
                    };
                    if let Err(error) = outcome {
                        if let Ok(mut slot) = first_error.lock()
                            && slot.is_none()
                        {
                            *slot = Some(error);
                        }
                        return;
                    }
                    let finished = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(progress) = progress {
                        progress(TileWriteProgress {
                            tile,
                            completed: finished,
                            total,
                        });
                    }
                }
            });
        }
    });
    if let Ok(mut slot) = first_error.lock()
        && let Some(error) = slot.take()
    {
        return Err(error);
    }
    Ok(plan)
}

pub fn terrain_path(output_directory: impl AsRef<Path>, tile: TileCoord) -> PathBuf {
    output_directory
        .as_ref()
        .join(tile.zoom.to_string())
        .join(tile.x.to_string())
        .join(format!("{}.terrain", tile.y))
}

fn child_mask_from_tiles(parent: TileCoord, children: &BTreeSet<TileCoord>) -> ChildMask {
    let mut mask = ChildMask::empty();
    set_child_if_present(&mut mask, children, parent, 0, 0, ChildMask::SOUTH_WEST);
    set_child_if_present(&mut mask, children, parent, 1, 0, ChildMask::SOUTH_EAST);
    set_child_if_present(&mut mask, children, parent, 0, 1, ChildMask::NORTH_WEST);
    set_child_if_present(&mut mask, children, parent, 1, 1, ChildMask::NORTH_EAST);
    mask
}

fn write_terrain_atomically(terrain: &HeightmapTerrain, path: &Path) -> Result<(), CtbError> {
    let parent = path.parent().ok_or_else(|| {
        CtbError::TilesetIo(format!(
            "terrain path {} has no parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    let filename = path.file_name().ok_or_else(|| {
        CtbError::TilesetIo(format!("terrain path {} has no filename", path.display()))
    })?;
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        filename.to_string_lossy(),
        std::process::id()
    ));
    terrain
        .write_gzip(&temporary)
        .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    fs::rename(&temporary, path).map_err(|error| CtbError::TilesetIo(error.to_string()))
}

fn set_child_if_present(
    mask: &mut ChildMask,
    children: &BTreeSet<TileCoord>,
    parent: TileCoord,
    offset_x: u32,
    offset_y: u32,
    flag: u8,
) {
    let Some(zoom) = parent.zoom.checked_add(1) else {
        return;
    };
    let Some(x) = parent
        .x
        .checked_mul(2)
        .and_then(|value| value.checked_add(offset_x))
    else {
        return;
    };
    let Some(y) = parent
        .y
        .checked_mul(2)
        .and_then(|value| value.checked_add(offset_y))
    else {
        return;
    };
    mask.set(flag, children.contains(&TileCoord { zoom, x, y }));
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::{
        CtbError,
        grid::GlobalMercatorGrid,
        raster::{
            AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterSource, RasterWindow,
            WindowRequest,
        },
    };

    use super::*;

    fn metadata() -> Result<RasterMetadata, CtbError> {
        Ok(RasterMetadata {
            width: 20,
            height: 20,
            band_count: 1,
            crs: Crs::Epsg4326,
            transform: AffineTransform::north_up(-10.0, 10.0, 1.0, -1.0)?,
            no_data: None,
            sample_type: RasterSampleType::Float64,
        })
    }

    #[test]
    fn plans_direct_mercator_source_on_the_mercator_grid() -> Result<(), CtbError> {
        let metadata = RasterMetadata {
            width: 2,
            height: 2,
            band_count: 1,
            crs: Crs::Epsg3857,
            transform: AffineTransform::north_up(-100.0, 100.0, 100.0, -100.0)?,
            no_data: None,
            sample_type: RasterSampleType::Signed32,
        };
        let grid = GlobalMercatorGrid::new(256)?;
        let plan = TilesetPlan::from_raster_with_tile_grid(&metadata, &grid, Some(0), Some(0))?;
        assert_eq!(plan.max_zoom, 0);
        assert_eq!(
            plan.levels,
            vec![TilesetLevel {
                zoom: 0,
                tiles: vec![TileCoord {
                    zoom: 0,
                    x: 0,
                    y: 0,
                }],
            }]
        );
        Ok(())
    }

    #[test]
    fn plans_reprojected_source_on_the_mercator_grid() -> Result<(), CtbError> {
        let plan = TilesetPlan::from_raster_with_tile_grid(
            &metadata()?,
            &GlobalMercatorGrid::new(256)?,
            Some(0),
            Some(0),
        )?;
        assert_eq!(plan.max_zoom, 0);
        assert!(!plan.levels[0].tiles.is_empty());
        Ok(())
    }

    struct FlatRaster {
        metadata: RasterMetadata,
        value: f64,
    }

    impl FlatRaster {
        fn world() -> Result<Self, CtbError> {
            Ok(Self {
                metadata: RasterMetadata {
                    width: 65,
                    height: 65,
                    band_count: 1,
                    crs: Crs::Epsg4326,
                    transform: AffineTransform::north_up(-182.8125, 92.8125, 5.625, -2.8125)?,
                    no_data: None,
                    sample_type: RasterSampleType::Float64,
                },
                value: 100.0,
            })
        }
    }

    impl RasterSource for FlatRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            let width =
                usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
            let height =
                usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
            let count = width
                .checked_mul(height)
                .ok_or(CtbError::InvalidRasterWindow)?;
            Ok(RasterWindow {
                request,
                samples: vec![self.value; count],
            })
        }
    }

    #[test]
    fn plans_all_intersecting_tiles_through_maximum_zoom() -> Result<(), CtbError> {
        let plan = TilesetPlan::from_raster(&metadata()?, GlobalGeodeticGrid::new(65)?)?;
        assert_eq!(plan.max_zoom, 2);
        assert_eq!(plan.levels.len(), 3);
        assert_eq!(plan.levels[0].tiles.len(), 2);
        assert!(plan.levels[2].tiles.contains(&TileCoord {
            zoom: 2,
            x: 3,
            y: 1
        }));
        Ok(())
    }

    #[test]
    fn derives_children_from_the_actual_next_level() -> Result<(), CtbError> {
        let plan = TilesetPlan {
            max_zoom: 1,
            levels: vec![
                TilesetLevel {
                    zoom: 0,
                    tiles: vec![TileCoord {
                        zoom: 0,
                        x: 0,
                        y: 0,
                    }],
                },
                TilesetLevel {
                    zoom: 1,
                    tiles: vec![
                        TileCoord {
                            zoom: 1,
                            x: 0,
                            y: 0,
                        },
                        TileCoord {
                            zoom: 1,
                            x: 1,
                            y: 1,
                        },
                    ],
                },
            ],
        };
        let mask = plan.child_mask_for(TileCoord {
            zoom: 0,
            x: 0,
            y: 0,
        });
        assert!(mask.contains(ChildMask::SOUTH_WEST));
        assert!(mask.contains(ChildMask::NORTH_EAST));
        assert!(!mask.contains(ChildMask::SOUTH_EAST));
        assert!(!mask.contains(ChildMask::NORTH_WEST));
        Ok(())
    }

    #[test]
    fn restricts_levels_to_the_requested_zoom_range() -> Result<(), CtbError> {
        let plan = TilesetPlan::from_raster_with_zoom_range(
            &metadata()?,
            GlobalGeodeticGrid::new(65)?,
            Some(2),
            Some(1),
        )?;
        assert_eq!(plan.max_zoom, 2);
        assert_eq!(
            plan.levels
                .iter()
                .map(|level| level.zoom)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        Ok(())
    }

    #[test]
    fn coverage_child_mask_is_independent_of_the_output_zoom_range() -> Result<(), CtbError> {
        let coverage = TilesetPlan::from_raster(&metadata()?, GlobalGeodeticGrid::new(65)?)?;
        let mask = coverage.child_mask_for(TileCoord {
            zoom: 1,
            x: 1,
            y: 0,
        });
        assert_ne!(mask, ChildMask::empty());
        Ok(())
    }

    #[test]
    fn rejects_zoom_ranges_outside_the_source_resolution() -> Result<(), CtbError> {
        let error = TilesetPlan::from_raster_with_zoom_range(
            &metadata()?,
            GlobalGeodeticGrid::new(65)?,
            Some(3),
            Some(0),
        )
        .expect_err("the source only supports zoom two");
        assert_eq!(
            error,
            CtbError::InvalidZoomRange {
                start: 3,
                end: 0,
                maximum: 2,
            }
        );
        Ok(())
    }

    #[test]
    fn writes_ctb_paths_and_gzip_terrain_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::env::temp_dir().join(format!("ctb-rs-tileset-{}", std::process::id()));
        let source = FlatRaster::world()?;
        let plan =
            write_heightmap_tileset(&source, GlobalGeodeticGrid::new(65)?, &directory, false)?;
        assert_eq!(plan.max_zoom, 0);
        for tile in &plan.levels[0].tiles {
            let terrain = HeightmapTerrain::read_gzip(terrain_path(&directory, *tile))?;
            assert!(terrain.heights[0] == 5000 || terrain.heights[0] == 5500);
            assert_eq!(terrain.children, ChildMask::empty());
        }
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn resume_skips_existing_terrain_files() -> Result<(), Box<dyn std::error::Error>> {
        let directory =
            std::env::temp_dir().join(format!("ctb-rs-tileset-resume-{}", std::process::id()));
        let mut source = FlatRaster::world()?;
        write_heightmap_tileset(&source, GlobalGeodeticGrid::new(65)?, &directory, false)?;
        source.value = f64::NAN;
        write_heightmap_tileset(&source, GlobalGeodeticGrid::new(65)?, &directory, true)?;
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn factory_opens_one_source_for_metadata_and_each_worker()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory =
            std::env::temp_dir().join(format!("ctb-rs-tileset-factory-{}", std::process::id()));
        let opens = Arc::new(AtomicUsize::new(0));
        let factory_opens = Arc::clone(&opens);
        let factory = move || {
            factory_opens.fetch_add(1, Ordering::Relaxed);
            FlatRaster::world().map(|source| Box::new(source) as Box<dyn RasterSource>)
        };
        let grid = GlobalGeodeticGrid::new(65)?;
        write_heightmap_tileset_with_factory(
            &factory,
            &grid,
            &directory,
            HeightmapTilesetOptions {
                worker_count: 2,
                ..HeightmapTilesetOptions::default()
            },
            None,
        )?;
        assert_eq!(opens.load(Ordering::Relaxed), 3);
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
