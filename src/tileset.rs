use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    CtbError,
    grid::{GlobalGeodeticGrid, TileCoord},
    raster::{RasterMetadata, RasterSource},
    sampling::ResamplingMethod,
    terrain::{ChildMask, HEIGHTMAP_TILE_SIZE, HeightmapTerrain},
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
}

impl Default for HeightmapTilesetOptions {
    fn default() -> Self {
        Self {
            resume: false,
            start_zoom: None,
            end_zoom: None,
            resampling: ResamplingMethod::Average,
        }
    }
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
            for y in range.lower_left.y..=range.upper_right.y {
                for x in range.lower_left.x..=range.upper_right.x {
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
    if grid.tile_size() != HEIGHTMAP_TILE_SIZE as u32 {
        return Err(CtbError::UnsupportedRaster(format!(
            "CTB heightmap tiles require a {HEIGHTMAP_TILE_SIZE} pixel grid"
        )));
    }
    let plan = TilesetPlan::from_raster_with_zoom_range(
        source.metadata(),
        grid,
        options.start_zoom,
        options.end_zoom,
    )?;
    let coverage_plan = TilesetPlan::from_raster(source.metadata(), grid)?;
    let output_directory = output_directory.as_ref();

    for level in plan.levels.iter().rev() {
        for tile in &level.tiles {
            let path = terrain_path(output_directory, *tile);
            if options.resume && path.exists() {
                continue;
            }

            let heights =
                TerrainSamplePlan::new(grid, *tile)?.sample_heights(source, options.resampling)?;
            let terrain = HeightmapTerrain::from_sampled_meters(
                &heights,
                coverage_plan.child_mask_for(*tile),
            )?;
            write_terrain_atomically(&terrain, &path)?;
        }
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
    use crate::{
        CtbError,
        raster::{AffineTransform, Crs, RasterMetadata, RasterSource, RasterWindow, WindowRequest},
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
        })
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
            Ok(RasterWindow {
                request,
                samples: vec![self.value],
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
            assert_eq!(terrain.heights[0], 5500);
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
}
