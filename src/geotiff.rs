use std::path::Path;

use oxigeo::{
    DatasetFormat, RasterDataType,
    core_types::io::FileDataSource,
    geotiff::{CogReader, GeoTiffReader, RasterType},
    open::open,
    vrt::{PixelRect, VrtReader, resolve_crs},
};

use crate::{
    CtbError,
    raster::{
        AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterSource, RasterWindow,
        WindowRequest,
    },
};

enum RasterData {
    GeoTiff(GeoTiffReader<FileDataSource>),
    Vrt(VrtReader),
}

/// A restricted, pure-Rust GeoTIFF/VRT source for the direct-source input contract.
///
/// It accepts one north-up band in an EPSG CRS resolvable by proj4rs.
/// Reprojection remains at the sampling-plan boundary; overview selection is
/// level-aware.
pub struct GeoTiffRasterSource {
    data: RasterData,
    metadata: RasterMetadata,
}

impl GeoTiffRasterSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CtbError> {
        let path = path.as_ref();
        let detected = open(path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        match detected.format() {
            DatasetFormat::GeoTiff => {
                let data_source = FileDataSource::open(path)
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                let file = GeoTiffReader::open(data_source)
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                Self::from_geotiff(file, path)
            }
            DatasetFormat::Vrt => {
                let file = VrtReader::open(path)
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                Self::from_vrt(file)
            }
            format => Err(CtbError::UnsupportedRaster(format!(
                "OxiGeo 0.2.3 detected {format:?} but has no pixel reader for it; \
                 supported raster inputs are GeoTIFF and VRT"
            ))),
        }
    }

    fn from_geotiff(file: GeoTiffReader<FileDataSource>, path: &Path) -> Result<Self, CtbError> {
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }
        let transform = file.geo_transform().ok_or_else(|| {
            CtbError::UnsupportedRaster("missing GeoTIFF affine transform".to_owned())
        })?;
        let source =
            FileDataSource::open(path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let cog =
            CogReader::open(source).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let pixel_is_point =
            cog.geo_keys().and_then(|keys| keys.raster_type()) == Some(RasterType::PixelIsPoint);
        let transform = if pixel_is_point {
            &shift_pixel_is_point_transform(transform)
        } else {
            transform
        };
        let sample_type = raster_sample_type(file.data_type().ok_or_else(|| {
            CtbError::UnsupportedRaster("GeoTIFF has no sample type".to_owned())
        })?)?;
        let epsg = file.epsg_code().ok_or(CtbError::MissingCrs)?;
        let (width, height) = raster_dimensions(file.width(), file.height())?;
        let metadata = build_metadata(
            width,
            height,
            crs_from_epsg(epsg)?,
            transform,
            file.nodata().as_f64(),
            sample_type,
        )?;
        Ok(Self {
            data: RasterData::GeoTiff(file),
            metadata,
        })
    }

    fn from_vrt(file: VrtReader) -> Result<Self, CtbError> {
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }
        let transform = file.geo_transform().ok_or_else(|| {
            CtbError::UnsupportedRaster("missing VRT affine transform".to_owned())
        })?;
        let data_type = file
            .band_data_type(1)
            .or_else(|| file.primary_data_type())
            .ok_or_else(|| CtbError::UnsupportedRaster("VRT has no band sample type".to_owned()))?;
        let sample_type = raster_sample_type(data_type)?;
        let srs = file.srs().ok_or(CtbError::MissingCrs)?;
        let resolved = resolve_crs(srs).map_err(|error| {
            CtbError::UnsupportedCrs(format!("VRT SRS {srs:?} cannot be resolved: {error}"))
        })?;
        let epsg = resolved.epsg_code().ok_or_else(|| {
            CtbError::UnsupportedCrs(format!(
                "VRT SRS {srs:?} does not expose an EPSG code usable by proj4rs"
            ))
        })?;
        let (width, height) = raster_dimensions(file.width(), file.height())?;
        let metadata = build_metadata(
            width,
            height,
            crs_from_epsg(epsg)?,
            transform,
            file.band_nodata(1).as_f64(),
            sample_type,
        )?;
        Ok(Self {
            data: RasterData::Vrt(file),
            metadata,
        })
    }

    fn validate_window(
        &self,
        metadata: &RasterMetadata,
        request: WindowRequest,
    ) -> Result<(), CtbError> {
        let end_x = request
            .x
            .checked_add(request.width)
            .ok_or(CtbError::InvalidRasterWindow)?;
        let end_y = request
            .y
            .checked_add(request.height)
            .ok_or(CtbError::InvalidRasterWindow)?;
        if request.width == 0
            || request.height == 0
            || end_x > metadata.width
            || end_y > metadata.height
        {
            return Err(CtbError::InvalidRasterWindow);
        }
        Ok(())
    }

    fn read_samples(&self, level: u16, request: WindowRequest) -> Result<Vec<f64>, CtbError> {
        let width = usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
        let height = usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
        let count = width
            .checked_mul(height)
            .ok_or(CtbError::InvalidRasterWindow)?;
        let mut samples = vec![0.0_f64; count];
        let x = u64::from(request.x);
        let y = u64::from(request.y);
        let width_u64 = u64::from(request.width);
        let height_u64 = u64::from(request.height);
        match &self.data {
            RasterData::GeoTiff(file) => file
                .read_window_into_typed::<f64>(
                    usize::from(level),
                    0,
                    x,
                    y,
                    width_u64,
                    height_u64,
                    &mut samples,
                )
                .map_err(|error| CtbError::RasterRead(error.to_string()))?,
            RasterData::Vrt(file) => {
                if level != 0 {
                    return Err(CtbError::UnsupportedRaster(
                        "VRT inputs have no overview levels".to_owned(),
                    ));
                }
                let buffer = file
                    .read_window(1, PixelRect::new(x, y, width_u64, height_u64))
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                buffer
                    .copy_to_slice(&mut samples)
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
            }
        }
        Ok(samples)
    }

    fn level_size(&self, level: usize) -> Result<(u64, u64), CtbError> {
        match &self.data {
            RasterData::GeoTiff(file) => file
                .level_size(level)
                .map_err(|error| CtbError::RasterRead(error.to_string())),
            RasterData::Vrt(_) => Err(CtbError::UnsupportedRaster(
                "VRT inputs have no overview levels".to_owned(),
            )),
        }
    }
}

impl RasterSource for GeoTiffRasterSource {
    fn metadata(&self) -> &RasterMetadata {
        &self.metadata
    }

    fn overview_count(&self) -> u16 {
        match &self.data {
            RasterData::GeoTiff(file) => {
                u16::try_from(file.overview_count()).map_or(u16::MAX, |count| count)
            }
            RasterData::Vrt(_) => 0,
        }
    }

    fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
        self.validate_window(&self.metadata, request)?;
        // GDALCreateWarpedVRT does not set padfSrcNoDataReal, so the warp
        // kernel treats NoData pixels as regular values (density=1.0).
        // We return raw pixel values without NaN conversion.
        let samples = self.read_samples(0, request)?;
        Ok(RasterWindow { request, samples })
    }

    fn sampling_level_for_ratio(
        &self,
        target_ratio: f64,
    ) -> Result<crate::raster::SamplingLevel, CtbError> {
        let overview_count = self.overview_count();
        if !target_ratio.is_finite() || target_ratio <= 1.0 || overview_count == 0 {
            return Ok(crate::raster::SamplingLevel {
                level: 0,
                data_width: self.metadata.width,
                data_height: self.metadata.height,
                metadata: self.metadata.clone(),
            });
        }

        let mut selected: i32 = -1;
        for overview in -1..i32::from(overview_count - 1) {
            let ratio = if overview < 0 {
                1.0
            } else {
                let level = usize::try_from(overview)
                    .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?
                    + 1;
                f64::from(self.metadata.width) / self.level_size(level)?.0 as f64
            };
            let next_level = usize::try_from(overview + 1)
                .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?
                + 1;
            let next_ratio = f64::from(self.metadata.width) / self.level_size(next_level)?.0 as f64;
            if (ratio < target_ratio && next_ratio > target_ratio)
                || (ratio - target_ratio).abs() < 0.1
            {
                selected = overview;
                break;
            }
            selected = overview + 1;
        }
        if selected < 0 {
            return Ok(crate::raster::SamplingLevel {
                level: 0,
                data_width: self.metadata.width,
                data_height: self.metadata.height,
                metadata: self.metadata.clone(),
            });
        }

        let index = usize::try_from(selected)
            .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?;
        let (overview_width, overview_height) = self.level_size(index + 1)?;
        let width = raster_dimension(overview_width, overview_height)?;
        let height = raster_dimension(overview_height, overview_width)?;
        let metadata = RasterMetadata {
            width,
            height,
            band_count: self.metadata.band_count,
            crs: self.metadata.crs,
            transform: AffineTransform::north_up(
                self.metadata.transform.origin_x,
                self.metadata.transform.origin_y,
                self.metadata.transform.pixel_width * f64::from(self.metadata.width)
                    / f64::from(width),
                self.metadata.transform.pixel_height * f64::from(self.metadata.height)
                    / f64::from(height),
            )?,
            no_data: self.metadata.no_data,
            sample_type: self.metadata.sample_type,
        };
        Ok(crate::raster::SamplingLevel {
            // C++ GDALTiler::createRasterTile recreates the transformer from the
            // overview dataset but never updates psWarpOptions->hSrcDS, so the
            // warp kernel reads from the base dataset at overview pixel indices.
            // level 0 (base IFD) preserves overview metadata for coordinate math
            // while reading from the base band, matching the C++ oracle exactly.
            level: 0,
            data_width: self.metadata.width,
            data_height: self.metadata.height,
            metadata,
        })
    }

    fn read_sampling_window(
        &self,
        level: &crate::raster::SamplingLevel,
        request: WindowRequest,
    ) -> Result<RasterWindow, CtbError> {
        let data_metadata = RasterMetadata {
            width: level.data_width,
            height: level.data_height,
            ..level.metadata.clone()
        };
        self.validate_window(&data_metadata, request)?;
        let samples = self.read_samples(level.level, request)?;
        Ok(RasterWindow { request, samples })
    }
}

fn raster_dimension(width: u64, height: u64) -> Result<u32, CtbError> {
    u32::try_from(width).map_err(|_| CtbError::InvalidRasterDimensions {
        width: u32::try_from(width).unwrap_or(u32::MAX),
        height: u32::try_from(height).unwrap_or(u32::MAX),
    })
}

fn raster_dimensions(width: u64, height: u64) -> Result<(u32, u32), CtbError> {
    Ok((
        raster_dimension(width, height)?,
        raster_dimension(height, width)?,
    ))
}

/// GDAL applies the GeoTIFF `PixelIsPoint` half-pixel offset on read
/// (`gtiffdataset_read.cpp` `LoadGeoreferencingAndPamIfNeeded`), so the affine
/// origin becomes the upper-left corner of the first pixel instead of its
/// center.
fn shift_pixel_is_point_transform(transform: &oxigeo::GeoTransform) -> oxigeo::GeoTransform {
    oxigeo::GeoTransform::north_up(
        transform.origin_x - transform.pixel_width * 0.5,
        transform.origin_y - transform.pixel_height * 0.5,
        transform.pixel_width,
        transform.pixel_height,
    )
}

fn build_metadata(
    width: u32,
    height: u32,
    crs: Crs,
    transform: &oxigeo::GeoTransform,
    no_data: Option<f64>,
    sample_type: RasterSampleType,
) -> Result<RasterMetadata, CtbError> {
    if transform.row_rotation != 0.0 || transform.col_rotation != 0.0 {
        return Err(CtbError::UnsupportedRaster(
            "rotated or sheared raster transforms are not supported".to_owned(),
        ));
    }
    let metadata = RasterMetadata {
        width,
        height,
        band_count: 1,
        crs,
        transform: AffineTransform::north_up(
            transform.origin_x,
            transform.origin_y,
            transform.pixel_width,
            transform.pixel_height,
        )?,
        no_data,
        sample_type,
    };
    metadata.transform.bounds(width, height)?;
    Ok(metadata)
}

fn crs_from_epsg(epsg: u32) -> Result<Crs, CtbError> {
    let code = u16::try_from(epsg).map_err(|_| {
        CtbError::UnsupportedCrs(format!("EPSG:{epsg} is outside the supported code range"))
    })?;
    match code {
        4326 => Ok(Crs::Epsg4326),
        3857 => Ok(Crs::Epsg3857),
        _ => {
            proj4rs::Proj::from_epsg_code(code).map_err(|error| {
                CtbError::UnsupportedCrs(format!(
                    "EPSG:{code} cannot be resolved by proj4rs: {error}"
                ))
            })?;
            Ok(Crs::Epsg(code))
        }
    }
}

fn raster_sample_type(data_type: RasterDataType) -> Result<RasterSampleType, CtbError> {
    match data_type {
        RasterDataType::UInt8 => Ok(RasterSampleType::Unsigned8),
        RasterDataType::Int8 => Ok(RasterSampleType::Signed8),
        RasterDataType::UInt16 => Ok(RasterSampleType::Unsigned16),
        RasterDataType::Int16 => Ok(RasterSampleType::Signed16),
        RasterDataType::UInt32 => Ok(RasterSampleType::Unsigned32),
        RasterDataType::Int32 => Ok(RasterSampleType::Signed32),
        RasterDataType::Float32 => Ok(RasterSampleType::Float32),
        RasterDataType::Float64 => Ok(RasterSampleType::Float64),
        _ => Err(CtbError::UnsupportedRaster(format!(
            "unsupported raster sample encoding {data_type:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use oxigeo::{
        GeoTransform, RasterDataType,
        core_types::{io::FileDataSource, types::NoDataValue},
        geotiff::{
            CogReader, GeoKey, GeoTiffWriter, GeoTiffWriterOptions, OverviewResampling, TiffTag,
            WriterConfig,
            tiff::{Compression, Predictor},
        },
        vrt::{SourceWindow, VrtBand, VrtBuilder, VrtSource},
    };

    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        env::temp_dir().join(format!("ctb-rs-{name}-{}.tif", std::process::id()))
    }

    fn mark_pixel_is_point(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let source = FileDataSource::open(path)?;
        let cog = CogReader::open(source)?;
        let byte_order = cog.tiff().byte_order();
        let entry = cog
            .tiff()
            .ifds
            .first()
            .and_then(|ifd| ifd.get_entry(TiffTag::GeoKeyDirectory))
            .ok_or("GeoKeyDirectory tag missing")?;
        let source = FileDataSource::open(path)?;
        let directory = entry.get_value_bytes(&source, cog.tiff().header.variant)?;
        let word_at = |offset: usize| -> usize {
            usize::from(byte_order.read_u16(&directory[offset..offset + 2]))
        };
        let keys = cog.geo_keys().ok_or("GeoKeyDirectory tag missing")?;
        let raster_type_position = keys
            .entries
            .iter()
            .position(|key| key.key_id == GeoKey::GtRasterType as u16)
            .ok_or("fixture geokey directory lacks GTRasterType")?;
        let raster_type_index = 8 + raster_type_position * 8 + 6;
        assert_eq!(
            word_at(raster_type_index),
            1,
            "fixture must start as PixelIsArea"
        );

        let mut bytes = fs::read(path)?;
        let value_offset = usize::try_from(entry.value_offset)?;
        let directory_end = value_offset
            .checked_add(directory.len())
            .ok_or("directory offset overflow")?;
        assert!(
            directory_end <= bytes.len(),
            "GeoKeyDirectory lies outside fixture"
        );
        let target = value_offset + raster_type_index;
        byte_order.write_u16(&mut bytes[target..target + 2], 2);
        fs::write(path, bytes)?;
        Ok(())
    }

    #[derive(Default)]
    struct FixtureOptions {
        nodata: Option<f64>,
        overviews: bool,
    }

    fn write_bytes(
        path: &Path,
        dimensions: (u64, u64),
        data_type: RasterDataType,
        bytes: &[u8],
        epsg: u16,
        transform: GeoTransform,
        options: FixtureOptions,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut config = WriterConfig::new(dimensions.0, dimensions.1, 1, data_type)
            .with_compression(Compression::None)
            .with_predictor(Predictor::None)
            .with_overviews(options.overviews, OverviewResampling::Nearest)
            .with_geo_transform(transform)
            .with_epsg_code(u32::from(epsg));
        if let Some(value) = options.nodata {
            config = config.with_nodata(NoDataValue::from_float(value));
        }
        if options.overviews {
            config = config.with_overview_levels(vec![2, 4]);
            config.tile_width = Some(16);
            config.tile_height = Some(16);
        } else {
            config.tile_width = None;
            config.tile_height = None;
        }
        let mut writer = GeoTiffWriter::create(path, config, GeoTiffWriterOptions::default())?;
        writer.write(bytes)?;
        Ok(())
    }

    fn write_fixture(
        path: &Path,
        epsg: u16,
        nodata: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let samples = [10.0_f64, 11.0, 12.0, 13.0];
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (2, 2),
            RasterDataType::Float64,
            &bytes,
            epsg,
            GeoTransform::north_up(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions {
                nodata: nodata.map(|value| value.parse::<f64>()).transpose()?,
                ..FixtureOptions::default()
            },
        )
    }

    fn write_float32_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = [10.0_f32, 11.0, 12.0, 13.0];
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (2, 2),
            RasterDataType::Float32,
            &bytes,
            4326,
            GeoTransform::north_up(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_signed_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = [-100_i16, -1, 0, 150];
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (2, 2),
            RasterDataType::Int16,
            &bytes,
            4326,
            GeoTransform::north_up(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_unsigned_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = [0_u16, 10, 1_000, u16::MAX];
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (2, 2),
            RasterDataType::UInt16,
            &bytes,
            4326,
            GeoTransform::north_up(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_projected_fixture(path: &Path, epsg: u16) -> Result<(), Box<dyn std::error::Error>> {
        let samples = [10.0_f64, 11.0, 12.0, 13.0];
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (2, 2),
            RasterDataType::Float64,
            &bytes,
            epsg,
            GeoTransform::north_up(500_000.0, 0.0, 1.0, -1.0),
            FixtureOptions::default(),
        )
    }

    fn write_overview_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = (0..64).map(|value| value as f64).collect::<Vec<_>>();
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (8, 8),
            RasterDataType::Float64,
            &bytes,
            4326,
            GeoTransform::north_up(0.0, 8.0, 1.0, -1.0),
            FixtureOptions {
                overviews: true,
                ..FixtureOptions::default()
            },
        )
    }

    #[test]
    fn opens_epsg_4326_and_reads_a_window() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg4326");
        write_fixture(&path, 4326, None)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().width, 2);
        assert_eq!(source.metadata().height, 2);
        assert_eq!(source.metadata().crs, Crs::Epsg4326);
        assert_eq!(source.metadata().sample_type, RasterSampleType::Float64);
        assert_eq!(
            source
                .read_window(WindowRequest {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    overview: 0,
                })?
                .samples,
            vec![10.0, 11.0, 12.0, 13.0]
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn opens_an_epsg_3857_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg3857");
        write_fixture(&path, 3857, None)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().crs, Crs::Epsg3857);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn shifts_pixel_is_point_origin_like_gdal() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("pixel-is-point");
        write_fixture(&path, 4326, None)?;
        mark_pixel_is_point(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().transform.origin_x, -180.25);
        assert_eq!(source.metadata().transform.origin_y, 90.25);
        assert_eq!(source.metadata().transform.pixel_width, 0.5);
        assert_eq!(source.metadata().transform.pixel_height, -0.5);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn keeps_pixel_is_point_shift_in_overview_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("pixel-is-point-overviews");
        write_overview_fixture(&path)?;
        mark_pixel_is_point(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let half = source.sampling_level_for_ratio(2.0)?;
        assert_eq!(half.metadata.width, 4);
        assert_eq!(half.metadata.transform.origin_x, -0.5);
        assert_eq!(half.metadata.transform.origin_y, 8.5);
        assert_eq!(half.metadata.transform.pixel_width, 2.0);
        assert_eq!(half.metadata.transform.pixel_height, -2.0);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn opens_an_arbitrary_epsg_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg32630");
        write_projected_fixture(&path, 32630)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().crs, Crs::Epsg(32630));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rejects_an_unknown_epsg_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg-unknown");
        write_projected_fixture(&path, 9999)?;
        assert!(matches!(
            GeoTiffRasterSource::open(&path),
            Err(CtbError::UnsupportedCrs(_))
        ));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn marks_nodata_inside_a_window_without_rejecting_the_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("nodata");
        write_fixture(&path, 4326, Some("10"))?;
        let source = GeoTiffRasterSource::open(&path)?;
        let window = source.read_window(WindowRequest {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            overview: 0,
        })?;
        // GDALCreateWarpedVRT does not filter NoData; raw values are returned.
        assert_eq!(window.samples[0], 10.0);
        assert_eq!(window.samples[1], 11.0);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rejects_a_window_outside_the_raster() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("bounds");
        write_fixture(&path, 4326, None)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert!(matches!(
            source.read_window(WindowRequest {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
                overview: 0,
            }),
            Err(CtbError::InvalidRasterWindow)
        ));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn selects_and_reads_internal_overviews() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("overviews");
        write_overview_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.overview_count(), 2);

        let base = source.sampling_level_for_ratio(1.5)?;
        assert_eq!(base.level, 0);
        assert_eq!(base.metadata.width, 8);

        let half = source.sampling_level_for_ratio(2.0)?;
        // C++ warp reads from the base dataset at overview pixel indices;
        // sampling_level_for_ratio returns level 0 with overview metadata.
        assert_eq!(half.level, 0);
        assert_eq!(half.metadata.width, 4);
        assert_eq!(half.metadata.height, 4);
        assert_eq!(half.metadata.transform.pixel_width, 2.0);
        assert_eq!(half.metadata.transform.pixel_height, -2.0);

        let quarter = source.sampling_level_for_ratio(4.0)?;
        assert_eq!(quarter.level, 0);
        assert_eq!(quarter.metadata.width, 2);
        assert_eq!(quarter.metadata.height, 2);
        assert_eq!(quarter.metadata.transform.pixel_width, 4.0);
        assert_eq!(quarter.metadata.transform.pixel_height, -4.0);

        // Reading through the selected level reads from the base IFD at
        // overview pixel coordinates, matching C++ warp semantics.
        let window = source.read_sampling_window(
            &half,
            WindowRequest {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                overview: half.level,
            },
        )?;
        assert_eq!(window.samples, vec![0.0, 1.0, 8.0, 9.0]);

        // Verify the overview IFD itself is readable via an explicit level.
        let ovr_level = crate::raster::SamplingLevel {
            level: 1,
            data_width: half.metadata.width,
            data_height: half.metadata.height,
            metadata: half.metadata.clone(),
        };
        let ovr_window = source.read_sampling_window(
            &ovr_level,
            WindowRequest {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                overview: 1,
            },
        )?;
        assert_eq!(ovr_window.samples, vec![0.0, 2.0, 16.0, 18.0]);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn converts_float32_dem_samples_to_the_public_f64_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("float32");
        write_float32_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().sample_type, RasterSampleType::Float32);
        assert_eq!(
            source
                .read_window(WindowRequest {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    overview: 0,
                })?
                .samples,
            vec![10.0, 11.0, 12.0, 13.0]
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn converts_signed_integer_negative_elevations_to_f64() -> Result<(), Box<dyn std::error::Error>>
    {
        let path = fixture_path("int16-negative");
        write_signed_integer_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().sample_type, RasterSampleType::Signed16);
        assert_eq!(
            source
                .read_window(WindowRequest {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    overview: 0,
                })?
                .samples,
            vec![-100.0, -1.0, 0.0, 150.0]
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn converts_unsigned_integer_elevations_to_f64() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("uint16");
        write_unsigned_integer_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().sample_type, RasterSampleType::Unsigned16);
        assert_eq!(
            source
                .read_window(WindowRequest {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    overview: 0,
                })?
                .samples,
            vec![0.0, 10.0, 1_000.0, f64::from(u16::MAX)]
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn opens_a_vrt_and_reads_a_window() -> Result<(), Box<dyn std::error::Error>> {
        let source_path = fixture_path("vrt-source");
        write_fixture(&source_path, 4326, None)?;
        let vrt_path = fixture_path("vrt").with_extension("vrt");
        let band = VrtBand::simple(
            1,
            RasterDataType::Float64,
            VrtSource::simple(&source_path, 1).with_window(SourceWindow::identity(2, 2)),
        );
        VrtBuilder::with_size(2, 2)
            .with_srs("EPSG:4326")
            .with_geo_transform(GeoTransform::north_up(-180.0, 90.0, 0.5, -0.5))
            .add_band(band)?
            .build_file(&vrt_path)?;

        let source = GeoTiffRasterSource::open(&vrt_path)?;
        assert_eq!(source.metadata().width, 2);
        assert_eq!(source.metadata().height, 2);
        assert_eq!(source.metadata().crs, Crs::Epsg4326);
        assert_eq!(source.metadata().sample_type, RasterSampleType::Float64);
        assert_eq!(source.overview_count(), 0);
        assert_eq!(
            source
                .read_window(WindowRequest {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                    overview: 0,
                })?
                .samples,
            vec![10.0, 11.0, 12.0, 13.0]
        );
        fs::remove_file(source_path)?;
        fs::remove_file(vrt_path)?;
        Ok(())
    }

    #[test]
    fn rejects_a_truncated_geotiff_without_panicking() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("truncated");
        fs::write(&path, [0x49_u8, 0x49, 0x2A])?;
        assert!(matches!(
            GeoTiffRasterSource::open(&path),
            Err(CtbError::RasterRead(_))
        ));
        fs::remove_file(path)?;
        Ok(())
    }
}
