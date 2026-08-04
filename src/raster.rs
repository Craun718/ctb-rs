use crate::{CtbError, grid::Bounds};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crs {
    Epsg4326,
    Epsg3857,
}

const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;

/// Transform one coordinate between the two CRS representations built into CTB.
pub fn transform_coordinate(
    x: f64,
    y: f64,
    source: &Crs,
    target: &Crs,
) -> Result<(f64, f64), CtbError> {
    if !x.is_finite() || !y.is_finite() {
        return Err(CtbError::InvalidBounds);
    }
    if source == target {
        return Ok((x, y));
    }
    match (source, target) {
        (Crs::Epsg4326, Crs::Epsg3857) => {
            let longitude = x.to_radians();
            let latitude = y.to_radians();
            Ok((
                WEB_MERCATOR_RADIUS * longitude,
                WEB_MERCATOR_RADIUS * (std::f64::consts::FRAC_PI_4 + latitude / 2.0).tan().ln(),
            ))
        }
        (Crs::Epsg3857, Crs::Epsg4326) => Ok((
            (x / WEB_MERCATOR_RADIUS).to_degrees(),
            (2.0 * (y / WEB_MERCATOR_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2)
                .to_degrees(),
        )),
        _ => Err(CtbError::UnsupportedCrs(format!(
            "cannot transform {source:?} to {target:?}"
        ))),
    }
}

/// Transform an axis-aligned bounds rectangle by transforming all four corners.
pub fn transform_bounds(bounds: Bounds, source: &Crs, target: &Crs) -> Result<Bounds, CtbError> {
    let corners = [
        (bounds.min_x, bounds.min_y),
        (bounds.min_x, bounds.max_y),
        (bounds.max_x, bounds.min_y),
        (bounds.max_x, bounds.max_y),
    ];
    let transformed = corners
        .into_iter()
        .map(|(x, y)| transform_coordinate(x, y, source, target))
        .collect::<Result<Vec<_>, _>>()?;
    let min_x = transformed
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::INFINITY, f64::min);
    let min_y = transformed
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::INFINITY, f64::min);
    let max_x = transformed
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = transformed
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max);
    Bounds::new(min_x, min_y, max_x, max_y)
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
    /// TIFF storage encoding retained for RasterTiler CreateCopy equivalents.
    /// Sampling continues to expose values as f64.
    pub sample_type: RasterSampleType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterSampleType {
    Unsigned8,
    Signed8,
    Unsigned16,
    Signed16,
    Unsigned32,
    Signed32,
    Float32,
    Float64,
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

    #[test]
    fn web_mercator_round_trip_uses_ctb_origin_shift() -> Result<(), CtbError> {
        let (x, y) = transform_coordinate(0.0, 0.0, &Crs::Epsg4326, &Crs::Epsg3857)?;
        assert!(x.abs() < 1e-12);
        assert!(y.abs() < 1e-8);
        let (longitude, latitude) = transform_coordinate(x, y, &Crs::Epsg3857, &Crs::Epsg4326)?;
        assert!((longitude - 0.0).abs() < 1e-12);
        assert!((latitude - 0.0).abs() < 1e-12);
        let (_, north_pole_limit) =
            transform_coordinate(0.0, 85.051_128_779_806_6, &Crs::Epsg4326, &Crs::Epsg3857)?;
        assert!((north_pole_limit - std::f64::consts::PI * WEB_MERCATOR_RADIUS).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn transform_bounds_includes_all_projected_corners() -> Result<(), CtbError> {
        let bounds = Bounds::new(-10.0, -10.0, 10.0, 10.0)?;
        let projected = transform_bounds(bounds, &Crs::Epsg4326, &Crs::Epsg3857)?;
        assert!(projected.min_x < 0.0 && projected.max_x > 0.0);
        assert!(projected.min_y < 0.0 && projected.max_y > 0.0);
        Ok(())
    }
}
