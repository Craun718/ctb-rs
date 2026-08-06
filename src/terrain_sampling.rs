use crate::{
    CtbError,
    grid::{Bounds, GlobalGeodeticGrid, TileCoord, TileGrid},
    raster::{RasterSource, transform_bounds, transform_coordinate},
    sampling::{ResamplingMethod, sample_with_footprint_level},
    terrain::HEIGHTMAP_TILE_SIZE,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSample {
    pub world_x: f64,
    pub world_y: f64,
    pub footprint: Bounds,
}

/// Plans the source-space pixels of one CTB terrain heightmap.
///
/// CTB creates the warp VRT with one pixel of west and north overlap.  Its
/// public GeoTransform is subsequently shifted back to the tile bounds, but
/// the warp transformer keeps the overlapped source-space pixels.  Therefore
/// a heightmap value represents the centre of that VRT pixel, not a node on
/// the nominal tile bounds.
///
/// The grid tile size and the heightmap tile size are independent: C++ uses
/// the profile default (65 for geodetic, 256 for mercator) for the grid, but
/// always reads `TILE_SIZE` (65) heights from the VRT (`config.hpp`).  For
/// mercator terrain the VRT is 256x256 but only the upper-left 65x65 pixels
/// are read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSamplePlan {
    bounds: Bounds,
    grid_tile_size: u32,
    heightmap_size: u32,
    cell_width: f64,
    cell_height: f64,
    target_crs: crate::raster::Crs,
}

impl TerrainSamplePlan {
    pub fn new(grid: GlobalGeodeticGrid, coord: TileCoord) -> Result<Self, CtbError> {
        Self::from_grid(&grid, coord)
    }

    pub fn from_grid(grid: &dyn TileGrid, coord: TileCoord) -> Result<Self, CtbError> {
        let bounds = grid.tile_bounds(coord)?;
        let grid_tile_size = grid.tile_size();
        let cells_per_edge = f64::from(grid_tile_size - 1);
        Ok(Self {
            bounds,
            grid_tile_size,
            heightmap_size: HEIGHTMAP_TILE_SIZE as u32,
            cell_width: bounds.width() / cells_per_edge,
            cell_height: bounds.height() / cells_per_edge,
            target_crs: grid.crs(),
        })
    }

    pub fn heightmap_size(self) -> u32 {
        self.heightmap_size
    }

    pub fn sample(self, row: u32, column: u32) -> Option<TerrainSample> {
        if row >= self.heightmap_size || column >= self.heightmap_size {
            return None;
        }
        let world_x = self.bounds.min_x + self.cell_width * (f64::from(column) - 0.5);
        let world_y = self.bounds.max_y - self.cell_height * (f64::from(row) - 0.5);
        let half_width = self.cell_width / 2.0;
        let half_height = self.cell_height / 2.0;
        Some(TerrainSample {
            world_x,
            world_y,
            footprint: Bounds::new(
                world_x - half_width,
                world_y - half_height,
                world_x + half_width,
                world_y + half_height,
            )
            .ok()?,
        })
    }

    pub fn sample_heights(
        self,
        source: &dyn RasterSource,
        method: ResamplingMethod,
    ) -> Result<Vec<f64>, CtbError> {
        // This mirrors the `GDALSuggestedWarpOutput2` ratio used by CTB's
        // overview chooser after reprojection to the source CRS.
        let target_ratio = 1.0 / source.metadata().transform.pixel_width;
        let level = source.sampling_level_for_ratio(target_ratio)?;
        let mut heights = Vec::new();
        for row in 0..self.heightmap_size {
            for column in 0..self.heightmap_size {
                let sample = self
                    .sample(row, column)
                    .ok_or(CtbError::InvalidRasterWindow)?;
                let (world_x, world_y) = transform_coordinate(
                    sample.world_x,
                    sample.world_y,
                    &self.target_crs,
                    &source.metadata().crs,
                )?;
                let footprint =
                    transform_bounds(sample.footprint, &self.target_crs, &source.metadata().crs)?;
                heights.push(sample_with_footprint_level(
                    source, &level, world_x, world_y, footprint, method,
                )?);
            }
        }
        Ok(heights)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CtbError,
        raster::{
            AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterWindow, WindowRequest,
        },
    };

    use super::*;
    use crate::sampling::sample_with_footprint;

    struct TestRaster {
        metadata: RasterMetadata,
    }

    impl RasterSource for TestRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            let value = f64::from(request.y * 2 + request.x);
            Ok(RasterWindow {
                request,
                samples: vec![value],
            })
        }
    }

    #[test]
    fn neighbouring_tiles_share_east_west_and_north_south_samples() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        let west = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let east = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 1,
                y: 0,
            },
        )?;
        // FMA-contracted tile_bounds means adjacent edges differ by ~4.4e-15.
        let wx_west = west
            .sample(0, 64)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_x;
        let wx_east = east
            .sample(0, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_x;
        assert!((wx_west - wx_east).abs() < 1e-10);

        let south = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 1,
                x: 0,
                y: 0,
            },
        )?;
        let north = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 1,
                x: 0,
                y: 1,
            },
        )?;
        let wy_south = south
            .sample(0, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_y;
        let wy_north = north
            .sample(64, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_y;
        assert!((wy_south - wy_north).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn heightmap_size_is_always_65_regardless_of_grid_tile_size() -> Result<(), CtbError> {
        let geodetic = GlobalGeodeticGrid::new(65)?;
        let plan_geo = TerrainSamplePlan::new(
            geodetic,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan_geo.heightmap_size(), HEIGHTMAP_TILE_SIZE as u32);

        let mercator = crate::grid::GlobalMercatorGrid::new(256)?;
        let plan_merc = TerrainSamplePlan::from_grid(
            &mercator,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan_merc.heightmap_size(), HEIGHTMAP_TILE_SIZE as u32);
        Ok(())
    }

    #[test]
    fn matches_ctb_west_and_north_overlap_at_a_source_pixel_edge() -> Result<(), CtbError> {
        let source = TestRaster {
            metadata: RasterMetadata {
                width: 2,
                height: 2,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: AffineTransform::north_up(-1.0, 1.0, 1.0, -1.0)?,
                no_data: None,
                sample_type: RasterSampleType::Float64,
            },
        };
        let grid = GlobalGeodeticGrid::new(65)?;
        let plan = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;

        let upper = plan.sample(32, 64).ok_or(CtbError::InvalidRasterWindow)?;
        let lower = plan.sample(33, 64).ok_or(CtbError::InvalidRasterWindow)?;
        assert_eq!(upper.footprint, Bounds::new(-2.8125, 0.0, 0.0, 2.8125)?);
        assert_eq!(lower.footprint, Bounds::new(-2.8125, -2.8125, 0.0, 0.0)?);
        assert_eq!(
            sample_with_footprint(
                &source,
                upper.world_x,
                upper.world_y,
                upper.footprint,
                ResamplingMethod::Average,
            )?,
            0.0,
        );
        assert_eq!(
            sample_with_footprint(
                &source,
                lower.world_x,
                lower.world_y,
                lower.footprint,
                ResamplingMethod::Average,
            )?,
            2.0,
        );
        Ok(())
    }
}
