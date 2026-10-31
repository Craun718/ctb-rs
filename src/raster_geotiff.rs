use std::path::Path;

use geotiff_core::GeoTransform as GeotiffRustTransform;
use geotiff_writer::{Compression, GeoTiffBuilder, Predictor, TiffVariant};
use ndarray::Array2;

use crate::{
    CtbError,
    raster::{Crs, RasterMetadata, RasterSampleType},
    raster_sampling::RasterTileSamplePlan,
};

/// Write one RasterTiler destination raster with its source storage type.
///
/// This is deliberately a file-format boundary only. Sampling, output paths,
/// creation options, and CLI dispatch remain outside this function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterGeoTiffCompression {
    None,
    Deflate,
    Lzw,
    Zstd,
    Jpeg,
    Lerc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterGeoTiffTiffVariant {
    Classic,
    BigTiff,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterGeoTiffPredictor {
    None,
    HorizontalDifferencing,
    FloatingPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterGeoTiffWriteOptions {
    pub compression: RasterGeoTiffCompression,
    pub tiff_variant: RasterGeoTiffTiffVariant,
    pub predictor: Option<RasterGeoTiffPredictor>,
    pub tile_size: Option<(u32, u32)>,
}

impl Default for RasterGeoTiffWriteOptions {
    fn default() -> Self {
        Self {
            compression: RasterGeoTiffCompression::None,
            tiff_variant: RasterGeoTiffTiffVariant::Auto,
            predictor: None,
            tile_size: None,
        }
    }
}

pub fn write_raster_tile_as_geotiff(
    path: impl AsRef<Path>,
    plan: &RasterTileSamplePlan,
    metadata: &RasterMetadata,
    values: Vec<f64>,
) -> Result<(), CtbError> {
    write_raster_tile_as_geotiff_with_compression(
        path,
        plan,
        metadata,
        values,
        RasterGeoTiffCompression::None,
    )
}

pub fn write_raster_tile_as_geotiff_with_compression(
    path: impl AsRef<Path>,
    plan: &RasterTileSamplePlan,
    metadata: &RasterMetadata,
    values: Vec<f64>,
    compression: RasterGeoTiffCompression,
) -> Result<(), CtbError> {
    write_raster_tile_as_geotiff_with_options(
        path,
        plan,
        metadata,
        values,
        RasterGeoTiffWriteOptions {
            compression,
            ..RasterGeoTiffWriteOptions::default()
        },
    )
}

pub fn write_raster_tile_as_geotiff_with_options(
    path: impl AsRef<Path>,
    plan: &RasterTileSamplePlan,
    metadata: &RasterMetadata,
    values: Vec<f64>,
    options: RasterGeoTiffWriteOptions,
) -> Result<(), CtbError> {
    let side =
        usize::try_from(plan.tile_size()).map_err(|_| CtbError::InvalidRasterDimensions {
            width: metadata.width,
            height: metadata.height,
        })?;
    let expected = side
        .checked_mul(side)
        .ok_or(CtbError::InvalidRasterDimensions {
            width: metadata.width,
            height: metadata.height,
        })?;
    if values.len() != expected {
        return Err(CtbError::RasterRead(format!(
            "RasterTiler sample plan requires {expected} values, got {}",
            values.len()
        )));
    }

    let compression = match options.compression {
        RasterGeoTiffCompression::None => Compression::None,
        RasterGeoTiffCompression::Deflate => Compression::Deflate,
        RasterGeoTiffCompression::Lzw => Compression::Lzw,
        RasterGeoTiffCompression::Zstd => Compression::Zstd,
        RasterGeoTiffCompression::Jpeg => {
            return Err(CtbError::UnsupportedRaster(
                "COMPRESS=JPEG is not implemented by the OxiGeo GeoTIFF writer".to_owned(),
            ));
        }
        RasterGeoTiffCompression::Lerc => {
            return Err(CtbError::UnsupportedRaster(
                "COMPRESS=LERC is not implemented by the OxiGeo GeoTIFF writer".to_owned(),
            ));
        }
    };
    validate_predictor(metadata.sample_type, options.predictor)?;

    let bounds = plan.bounds();
    let epsg = match metadata.crs {
        Crs::Epsg4326 => 4326,
        Crs::Epsg3857 => 3857,
        Crs::Epsg(code) => {
            return Err(CtbError::UnsupportedCrs(format!(
                "output raster CRS EPSG:{code} is not a CTB grid CRS"
            )));
        }
    };
    let side_u32 = u32::try_from(side).map_err(|_| CtbError::InvalidRasterDimensions {
        width: metadata.width,
        height: metadata.height,
    })?;
    let mut builder = GeoTiffBuilder::new(side_u32, side_u32)
        .epsg(epsg)
        .transform(GeotiffRustTransform::from_origin_and_pixel_size(
            bounds.min_x,
            bounds.max_y,
            plan.resolution(),
            -plan.resolution(),
        ))
        .compression(compression)
        .predictor(match options.predictor {
            Some(RasterGeoTiffPredictor::None) | None => Predictor::None,
            Some(RasterGeoTiffPredictor::HorizontalDifferencing) => Predictor::Horizontal,
            Some(RasterGeoTiffPredictor::FloatingPoint) => Predictor::FloatingPoint,
        })
        .tiff_variant(match options.tiff_variant {
            RasterGeoTiffTiffVariant::Classic => TiffVariant::Classic,
            RasterGeoTiffTiffVariant::BigTiff => TiffVariant::BigTiff,
            RasterGeoTiffTiffVariant::Auto => TiffVariant::Auto,
        });
    if let Some((tile_width, tile_height)) = options.tile_size {
        builder = builder.tile_size(tile_width, tile_height);
    }
    if let Some(no_data) = metadata.no_data {
        builder = builder.nodata(&no_data.to_string());
    }
    write_typed(builder, metadata.sample_type, side, values, path)
}

fn validate_predictor(
    sample_type: RasterSampleType,
    predictor: Option<RasterGeoTiffPredictor>,
) -> Result<(), CtbError> {
    match (predictor, sample_type) {
        (
            Some(RasterGeoTiffPredictor::FloatingPoint),
            RasterSampleType::Unsigned8
            | RasterSampleType::Signed8
            | RasterSampleType::Unsigned16
            | RasterSampleType::Signed16
            | RasterSampleType::Unsigned32
            | RasterSampleType::Signed32,
        ) => Err(CtbError::UnsupportedRaster(
            "PREDICTOR=3 requires a floating-point raster sample type".to_owned(),
        )),
        (
            Some(RasterGeoTiffPredictor::HorizontalDifferencing),
            RasterSampleType::Float32 | RasterSampleType::Float64,
        ) => Err(CtbError::UnsupportedRaster(
            "PREDICTOR=2 requires an integer raster sample type".to_owned(),
        )),
        _ => Ok(()),
    }
}

trait IntegralSample: TryFrom<i128> {
    const NAME: &'static str;
    const MINIMUM: f64;
    const MAXIMUM: f64;
}

macro_rules! integral_sample {
    ($type:ty, $name:literal) => {
        impl IntegralSample for $type {
            const NAME: &'static str = $name;
            const MINIMUM: f64 = <$type>::MIN as f64;
            const MAXIMUM: f64 = <$type>::MAX as f64;
        }
    };
}

integral_sample!(u8, "u8");
integral_sample!(i8, "i8");
integral_sample!(u16, "u16");
integral_sample!(i16, "i16");
integral_sample!(u32, "u32");
integral_sample!(i32, "i32");

fn integral_values<T: IntegralSample>(values: &[f64]) -> Result<Vec<T>, CtbError> {
    values
        .iter()
        .copied()
        .map(|value| {
            if !value.is_finite() {
                return Err(CtbError::UnsupportedRaster(format!(
                    "non-finite value {value} cannot be converted to {}",
                    T::NAME
                )));
            }
            // Match GDALWarpKernel::ClampRoundAndAvoidNoData: clamp to the
            // integer destination range, then use floor(value + 0.5).
            let integer = value
                .clamp(T::MINIMUM, T::MAXIMUM)
                .mul_add(1.0, 0.5)
                .floor() as i128;
            T::try_from(integer).map_err(|_| {
                CtbError::UnsupportedRaster(format!(
                    "GDAL-rounded value {value} lies outside the {} output range",
                    T::NAME
                ))
            })
        })
        .collect()
}

trait FloatSample: Sized {
    fn convert(value: f64) -> Result<Self, CtbError>;
}

impl FloatSample for f32 {
    fn convert(value: f64) -> Result<Self, CtbError> {
        if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
            return Err(CtbError::UnsupportedRaster(format!(
                "value {value} cannot be represented by f32 output"
            )));
        }
        Ok(value as f32)
    }
}

impl FloatSample for f64 {
    fn convert(value: f64) -> Result<Self, CtbError> {
        if !value.is_finite() {
            return Err(CtbError::UnsupportedRaster(format!(
                "value {value} cannot be represented by f64 output"
            )));
        }
        Ok(value)
    }
}

fn float_values<T: FloatSample>(values: &[f64]) -> Result<Vec<T>, CtbError> {
    values.iter().copied().map(T::convert).collect()
}

fn write_typed(
    builder: GeoTiffBuilder,
    sample_type: RasterSampleType,
    side: usize,
    values: Vec<f64>,
    path: impl AsRef<Path>,
) -> Result<(), CtbError> {
    let dimensions = (side, side);
    let result = match sample_type {
        RasterSampleType::Unsigned8 => {
            write_array(builder, dimensions, integral_values::<u8>(&values)?, path)
        }
        RasterSampleType::Signed8 => {
            write_array(builder, dimensions, integral_values::<i8>(&values)?, path)
        }
        RasterSampleType::Unsigned16 => {
            write_array(builder, dimensions, integral_values::<u16>(&values)?, path)
        }
        RasterSampleType::Signed16 => {
            write_array(builder, dimensions, integral_values::<i16>(&values)?, path)
        }
        RasterSampleType::Unsigned32 => {
            write_array(builder, dimensions, integral_values::<u32>(&values)?, path)
        }
        RasterSampleType::Signed32 => {
            write_array(builder, dimensions, integral_values::<i32>(&values)?, path)
        }
        RasterSampleType::Float32 => {
            write_array(builder, dimensions, float_values::<f32>(&values)?, path)
        }
        RasterSampleType::Float64 => {
            write_array(builder, dimensions, float_values::<f64>(&values)?, path)
        }
    };
    result.map_err(|error| CtbError::TilesetIo(error.to_string()))
}

fn write_array<T: geotiff_writer::NumericSample>(
    builder: GeoTiffBuilder,
    dimensions: (usize, usize),
    values: Vec<T>,
    path: impl AsRef<Path>,
) -> Result<(), geotiff_writer::Error> {
    let data = Array2::from_shape_vec(dimensions, values)
        .map_err(|error| geotiff_writer::Error::Other(error.to_string()))?;
    builder.write_2d(path, data.view())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use geotiff_reader::GeoTiffFile;

    use crate::{
        grid::{GlobalGeodeticGrid, TileCoord},
        raster::{AffineTransform, Crs},
    };

    use super::*;

    fn metadata(sample_type: RasterSampleType) -> Result<RasterMetadata, CtbError> {
        Ok(RasterMetadata {
            width: 2,
            height: 2,
            band_count: 1,
            crs: Crs::Epsg4326,
            transform: AffineTransform::north_up(-1.0, 1.0, 1.0, -1.0)?,
            no_data: Some(-9999.0),
            sample_type,
        })
    }

    #[test]
    fn writes_int32_geotiff_with_rastertiler_georeferencing() -> Result<(), CtbError> {
        let path =
            std::env::temp_dir().join(format!("ctb-rs-raster-geotiff-{}.tif", std::process::id()));
        let plan = RasterTileSamplePlan::new(
            GlobalGeodeticGrid::new(4)?,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        write_raster_tile_as_geotiff(
            &path,
            &plan,
            &metadata(RasterSampleType::Signed32)?,
            vec![
                0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0,
                15.0,
            ],
        )?;
        let file =
            GeoTiffFile::open(&path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        assert_eq!(file.width(), 4);
        assert_eq!(file.height(), 4);
        assert_eq!(file.epsg(), Some(4326));
        assert_eq!(file.nodata(), Some("-9999"));
        let transform = file.transform().ok_or(CtbError::InvalidBounds)?;
        assert_eq!(transform.origin_x, -180.0);
        assert_eq!(transform.origin_y, 90.0);
        assert_eq!(transform.pixel_width, 45.0);
        assert_eq!(transform.pixel_height, -45.0);
        let samples = file
            .tiff()
            .read_window::<i32>(file.base_ifd_index(), 0, 0, 4, 4)
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let samples = samples
            .as_slice()
            .ok_or_else(|| CtbError::RasterRead("GeoTIFF raster is not contiguous".to_owned()))?;
        assert_eq!(
            samples,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        fs::remove_file(path).map_err(|error| CtbError::TilesetIo(error.to_string()))?;
        Ok(())
    }

    #[test]
    fn rounds_and_clamps_integer_values_like_gdal_warp() -> Result<(), CtbError> {
        assert_eq!(
            integral_values::<i16>(&[1.5, -1.5, -1.6, 50_000.0, -50_000.0])?,
            vec![2, -1, -2, i16::MAX, i16::MIN]
        );
        assert_eq!(
            integral_values::<u8>(&[-1.0, 254.6, 300.0])?,
            vec![0, 255, 255]
        );
        Ok(())
    }

    #[test]
    fn rejects_incompatible_predictor_for_sample_type() -> Result<(), CtbError> {
        assert!(
            validate_predictor(
                RasterSampleType::Float64,
                Some(RasterGeoTiffPredictor::HorizontalDifferencing)
            )
            .is_err()
        );
        assert!(
            validate_predictor(
                RasterSampleType::Signed16,
                Some(RasterGeoTiffPredictor::FloatingPoint)
            )
            .is_err()
        );
        assert!(
            validate_predictor(
                RasterSampleType::Float64,
                Some(RasterGeoTiffPredictor::FloatingPoint)
            )
            .is_ok()
        );
        assert!(
            validate_predictor(
                RasterSampleType::Signed16,
                Some(RasterGeoTiffPredictor::HorizontalDifferencing)
            )
            .is_ok()
        );
        Ok(())
    }
}
