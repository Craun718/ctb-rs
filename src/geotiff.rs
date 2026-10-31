use std::path::Path;

use geotiff_core::GeoTransform as GeotiffRustTransform;
use geotiff_reader::GeoTiffFile;
use ndarray::ArrayD;
use oxigeo::{
    DatasetFormat,
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
use tiff_core::sample::TiffSample;

enum RasterData {
    GeoTiff(GeoTiffFile),
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
                let file = GeoTiffFile::open(path)
                    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                Self::from_geotiff(file)
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

    fn from_geotiff(file: GeoTiffFile) -> Result<Self, CtbError> {
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }
        let transform = file.transform().ok_or_else(|| {
            CtbError::UnsupportedRaster("missing GeoTIFF affine transform".to_owned())
        })?;
        let transform = oxigeo_transform(transform);
        let layout = file
            .tiff()
            .ifd(file.base_ifd_index())
            .map_err(|error| CtbError::RasterRead(error.to_string()))?
            .raster_layout()
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let sample_type = raster_sample_type(layout.sample_format, layout.bits_per_sample)?;
        let _ = sample_type;
        let epsg = file.epsg().ok_or(CtbError::MissingCrs)?;
        let (width, height) = raster_dimensions(u64::from(file.width()), u64::from(file.height()))?;
        let metadata = build_metadata(
            width,
            height,
            crs_from_epsg(epsg)?,
            &transform,
            file.nodata().and_then(|value| value.parse::<f64>().ok()),
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
        let sample_type = raster_sample_type_oxigeo(data_type)?;
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
            RasterData::GeoTiff(file) => {
                let x = usize::try_from(x).map_err(|_| {
                    CtbError::RasterRead("GeoTIFF window offset overflow".to_owned())
                })?;
                let y = usize::try_from(y).map_err(|_| {
                    CtbError::RasterRead("GeoTIFF window offset overflow".to_owned())
                })?;
                let sample_type = self.metadata.sample_type;
                let values: ArrayD<f64> = match sample_type {
                    RasterSampleType::Unsigned8 => {
                        read_window_typed::<u8>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Signed8 => {
                        read_window_typed::<i8>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Unsigned16 => {
                        read_window_typed::<u16>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Signed16 => {
                        read_window_typed::<i16>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Unsigned32 => {
                        read_window_typed::<u32>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Signed32 => {
                        read_window_typed::<i32>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Float32 => {
                        read_window_typed::<f32>(file, level, x, y, height, width)?
                    }
                    RasterSampleType::Float64 => {
                        read_window_typed::<f64>(file, level, x, y, height, width)?
                    }
                };
                let values = values.as_slice().ok_or_else(|| {
                    CtbError::RasterRead("GeoTIFF window is not contiguous".to_owned())
                })?;
                samples.copy_from_slice(values);
            }
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

    fn level_size(&self, level: usize) -> Result<(u64, u64), CtbError> {
        match &self.data {
            RasterData::GeoTiff(file) => file
                .overview_ifd(level.checked_sub(1).ok_or_else(|| {
                    CtbError::RasterRead(format!("GeoTIFF overview level {level} is invalid"))
                })?)
                .map(|ifd| (u64::from(ifd.width()), u64::from(ifd.height())))
                .map_err(|error| CtbError::RasterRead(error.to_string())),
            RasterData::Vrt(_) => Err(CtbError::UnsupportedRaster(
                "VRT inputs have no overview levels".to_owned(),
            )),
        }
    }
}

fn read_window_typed<T>(
    file: &GeoTiffFile,
    level: u16,
    x: usize,
    y: usize,
    height: usize,
    width: usize,
) -> Result<ArrayD<f64>, CtbError>
where
    T: TiffSample,
    f64: From<T>,
{
    let values: ArrayD<T> = if level == 0 {
        file.tiff()
            .read_band_window::<T>(file.base_ifd_index(), 0, y, x, height, width)
            .map_err(geotiff_reader::Error::from)
    } else {
        let overview = usize::from(level)
            .checked_sub(1)
            .ok_or(CtbError::InvalidRasterWindow)?;
        file.read_overview_band_window::<T>(overview, 0, y, x, height, width)
    }
    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let values = values.mapv(|value| f64::from(value));
    if values.shape() != [height, width] {
        return Err(CtbError::RasterRead(format!(
            "geotiff-rust returned window shape {:?}, expected [{height}, {width}]",
            values.shape()
        )));
    }
    Ok(values)
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

fn oxigeo_transform(transform: &GeotiffRustTransform) -> oxigeo::GeoTransform {
    oxigeo::GeoTransform::north_up(
        transform.origin_x,
        transform.origin_y,
        transform.pixel_width,
        transform.pixel_height,
    )
}

fn raster_sample_type_oxigeo(
    data_type: oxigeo::RasterDataType,
) -> Result<RasterSampleType, CtbError> {
    match data_type {
        oxigeo::RasterDataType::UInt8 => Ok(RasterSampleType::Unsigned8),
        oxigeo::RasterDataType::Int8 => Ok(RasterSampleType::Signed8),
        oxigeo::RasterDataType::UInt16 => Ok(RasterSampleType::Unsigned16),
        oxigeo::RasterDataType::Int16 => Ok(RasterSampleType::Signed16),
        oxigeo::RasterDataType::UInt32 => Ok(RasterSampleType::Unsigned32),
        oxigeo::RasterDataType::Int32 => Ok(RasterSampleType::Signed32),
        oxigeo::RasterDataType::Float32 => Ok(RasterSampleType::Float32),
        oxigeo::RasterDataType::Float64 => Ok(RasterSampleType::Float64),
        _ => Err(CtbError::UnsupportedRaster(format!(
            "unsupported raster sample encoding {data_type:?}"
        ))),
    }
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

fn raster_sample_type(
    sample_format: u16,
    bits_per_sample: u16,
) -> Result<RasterSampleType, CtbError> {
    match (sample_format, bits_per_sample) {
        (1, 8) => Ok(RasterSampleType::Unsigned8),
        (2, 8) => Ok(RasterSampleType::Signed8),
        (1, 16) => Ok(RasterSampleType::Unsigned16),
        (2, 16) => Ok(RasterSampleType::Signed16),
        (1, 32) => Ok(RasterSampleType::Unsigned32),
        (2, 32) => Ok(RasterSampleType::Signed32),
        (3, 32) => Ok(RasterSampleType::Float32),
        (3, 64) => Ok(RasterSampleType::Float64),
        (format, bits) => Err(CtbError::UnsupportedRaster(format!(
            "unsupported raster sample encoding SampleFormat={format} BitsPerSample={bits}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use geotiff_core::GeoTransform as FixtureTransform;
    use geotiff_writer::{Compression, GeoTiffBuilder, Predictor};
    use ndarray::Array2;
    use oxigeo::{
        GeoTransform, RasterDataType,
        vrt::{SourceWindow, VrtBand, VrtBuilder, VrtSource},
    };

    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        env::temp_dir().join(format!("ctb-rs-{name}-{}.tif", std::process::id()))
    }

    #[derive(Default)]
    struct FixtureOptions {
        nodata: Option<f64>,
    }

    fn write_fixture_with_types<T>(
        path: &Path,
        dimensions: (u32, u32),
        values: [T; 4],
        epsg: u16,
        transform: FixtureTransform,
        options: FixtureOptions,
    ) -> Result<(), Box<dyn std::error::Error>>
    where
        T: geotiff_writer::NumericSample,
    {
        let data = values.to_vec();
        let array = Array2::from_shape_vec(
            (
                usize::try_from(dimensions.1)?,
                usize::try_from(dimensions.0)?,
            ),
            data,
        )?;
        let mut builder = GeoTiffBuilder::new(dimensions.0, dimensions.1)
            .epsg(epsg)
            .transform(transform)
            .compression(Compression::None)
            .predictor(Predictor::None);
        if let Some(value) = options.nodata {
            builder = builder.nodata(&value.to_string());
        }
        builder.write_2d(path, array.view())?;
        Ok(())
    }

    fn write_fixture(
        path: &Path,
        epsg: u16,
        nodata: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_with_types(
            path,
            (2, 2),
            [10.0_f64, 11.0, 12.0, 13.0],
            epsg,
            FixtureTransform::from_origin_and_pixel_size(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions {
                nodata: nodata.map(|value| value.parse::<f64>()).transpose()?,
            },
        )
    }

    fn write_float32_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_with_types(
            path,
            (2, 2),
            [10.0_f32, 11.0, 12.0, 13.0],
            4326,
            FixtureTransform::from_origin_and_pixel_size(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_signed_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_with_types(
            path,
            (2, 2),
            [-100_i16, -1, 0, 150],
            4326,
            FixtureTransform::from_origin_and_pixel_size(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_unsigned_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_with_types(
            path,
            (2, 2),
            [0_u16, 10, 1_000, u16::MAX],
            4326,
            FixtureTransform::from_origin_and_pixel_size(-180.0, 90.0, 0.5, -0.5),
            FixtureOptions::default(),
        )
    }

    fn write_projected_fixture(path: &Path, epsg: u16) -> Result<(), Box<dyn std::error::Error>> {
        write_fixture_with_types(
            path,
            (2, 2),
            [10.0_f64, 11.0, 12.0, 13.0],
            epsg,
            FixtureTransform::from_origin_and_pixel_size(500_000.0, 0.0, 1.0, -1.0),
            FixtureOptions::default(),
        )
    }

    fn write_overview_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let data = Array2::from_shape_fn((8, 8), |(row, column)| (row * 8 + column) as f64);
        let builder = GeoTiffBuilder::new(8, 8)
            .epsg(4326)
            .transform(FixtureTransform::from_origin_and_pixel_size(
                0.0, 8.0, 1.0, -1.0,
            ))
            .tile_size(16, 16);
        geotiff_writer::CogBuilder::new(builder)
            .overview_levels(vec![2, 4])
            .resampling(geotiff_writer::Resampling::NearestNeighbor)
            .write_2d(path, data.view())?;
        Ok(())
    }

    fn write_edge_fixture(path: &Path, tiled: bool) -> Result<(), Box<dyn std::error::Error>> {
        let data =
            Array2::from_shape_fn((24, 40), |(row, column)| column as f32 + row as f32 * 100.0);
        let mut builder = GeoTiffBuilder::new(40, 24)
            .epsg(4326)
            .transform(FixtureTransform::from_origin_and_pixel_size(
                0.0, 24.0, 1.0, -1.0,
            ))
            .compression(Compression::None)
            .predictor(Predictor::None);
        if tiled {
            builder = builder.tile_size(16, 16);
        }
        builder.write_2d(path, data.view())?;
        Ok(())
    }

    fn write_large_overview_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let data = Array2::from_shape_fn((64, 64), |(row, column)| (row * 64 + column) as f64);
        let builder = GeoTiffBuilder::new(64, 64)
            .epsg(4326)
            .transform(FixtureTransform::from_origin_and_pixel_size(
                0.0, 64.0, 1.0, -1.0,
            ))
            .tile_size(16, 16);
        geotiff_writer::CogBuilder::new(builder)
            .overview_levels(vec![2, 4])
            .resampling(geotiff_writer::Resampling::NearestNeighbor)
            .write_2d(path, data.view())?;
        Ok(())
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
        let array = Array2::from_shape_vec((2, 2), [10.0_f64, 11.0, 12.0, 13.0].to_vec())
            .map_err(|error| geotiff_writer::Error::Other(error.to_string()))?;
        GeoTiffBuilder::new(2, 2)
            .epsg(4326)
            .raster_type(geotiff_writer::RasterType::PixelIsPoint)
            .transform(FixtureTransform::from_origin_and_pixel_size(
                -180.25, 90.25, 0.5, -0.5,
            ))
            .write_2d(&path, array.view())?;
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
        let samples = (0..64).map(|value| value as f64).collect::<Vec<_>>();
        let array = Array2::from_shape_vec((8, 8), samples)
            .map_err(|error| geotiff_writer::Error::Other(error.to_string()))?;
        let builder = GeoTiffBuilder::new(8, 8)
            .epsg(4326)
            .raster_type(geotiff_writer::RasterType::PixelIsPoint)
            .transform(FixtureTransform::from_origin_and_pixel_size(
                -0.5, 8.5, 1.0, -1.0,
            ))
            .tile_size(16, 16);
        geotiff_writer::CogBuilder::new(builder)
            .overview_levels(vec![2, 4])
            .resampling(geotiff_writer::Resampling::NearestNeighbor)
            .write_2d(&path, array.view())?;
        let source = GeoTiffRasterSource::open(&path)?;
        let half = source.sampling_level_for_ratio(4.0)?;
        assert_eq!(half.metadata.width, 2);
        assert_eq!(half.metadata.transform.origin_x, -0.5);
        assert_eq!(half.metadata.transform.origin_y, 8.5);
        assert_eq!(half.metadata.transform.pixel_width, 4.0);
        assert_eq!(half.metadata.transform.pixel_height, -4.0);
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
    fn reads_windows_across_tiled_edge_blocks() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("tiled-edge");
        write_edge_fixture(&path, true)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 5,
            width: 20,
            height: 18,
            overview: 0,
        };
        let mut expected = Vec::with_capacity(20 * 18);
        for row in 5..23 {
            for column in 15..35 {
                expected.push(column as f32 as f64 + row as f32 as f64 * 100.0);
            }
        }
        assert_eq!(source.read_samples(0, request)?, expected);
        let repeated = source.read_samples(0, request)?;
        assert_eq!(repeated, expected);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn reads_windows_across_striped_final_block() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("striped-edge");
        write_edge_fixture(&path, false)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 5,
            width: 20,
            height: 18,
            overview: 0,
        };
        let mut expected = Vec::with_capacity(20 * 18);
        for row in 5..23 {
            for column in 15..35 {
                expected.push(column as f32 as f64 + row as f32 as f64 * 100.0);
            }
        }
        assert_eq!(source.read_samples(0, request)?, expected);
        let repeated = source.read_samples(0, request)?;
        assert_eq!(repeated, expected);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn reads_windows_on_explicit_overview_level() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("overview-window");
        write_large_overview_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 15,
            width: 17,
            height: 17,
            overview: 1,
        };
        let mut expected = Vec::with_capacity(17 * 17);
        for row in 15..32 {
            for column in 15..32 {
                expected.push((row * 2 * 64 + column * 2) as f64);
            }
        }
        assert_eq!(source.read_samples(1, request)?, expected);
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
