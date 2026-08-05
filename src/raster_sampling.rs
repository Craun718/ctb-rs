use crate::{
    CtbError,
    grid::{Bounds, GlobalGeodeticGrid, TileCoord, TileGrid},
    raster::{Crs, RasterSource, transform_bounds, transform_coordinate},
    sampling::{ResamplingMethod, sample_with_footprint_raster_tiler_level},
};

/// One destination cell of a CTB `RasterTiler` warped VRT.
///
/// Unlike heightmap terrain sampling, RasterTiler has no shared edge samples:
/// the VRT consists of ordinary pixel-centre samples and their pixel-area
/// footprints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterTileSample {
    pub world_x: f64,
    pub world_y: f64,
    pub footprint: Bounds,
}

/// Destination pixel geometry for one CTB `RasterTiler` tile.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterTileSamplePlan {
    tile_size: u32,
    tile: TileCoord,
    bounds: Bounds,
    resolution: f64,
    target_crs: Crs,
}

impl RasterTileSamplePlan {
    pub fn new(grid: GlobalGeodeticGrid, tile: TileCoord) -> Result<Self, CtbError> {
        Self::from_grid(&grid, tile)
    }

    pub fn from_grid(grid: &dyn TileGrid, tile: TileCoord) -> Result<Self, CtbError> {
        let bounds = grid.tile_bounds(tile)?;
        let resolution = grid.resolution(tile.zoom)?;
        Ok(Self {
            tile_size: grid.tile_size(),
            tile,
            bounds,
            resolution,
            target_crs: grid.crs(),
        })
    }

    pub fn tile(&self) -> TileCoord {
        self.tile
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    pub fn resolution(&self) -> f64 {
        self.resolution
    }

    pub fn tile_size(&self) -> u32 {
        self.tile_size
    }

    pub fn sample(&self, row: u32, column: u32) -> Option<RasterTileSample> {
        if row >= self.tile_size || column >= self.tile_size {
            return None;
        }
        let min_x = self.bounds.min_x + f64::from(column) * self.resolution;
        let max_x = min_x + self.resolution;
        let max_y = self.bounds.max_y - f64::from(row) * self.resolution;
        let min_y = max_y - self.resolution;
        Some(RasterTileSample {
            world_x: (min_x + max_x) / 2.0,
            world_y: (min_y + max_y) / 2.0,
            footprint: Bounds::new(min_x, min_y, max_x, max_y).expect(
                "grid resolution and validated tile bounds form a non-empty destination cell",
            ),
        })
    }

    pub fn sample_values(
        &self,
        source: &dyn RasterSource,
        method: ResamplingMethod,
    ) -> Result<Vec<f64>, CtbError> {
        let side = usize::try_from(self.tile_size)
            .map_err(|_| CtbError::InvalidTileSize(self.tile_size))?;
        let capacity = side
            .checked_mul(side)
            .ok_or(CtbError::InvalidTileSize(self.tile_size))?;
        let mut values = Vec::with_capacity(capacity);
        let target_ratio = self.resolution / source.metadata().transform.pixel_width.abs();
        let level = source.sampling_level_for_ratio(target_ratio)?;
        for row in 0..self.tile_size {
            for column in 0..self.tile_size {
                let point = self.sample(row, column).expect(
                    "row and column bounded by tile size always identify a destination cell",
                );
                let (world_x, world_y) = transform_coordinate(
                    point.world_x,
                    point.world_y,
                    &self.target_crs,
                    &source.metadata().crs,
                )?;
                let footprint =
                    transform_bounds(point.footprint, &self.target_crs, &source.metadata().crs)?;
                values.push(sample_with_footprint_raster_tiler_level(
                    source, &level, world_x, world_y, footprint, method,
                )?);
            }
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        grid::GlobalMercatorGrid,
        raster::{
            AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterSource, RasterWindow,
            SamplingLevel, WindowRequest,
        },
    };

    use super::*;

    #[test]
    fn follows_ctb_vrt_geotransform_pixel_centres() -> Result<(), CtbError> {
        let plan = RasterTileSamplePlan::new(
            GlobalGeodeticGrid::new(4)?,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan.bounds(), Bounds::new(-180.0, -90.0, 0.0, 90.0)?);
        assert_eq!(plan.resolution(), 45.0);
        assert_eq!(
            plan.sample(0, 0),
            Some(RasterTileSample {
                world_x: -157.5,
                world_y: 67.5,
                footprint: Bounds::new(-180.0, 45.0, -135.0, 90.0)?,
            })
        );
        assert_eq!(
            plan.sample(3, 3),
            Some(RasterTileSample {
                world_x: -22.5,
                world_y: -67.5,
                footprint: Bounds::new(-45.0, -90.0, 0.0, -45.0)?,
            })
        );
        assert_eq!(plan.sample(4, 0), None);
        assert_eq!(plan.sample(0, 4), None);
        Ok(())
    }

    #[test]
    fn adjacent_tiles_have_contiguous_destination_geotransforms() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(4)?;
        let west = RasterTileSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let east = RasterTileSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 1,
                y: 0,
            },
        )?;
        assert_eq!(west.bounds().max_x, east.bounds().min_x);
        assert_eq!(
            west.sample(0, 3).expect("in-bounds cell").footprint.max_x,
            0.0
        );
        assert_eq!(
            east.sample(0, 0).expect("in-bounds cell").footprint.min_x,
            0.0
        );
        Ok(())
    }

    #[test]
    fn can_plan_cpp_global_mercator_destination_cells() -> Result<(), CtbError> {
        let grid = GlobalMercatorGrid::new(256)?;
        let plan = RasterTileSamplePlan::from_grid(
            &grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan.tile_size(), 256);
        assert_eq!(plan.bounds(), grid.extent());
        assert_eq!(plan.resolution(), grid.resolution(0)?);
        let top_left = plan.sample(0, 0).expect("in-bounds Mercator cell");
        assert_eq!(top_left.footprint.max_y, GlobalMercatorGrid::ORIGIN_SHIFT);
        assert_eq!(top_left.footprint.min_x, -GlobalMercatorGrid::ORIGIN_SHIFT);
        Ok(())
    }

    struct OverviewOnlySource {
        base: RasterMetadata,
        overview: SamplingLevel,
    }

    impl RasterSource for OverviewOnlySource {
        fn metadata(&self) -> &RasterMetadata {
            &self.base
        }

        fn overview_count(&self) -> u16 {
            1
        }

        fn read_window(&self, _request: WindowRequest) -> Result<RasterWindow, CtbError> {
            Err(CtbError::RasterRead(
                "base-level read was used instead of the overview".to_owned(),
            ))
        }

        fn sampling_level_for_ratio(&self, _target_ratio: f64) -> Result<SamplingLevel, CtbError> {
            Ok(self.overview.clone())
        }

        fn read_sampling_window(
            &self,
            level: &SamplingLevel,
            request: WindowRequest,
        ) -> Result<RasterWindow, CtbError> {
            if level.level != 1 {
                return Err(CtbError::RasterRead(
                    "RasterTiler did not retain the selected overview level".to_owned(),
                ));
            }
            Ok(RasterWindow {
                request,
                samples: vec![42.0],
            })
        }
    }

    #[test]
    fn sample_values_reuses_the_selected_overview_level() -> Result<(), CtbError> {
        let base_transform = AffineTransform::north_up(-180.0, 90.0, 180.0, -90.0)?;
        let overview_transform = AffineTransform::north_up(-180.0, 90.0, 360.0, -180.0)?;
        let source = OverviewOnlySource {
            base: RasterMetadata {
                width: 2,
                height: 2,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: base_transform,
                no_data: None,
                sample_type: RasterSampleType::Float64,
            },
            overview: SamplingLevel {
                level: 1,
                data_width: 1,
                data_height: 1,
                metadata: RasterMetadata {
                    width: 1,
                    height: 1,
                    band_count: 1,
                    crs: Crs::Epsg4326,
                    transform: overview_transform,
                    no_data: None,
                    sample_type: RasterSampleType::Float64,
                },
            },
        };
        let plan = RasterTileSamplePlan::new(
            GlobalGeodeticGrid::new(2)?,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let values = plan.sample_values(&source, ResamplingMethod::Nearest)?;
        assert_eq!(values, vec![42.0; 4]);
        Ok(())
    }
}
