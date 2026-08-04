use crate::{CtbError, raster::Crs};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Bounds {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Result<Self, CtbError> {
        if !min_x.is_finite()
            || !min_y.is_finite()
            || !max_x.is_finite()
            || !max_y.is_finite()
            || min_x >= max_x
            || min_y >= max_y
        {
            return Err(CtbError::InvalidBounds);
        }

        Ok(Self {
            min_x,
            min_y,
            max_x,
            max_y,
        })
    }

    pub fn width(self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        Self::new(
            self.min_x.max(other.min_x),
            self.min_y.max(other.min_y),
            self.max_x.min(other.max_x),
            self.max_y.min(other.max_y),
        )
        .ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileCoord {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRange {
    pub lower_left: TileCoord,
    pub upper_right: TileCoord,
}

/// The shared CTB `Grid` contract used by raster tilers.
///
/// Concrete profiles retain their original coordinate units: degrees for
/// Global Geodetic and metres for Global Mercator.
pub trait TileGrid: Send + Sync {
    fn crs(&self) -> Crs;
    fn tile_size(&self) -> u32;
    fn resolution(&self, zoom: u8) -> Result<f64, CtbError>;
    fn zoom_for_resolution(&self, resolution: f64) -> Result<u8, CtbError>;
    fn tiles_x(&self, zoom: u8) -> Result<u32, CtbError>;
    fn tiles_y(&self, zoom: u8) -> Result<u32, CtbError>;
    fn tile_bounds(&self, tile: TileCoord) -> Result<Bounds, CtbError>;
    fn coordinate_to_tile(&self, x: f64, y: f64, zoom: u8) -> Result<TileCoord, CtbError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalGeodeticGrid {
    tile_size: u32,
}

impl GlobalGeodeticGrid {
    pub const WORLD_BOUNDS: Bounds = Bounds {
        min_x: -180.0,
        min_y: -90.0,
        max_x: 180.0,
        max_y: 90.0,
    };

    pub fn new(tile_size: u32) -> Result<Self, CtbError> {
        if tile_size < 2 {
            return Err(CtbError::InvalidTileSize(tile_size));
        }
        Ok(Self { tile_size })
    }

    pub fn tile_size(self) -> u32 {
        self.tile_size
    }

    pub fn resolution(self, zoom: u8) -> Result<f64, CtbError> {
        let scale = self.scale(zoom)?;
        // Match CTB Grid::resolution: compute the initial resolution first,
        // then divide it by the zoom factor.
        let initial_resolution = (360.0 / 2.0) / f64::from(self.tile_size);
        Ok(initial_resolution / f64::from(scale))
    }

    pub fn zoom_for_resolution(self, resolution: f64) -> Result<u8, CtbError> {
        if !resolution.is_finite() || resolution <= 0.0 {
            return Err(CtbError::InvalidBounds);
        }
        let initial_resolution = self.resolution(0)?;
        let zoom = (initial_resolution / resolution).log2().ceil().max(0.0);
        if zoom > 30.0 {
            return Err(CtbError::InvalidZoom(31));
        }
        Ok(zoom as u8)
    }

    pub fn tile_range_for_area(self, bounds: Bounds, zoom: u8) -> Result<TileRange, CtbError> {
        let clipped = bounds
            .intersection(Self::WORLD_BOUNDS)
            .ok_or(CtbError::RasterOutsideGrid)?;
        Ok(TileRange {
            lower_left: self.coordinate_to_tile(clipped.min_x, clipped.min_y, zoom)?,
            upper_right: self.coordinate_to_tile(clipped.max_x, clipped.max_y, zoom)?,
        })
    }

    pub fn tiles_x(self, zoom: u8) -> Result<u32, CtbError> {
        self.scale(zoom)?
            .checked_mul(2)
            .ok_or(CtbError::InvalidZoom(zoom))
    }

    pub fn tiles_y(self, zoom: u8) -> Result<u32, CtbError> {
        self.scale(zoom)
    }

    pub fn tile_bounds(self, tile: TileCoord) -> Result<Bounds, CtbError> {
        self.validate_tile(tile)?;
        // Match CTB Grid::tileBounds: it converts tile pixel corners through
        // resolution(), rather than deriving a tile width directly.
        let resolution = self.resolution(tile.zoom)?;
        let lower_pixel_x = tile
            .x
            .checked_mul(self.tile_size)
            .ok_or(CtbError::InvalidZoom(tile.zoom))?;
        let lower_pixel_y = tile
            .y
            .checked_mul(self.tile_size)
            .ok_or(CtbError::InvalidZoom(tile.zoom))?;
        let upper_pixel_x = tile
            .x
            .checked_add(1)
            .and_then(|value| value.checked_mul(self.tile_size))
            .ok_or(CtbError::InvalidZoom(tile.zoom))?;
        let upper_pixel_y = tile
            .y
            .checked_add(1)
            .and_then(|value| value.checked_mul(self.tile_size))
            .ok_or(CtbError::InvalidZoom(tile.zoom))?;
        Bounds::new(
            f64::from(lower_pixel_x) * resolution - 180.0,
            f64::from(lower_pixel_y) * resolution - 90.0,
            f64::from(upper_pixel_x) * resolution - 180.0,
            f64::from(upper_pixel_y) * resolution - 90.0,
        )
    }

    pub fn coordinate_to_tile(self, x: f64, y: f64, zoom: u8) -> Result<TileCoord, CtbError> {
        if !x.is_finite()
            || !y.is_finite()
            || !(-180.0..=180.0).contains(&x)
            || !(-90.0..=90.0).contains(&y)
        {
            return Err(CtbError::CoordinateOutsideGrid { x, y });
        }

        let tiles_x = self.tiles_x(zoom)?;
        let tiles_y = self.tiles_y(zoom)?;
        let tile_x = Self::index_at(x, -180.0, 360.0, tiles_x);
        let tile_y = Self::index_at(y, -90.0, 180.0, tiles_y);
        Ok(TileCoord {
            zoom,
            x: tile_x,
            y: tile_y,
        })
    }

    pub fn tile_range_for_bounds(self, bounds: Bounds, zoom: u8) -> Result<TileRange, CtbError> {
        let lower_left = self.coordinate_to_tile(bounds.min_x, bounds.min_y, zoom)?;
        let upper_right = self.coordinate_to_tile(bounds.max_x, bounds.max_y, zoom)?;
        Ok(TileRange {
            lower_left,
            upper_right,
        })
    }

    fn scale(self, zoom: u8) -> Result<u32, CtbError> {
        1_u32
            .checked_shl(u32::from(zoom))
            .ok_or(CtbError::InvalidZoom(zoom))
    }

    fn validate_tile(self, tile: TileCoord) -> Result<(), CtbError> {
        if tile.x > self.tiles_x(tile.zoom)? || tile.y > self.tiles_y(tile.zoom)? {
            return Err(CtbError::CoordinateOutsideGrid {
                x: f64::from(tile.x),
                y: f64::from(tile.y),
            });
        }
        Ok(())
    }

    fn index_at(value: f64, min: f64, span: f64, count: u32) -> u32 {
        let raw = ((value - min) / span * f64::from(count)).floor();
        raw as u32
    }
}

impl TileGrid for GlobalGeodeticGrid {
    fn crs(&self) -> Crs {
        Crs::Epsg4326
    }

    fn tile_size(&self) -> u32 {
        (*self).tile_size()
    }

    fn resolution(&self, zoom: u8) -> Result<f64, CtbError> {
        (*self).resolution(zoom)
    }

    fn zoom_for_resolution(&self, resolution: f64) -> Result<u8, CtbError> {
        (*self).zoom_for_resolution(resolution)
    }

    fn tiles_x(&self, zoom: u8) -> Result<u32, CtbError> {
        (*self).tiles_x(zoom)
    }

    fn tiles_y(&self, zoom: u8) -> Result<u32, CtbError> {
        (*self).tiles_y(zoom)
    }

    fn tile_bounds(&self, tile: TileCoord) -> Result<Bounds, CtbError> {
        (*self).tile_bounds(tile)
    }

    fn coordinate_to_tile(&self, x: f64, y: f64, zoom: u8) -> Result<TileCoord, CtbError> {
        (*self).coordinate_to_tile(x, y, zoom)
    }
}

/// CTB's TMS Global Mercator grid in EPSG:3857 metres.
///
/// This mirrors the cpp `GlobalMercator` constructor: one root tile, a
/// power-of-two zoom factor, and an extent of ±π×6378137 metres.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalMercatorGrid {
    tile_size: u32,
}

impl GlobalMercatorGrid {
    pub const SEMI_MAJOR_AXIS: f64 = 6_378_137.0;
    pub const ORIGIN_SHIFT: f64 = std::f64::consts::PI * Self::SEMI_MAJOR_AXIS;

    pub fn new(tile_size: u32) -> Result<Self, CtbError> {
        if tile_size < 2 {
            return Err(CtbError::InvalidTileSize(tile_size));
        }
        Ok(Self { tile_size })
    }

    pub fn tile_size(self) -> u32 {
        self.tile_size
    }

    pub fn extent(self) -> Bounds {
        // Constants above are finite and strictly ordered by construction.
        Bounds::new(
            -Self::ORIGIN_SHIFT,
            -Self::ORIGIN_SHIFT,
            Self::ORIGIN_SHIFT,
            Self::ORIGIN_SHIFT,
        )
        .expect("GlobalMercator origin shift defines a non-empty finite extent")
    }

    pub fn resolution(self, zoom: u8) -> Result<f64, CtbError> {
        let scale = self.scale(zoom)?;
        Ok((2.0 * Self::ORIGIN_SHIFT / f64::from(self.tile_size)) / f64::from(scale))
    }

    pub fn zoom_for_resolution(self, resolution: f64) -> Result<u8, CtbError> {
        if !resolution.is_finite() || resolution <= 0.0 {
            return Err(CtbError::InvalidBounds);
        }
        let initial_resolution = self.resolution(0)?;
        let zoom = (initial_resolution / resolution).log2().ceil().max(0.0);
        if zoom > 30.0 {
            return Err(CtbError::InvalidZoom(31));
        }
        Ok(zoom as u8)
    }

    pub fn tiles_x(self, zoom: u8) -> Result<u32, CtbError> {
        self.scale(zoom)
    }

    pub fn tiles_y(self, zoom: u8) -> Result<u32, CtbError> {
        self.scale(zoom)
    }

    pub fn tile_bounds(self, tile: TileCoord) -> Result<Bounds, CtbError> {
        self.validate_tile(tile)?;
        let resolution = self.resolution(tile.zoom)?;
        let min_x = f64::from(
            tile.x
                .checked_mul(self.tile_size)
                .ok_or(CtbError::InvalidZoom(tile.zoom))?,
        ) * resolution
            - Self::ORIGIN_SHIFT;
        let min_y = f64::from(
            tile.y
                .checked_mul(self.tile_size)
                .ok_or(CtbError::InvalidZoom(tile.zoom))?,
        ) * resolution
            - Self::ORIGIN_SHIFT;
        let max_x = min_x + f64::from(self.tile_size) * resolution;
        let max_y = min_y + f64::from(self.tile_size) * resolution;
        Bounds::new(min_x, min_y, max_x, max_y)
    }

    pub fn coordinate_to_tile(self, x: f64, y: f64, zoom: u8) -> Result<TileCoord, CtbError> {
        if !x.is_finite()
            || !y.is_finite()
            || !(-Self::ORIGIN_SHIFT..=Self::ORIGIN_SHIFT).contains(&x)
            || !(-Self::ORIGIN_SHIFT..=Self::ORIGIN_SHIFT).contains(&y)
        {
            return Err(CtbError::CoordinateOutsideGrid { x, y });
        }
        let tiles = self.scale(zoom)?;
        Ok(TileCoord {
            zoom,
            x: Self::index_at(x, tiles),
            y: Self::index_at(y, tiles),
        })
    }

    fn scale(self, zoom: u8) -> Result<u32, CtbError> {
        1_u32
            .checked_shl(u32::from(zoom))
            .ok_or(CtbError::InvalidZoom(zoom))
    }

    fn validate_tile(self, tile: TileCoord) -> Result<(), CtbError> {
        let tiles = self.scale(tile.zoom)?;
        if tile.x > tiles || tile.y > tiles {
            return Err(CtbError::CoordinateOutsideGrid {
                x: f64::from(tile.x),
                y: f64::from(tile.y),
            });
        }
        Ok(())
    }

    fn index_at(value: f64, tiles: u32) -> u32 {
        ((value + Self::ORIGIN_SHIFT) / (2.0 * Self::ORIGIN_SHIFT) * f64::from(tiles)).floor()
            as u32
    }
}

impl TileGrid for GlobalMercatorGrid {
    fn crs(&self) -> Crs {
        Crs::Epsg3857
    }

    fn tile_size(&self) -> u32 {
        (*self).tile_size()
    }

    fn resolution(&self, zoom: u8) -> Result<f64, CtbError> {
        (*self).resolution(zoom)
    }

    fn zoom_for_resolution(&self, resolution: f64) -> Result<u8, CtbError> {
        (*self).zoom_for_resolution(resolution)
    }

    fn tiles_x(&self, zoom: u8) -> Result<u32, CtbError> {
        (*self).tiles_x(zoom)
    }

    fn tiles_y(&self, zoom: u8) -> Result<u32, CtbError> {
        (*self).tiles_y(zoom)
    }

    fn tile_bounds(&self, tile: TileCoord) -> Result<Bounds, CtbError> {
        (*self).tile_bounds(tile)
    }

    fn coordinate_to_tile(&self, x: f64, y: f64, zoom: u8) -> Result<TileCoord, CtbError> {
        (*self).coordinate_to_tile(x, y, zoom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_geodetic_zoom_zero_has_two_tiles() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        assert_eq!(grid.tiles_x(0)?, 2);
        assert_eq!(grid.tiles_y(0)?, 1);
        assert_eq!(grid.resolution(0)?, 180.0 / 65.0);
        assert_eq!(
            grid.tile_bounds(TileCoord {
                zoom: 0,
                x: 0,
                y: 0
            })?,
            Bounds::new(-180.0, -90.0, 0.0, 90.0)?
        );
        assert_eq!(
            grid.tile_bounds(TileCoord {
                zoom: 0,
                x: 1,
                y: 0
            })?,
            Bounds::new(0.0, -90.0, 180.0, 90.0)?
        );
        Ok(())
    }

    #[test]
    fn maximum_world_edge_maps_to_the_ctb_outer_tile() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        assert_eq!(
            grid.coordinate_to_tile(180.0, 90.0, 3)?,
            TileCoord {
                zoom: 3,
                x: 16,
                y: 8
            }
        );
        Ok(())
    }

    #[test]
    fn tile_bounds_follow_ctb_pixel_to_crs_coordinates() -> Result<(), CtbError> {
        let bounds = GlobalGeodeticGrid::new(65)?.tile_bounds(TileCoord {
            zoom: 1,
            x: 1,
            y: 0,
        })?;
        assert!(bounds.max_x.abs() <= f64::EPSILON);
        assert!(bounds.max_y.abs() <= f64::EPSILON);
        Ok(())
    }

    #[test]
    fn area_ranges_include_tiles_touched_at_the_upper_edge() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        let range = grid.tile_range_for_area(Bounds::new(-180.0, -90.0, 0.0, 90.0)?, 0)?;
        assert_eq!(
            range.lower_left,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0
            }
        );
        assert_eq!(
            range.upper_right,
            TileCoord {
                zoom: 0,
                x: 1,
                y: 1
            }
        );
        Ok(())
    }

    #[test]
    fn global_mercator_matches_ctb_root_grid_constants() -> Result<(), CtbError> {
        let grid = GlobalMercatorGrid::new(256)?;
        assert_eq!(grid.tile_size(), 256);
        assert_eq!(grid.tiles_x(0)?, 1);
        assert_eq!(grid.tiles_y(0)?, 1);
        assert_eq!(
            grid.resolution(0)?,
            2.0 * GlobalMercatorGrid::ORIGIN_SHIFT / 256.0
        );
        assert_eq!(grid.resolution(1)?, grid.resolution(0)? / 2.0);
        assert_eq!(
            grid.tile_bounds(TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            })?,
            grid.extent()
        );
        Ok(())
    }

    #[test]
    fn global_mercator_uses_ctb_tms_bottom_left_tile_coordinates() -> Result<(), CtbError> {
        let grid = GlobalMercatorGrid::new(256)?;
        let origin = GlobalMercatorGrid::ORIGIN_SHIFT;
        assert_eq!(
            grid.coordinate_to_tile(-origin / 2.0, -origin / 2.0, 1)?,
            TileCoord {
                zoom: 1,
                x: 0,
                y: 0,
            }
        );
        assert_eq!(
            grid.coordinate_to_tile(origin / 2.0, origin / 2.0, 1)?,
            TileCoord {
                zoom: 1,
                x: 1,
                y: 1,
            }
        );
        assert_eq!(
            grid.tile_bounds(TileCoord {
                zoom: 1,
                x: 1,
                y: 1,
            })?,
            Bounds::new(0.0, 0.0, origin, origin)?
        );
        Ok(())
    }

    #[test]
    fn tile_grid_contract_preserves_each_cpp_profile_crs_and_geometry() -> Result<(), CtbError> {
        let geodetic = GlobalGeodeticGrid::new(65)?;
        let mercator = GlobalMercatorGrid::new(256)?;
        let grids: [(&dyn TileGrid, Crs, u32, TileCoord); 2] = [
            (
                &geodetic,
                Crs::Epsg4326,
                65,
                TileCoord {
                    zoom: 0,
                    x: 1,
                    y: 0,
                },
            ),
            (
                &mercator,
                Crs::Epsg3857,
                256,
                TileCoord {
                    zoom: 0,
                    x: 0,
                    y: 0,
                },
            ),
        ];
        for (grid, crs, tile_size, origin_tile) in grids {
            assert_eq!(grid.crs(), crs);
            assert_eq!(grid.tile_size(), tile_size);
            assert!(grid.resolution(0)? > 0.0);
            assert_eq!(grid.coordinate_to_tile(0.0, 0.0, 0)?, origin_tile);
        }
        Ok(())
    }
}
