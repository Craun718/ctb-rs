use crate::CtbError;

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
            upper_right: TileCoord {
                zoom,
                x: Self::strict_upper_index(
                    clipped.max_x,
                    Self::WORLD_BOUNDS.min_x,
                    Self::WORLD_BOUNDS.width(),
                    self.tiles_x(zoom)?,
                ),
                y: Self::strict_upper_index(
                    clipped.max_y,
                    Self::WORLD_BOUNDS.min_y,
                    Self::WORLD_BOUNDS.height(),
                    self.tiles_y(zoom)?,
                ),
            },
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
        if tile.x >= self.tiles_x(tile.zoom)? || tile.y >= self.tiles_y(tile.zoom)? {
            return Err(CtbError::CoordinateOutsideGrid {
                x: f64::from(tile.x),
                y: f64::from(tile.y),
            });
        }
        Ok(())
    }

    fn index_at(value: f64, min: f64, span: f64, count: u32) -> u32 {
        let raw = ((value - min) / span * f64::from(count)).floor();
        let max_index = f64::from(count - 1);
        raw.clamp(0.0, max_index) as u32
    }

    fn strict_upper_index(value: f64, min: f64, span: f64, count: u32) -> u32 {
        let raw = (value - min) / span * f64::from(count);
        let whole = raw.floor();
        let index = if raw == whole && whole > 0.0 {
            whole - 1.0
        } else {
            whole
        };
        index.clamp(0.0, f64::from(count - 1)) as u32
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
    fn maximum_world_edge_maps_to_last_tile() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        assert_eq!(
            grid.coordinate_to_tile(180.0, 90.0, 3)?,
            TileCoord {
                zoom: 3,
                x: 15,
                y: 7
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
    fn area_ranges_exclude_tiles_only_touched_at_the_upper_edge() -> Result<(), CtbError> {
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
                x: 0,
                y: 0
            }
        );
        Ok(())
    }
}
