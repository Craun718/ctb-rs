use std::path::Path;

use oxigeo::{
    GeoTransform, RasterDataType,
    core_types::types::NoDataValue,
    geotiff::{
        GeoTiffWriter, GeoTiffWriterOptions, OverviewResampling, WriterConfig,
        tiff::{Compression, Predictor},
        writer::BigTiffMode,
    },
};

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
    let width = u64::try_from(side).map_err(|_| CtbError::InvalidRasterDimensions {
        width: metadata.width,
        height: metadata.height,
    })?;
    let height = u64::try_from(side).map_err(|_| CtbError::InvalidRasterDimensions {
        width: metadata.width,
        height: metadata.height,
    })?;
    let (data_type, bytes) = typed_samples(metadata.sample_type, &values)?;
    let mut config = WriterConfig::new(width, height, 1, data_type)
        .with_compression(compression)
        .with_predictor(match options.predictor {
            Some(RasterGeoTiffPredictor::None) | None => Predictor::None,
            Some(RasterGeoTiffPredictor::HorizontalDifferencing) => {
                Predictor::HorizontalDifferencing
            }
            Some(RasterGeoTiffPredictor::FloatingPoint) => Predictor::FloatingPoint,
        })
        .with_overviews(false, OverviewResampling::Average)
        .with_geo_transform(GeoTransform::north_up(
            bounds.min_x,
            bounds.max_y,
            plan.resolution(),
            -plan.resolution(),
        ))
        .with_epsg_code(epsg);
    if let Some((tile_width, tile_height)) = options.tile_size {
        config = config.with_tile_size(tile_width, tile_height);
    } else {
        config.tile_width = None;
        config.tile_height = None;
    }
    if let Some(no_data) = metadata.no_data {
        config = config.with_nodata(NoDataValue::from_float(no_data));
    }
    let writer_options = GeoTiffWriterOptions {
        bigtiff_mode: match options.tiff_variant {
            RasterGeoTiffTiffVariant::Classic => BigTiffMode::Disable,
            RasterGeoTiffTiffVariant::BigTiff => BigTiffMode::Force,
            RasterGeoTiffTiffVariant::Auto => BigTiffMode::Auto,
        },
        ..GeoTiffWriterOptions::default()
    };
    let mut writer = GeoTiffWriter::create(path, config, writer_options)
        .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    writer
        .write(&bytes)
        .map_err(|error| CtbError::TilesetIo(error.to_string()))
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

fn typed_samples(
    sample_type: RasterSampleType,
    values: &[f64],
) -> Result<(RasterDataType, Vec<u8>), CtbError> {
    let bytes = match sample_type {
        RasterSampleType::Unsigned8 => {
            let samples = integral_values::<u8>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Signed8 => {
            let samples = integral_values::<i8>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Unsigned16 => {
            let samples = integral_values::<u16>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Signed16 => {
            let samples = integral_values::<i16>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Unsigned32 => {
            let samples = integral_values::<u32>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Signed32 => {
            let samples = integral_values::<i32>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Float32 => {
            let samples = float_values::<f32>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
        RasterSampleType::Float64 => {
            let samples = float_values::<f64>(values)?;
            samples
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        }
    };
    let data_type = match sample_type {
        RasterSampleType::Unsigned8 => RasterDataType::UInt8,
        RasterSampleType::Signed8 => RasterDataType::Int8,
        RasterSampleType::Unsigned16 => RasterDataType::UInt16,
        RasterSampleType::Signed16 => RasterDataType::Int16,
        RasterSampleType::Unsigned32 => RasterDataType::UInt32,
        RasterSampleType::Signed32 => RasterDataType::Int32,
        RasterSampleType::Float32 => RasterDataType::Float32,
        RasterSampleType::Float64 => RasterDataType::Float64,
    };
    Ok((data_type, bytes))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use oxigeo::{core_types::io::FileDataSource, geotiff::GeoTiffReader};

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
        let file = GeoTiffReader::open(
            FileDataSource::open(&path).map_err(|error| CtbError::RasterRead(error.to_string()))?,
        )
        .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        assert_eq!(file.width(), 4);
        assert_eq!(file.height(), 4);
        assert_eq!(file.epsg_code(), Some(4326));
        assert_eq!(file.nodata().as_f64(), Some(-9999.0));
        let transform = file.geo_transform().ok_or(CtbError::InvalidBounds)?;
        assert_eq!(transform.origin_x, -180.0);
        assert_eq!(transform.origin_y, 90.0);
        assert_eq!(transform.pixel_width, 45.0);
        assert_eq!(transform.pixel_height, -45.0);
        let mut samples = vec![0_i32; 16];
        file.read_window_into_typed::<i32>(0, 0, 0, 0, 4, 4, &mut samples)
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
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
