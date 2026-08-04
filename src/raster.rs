use crate::{CtbError, grid::Bounds};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crs {
    Epsg4326,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineTransform {
    pub origin_x: f64,
    pub pixel_width: f64,
    pub row_rotation: f64,
    pub origin_y: f64,
    pub column_rotation: f64,
    pub pixel_height: f64,
}

impl AffineTransform {
    pub fn north_up(
        origin_x: f64,
        origin_y: f64,
        pixel_width: f64,
        pixel_height: f64,
    ) -> Result<Self, CtbError> {
        if !origin_x.is_finite()
            || !origin_y.is_finite()
            || !pixel_width.is_finite()
            || !pixel_height.is_finite()
            || pixel_width <= 0.0
            || pixel_height >= 0.0
        {
            return Err(CtbError::InvalidBounds);
        }
        Ok(Self {
            origin_x,
            pixel_width,
            row_rotation: 0.0,
            origin_y,
            column_rotation: 0.0,
            pixel_height,
        })
    }

    pub fn bounds(self, width: u32, height: u32) -> Result<Bounds, CtbError> {
        if width == 0 || height == 0 {
            return Err(CtbError::InvalidRasterDimensions { width, height });
        }
        if self.row_rotation != 0.0 || self.column_rotation != 0.0 {
            return Err(CtbError::UnsupportedCrs(
                "rotated affine transforms are not supported".to_owned(),
            ));
        }
        Bounds::new(
            self.origin_x,
            self.origin_y + self.pixel_height * f64::from(height),
            self.origin_x + self.pixel_width * f64::from(width),
            self.origin_y,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterMetadata {
    pub width: u32,
    pub height: u32,
    pub band_count: u16,
    pub crs: Crs,
    pub transform: AffineTransform,
    pub no_data: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRequest {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub overview: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterWindow {
    pub request: WindowRequest,
    pub samples: Vec<f64>,
}

/// The raster resolution used for sampling one terrain tile.
///
/// `metadata()` on a source always describes its base dataset for tile
/// planning. A sampling level may instead describe one internal overview.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplingLevel {
    pub level: u16,
    pub metadata: RasterMetadata,
}

pub trait RasterSource: Send + Sync {
    fn metadata(&self) -> &RasterMetadata;
    fn overview_count(&self) -> u16;
    fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError>;

    fn sampling_level_for_ratio(&self, _target_ratio: f64) -> Result<SamplingLevel, CtbError> {
        Ok(SamplingLevel {
            level: 0,
            metadata: self.metadata().clone(),
        })
    }

    fn read_sampling_window(
        &self,
        level: &SamplingLevel,
        request: WindowRequest,
    ) -> Result<RasterWindow, CtbError> {
        if level.level != 0 {
            return Err(CtbError::UnsupportedRaster(
                "the raster source does not expose overview sampling".to_owned(),
            ));
        }
        self.read_window(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn north_up_transform_returns_dataset_bounds() -> Result<(), CtbError> {
        let transform = AffineTransform::north_up(-180.0, 90.0, 0.5, -0.5)?;
        assert_eq!(
            transform.bounds(720, 360)?,
            Bounds::new(-180.0, -90.0, 180.0, 90.0)?
        );
        Ok(())
    }
}
