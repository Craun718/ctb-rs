use std::{fs::File, io::Read, path::Path, sync::Arc};

use geotiff_reader::{GeoTiffFile, GeoTiffOpenOptions};

use crate::{
    CtbError,
    raster::{
        AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterSource, RasterWindow,
        SamplingLevel, WindowRequest,
    },
    vrt::VrtReader,
};

pub type SharedGeoTiffReader = Arc<GeoTiffFile>;

const GEOTIFF_DECODED_BLOCK_CACHE_BYTES: usize = 819 << 20;
const GEOTIFF_DECODED_BLOCK_CACHE_SLOTS: usize = 65_536;
const RASTER_FORMAT_PROBE_LIMIT_BYTES: u64 = 64 << 10;

enum RasterData {
    GeoTiff(SharedGeoTiffReader),
    Vrt(Arc<VrtReader>),
}

/// A restricted, pure-Rust GeoTIFF/VRT source for the direct-source contract.
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
        Self::open_inner(path.as_ref())
    }

    pub fn new_shared_block_cache(path: impl AsRef<Path>) -> Result<SharedGeoTiffReader, CtbError> {
        let path = path.as_ref();
        match detect_raster_format(path)? {
            RasterFormat::GeoTiff => open_shared_geotiff(path),
            RasterFormat::Vrt => Err(CtbError::UnsupportedRaster(
                "shared GeoTIFF block cache requires a GeoTIFF input, found VRT".to_owned(),
            )),
        }
    }

    pub fn open_with_shared_cache(
        path: impl AsRef<Path>,
        reader: SharedGeoTiffReader,
    ) -> Result<Self, CtbError> {
        let path = path.as_ref();
        match detect_raster_format(path)? {
            RasterFormat::GeoTiff => Self::from_geotiff(reader),
            RasterFormat::Vrt => Err(CtbError::UnsupportedRaster(
                "shared GeoTIFF block cache cannot wrap a VRT input".to_owned(),
            )),
        }
    }

    fn open_inner(path: &Path) -> Result<Self, CtbError> {
        match detect_raster_format(path)? {
            RasterFormat::GeoTiff => Self::from_geotiff(open_shared_geotiff(path)?),
            RasterFormat::Vrt => Self::from_vrt(Arc::new(VrtReader::open(path)?)),
        }
    }

    fn from_geotiff(file: SharedGeoTiffReader) -> Result<Self, CtbError> {
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }
        let transform = file.transform().ok_or_else(|| {
            CtbError::UnsupportedRaster("missing GeoTIFF affine transform".to_owned())
        })?;
        let sample_type = geotiff_sample_type(&file)?;
        let epsg = file.epsg().ok_or(CtbError::MissingCrs)?;
        let no_data = match file.nodata() {
            Some(value) => Some(value.parse::<f64>().map_err(|_| {
                CtbError::UnsupportedRaster(format!("cannot parse GeoTIFF NoData value {value:?}"))
            })?),
            None => None,
        };
        let metadata = build_metadata(
            file.width(),
            file.height(),
            crs_from_epsg(epsg)?,
            transform,
            no_data,
            sample_type,
        )?;
        Ok(Self {
            data: RasterData::GeoTiff(file),
            metadata,
        })
    }

    fn from_vrt(file: Arc<VrtReader>) -> Result<Self, CtbError> {
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }
        let transform = file.geo_transform();
        let epsg = crate::vrt::resolve_vrt_epsg(file.srs())?;
        let metadata = build_metadata(
            file.width(),
            file.height(),
            crs_from_epsg(u32::from(epsg))?,
            &geotiff_reader::transform::GeoTransform::from_origin_and_pixel_size(
                transform[0],
                transform[3],
                transform[1],
                transform[5],
            ),
            file.band_no_data(),
            file.band_sample_type(),
        )?;
        Ok(Self {
            data: RasterData::Vrt(file),
            metadata,
        })
    }

    fn read_samples(&self, level: u16, request: WindowRequest) -> Result<Vec<f64>, CtbError> {
        match &self.data {
            RasterData::GeoTiff(file) => {
                if level == 0 {
                    read_geotiff_band_window(file, 1, request)
                } else {
                    read_geotiff_overview_window(file, usize::from(level) - 1, 1, request)
                }
            }
            RasterData::Vrt(file) => {
                if level != 0 {
                    return Err(CtbError::UnsupportedRaster(
                        "VRT inputs have no overview levels".to_owned(),
                    ));
                }
                file.read_window(request)
            }
        }
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

    fn level_size(&self, level: usize) -> Result<(u32, u32), CtbError> {
        let RasterData::GeoTiff(file) = &self.data else {
            return Err(CtbError::UnsupportedRaster(
                "VRT inputs have no overview levels".to_owned(),
            ));
        };
        if level == 0 {
            return Ok((file.width(), file.height()));
        }
        let overview = file
            .overview_ifd(level - 1)
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        Ok((overview.width(), overview.height()))
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
        let samples = self.read_samples(0, request)?;
        Ok(RasterWindow { request, samples })
    }

    fn sampling_level_for_ratio(&self, target_ratio: f64) -> Result<SamplingLevel, CtbError> {
        let overview_count = self.overview_count();
        if !target_ratio.is_finite() || target_ratio <= 1.0 || overview_count == 0 {
            return Ok(SamplingLevel {
                level: 0,
                data_width: self.metadata.width,
                data_height: self.metadata.height,
                metadata: self.metadata.clone(),
            });
        }

        let mut selected = -1_i32;
        for overview in -1..i32::from(overview_count - 1) {
            let level = if overview < 0 {
                0
            } else {
                usize::try_from(overview)
                    .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?
                    + 1
            };
            let (overview_width, _) = self.level_size(level)?;
            let ratio = f64::from(self.metadata.width) / f64::from(overview_width);
            let next_level = usize::try_from(overview + 1)
                .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?
                + 1;
            let (next_width, _) = self.level_size(next_level)?;
            let next_ratio = f64::from(self.metadata.width) / f64::from(next_width);
            if (ratio < target_ratio && next_ratio > target_ratio)
                || (ratio - target_ratio).abs() < 0.1
            {
                selected = overview;
                break;
            }
            selected = overview + 1;
        }
        if selected < 0 {
            return Ok(SamplingLevel {
                level: 0,
                data_width: self.metadata.width,
                data_height: self.metadata.height,
                metadata: self.metadata.clone(),
            });
        }

        let index = usize::try_from(selected)
            .map_err(|_| CtbError::RasterRead("overview index overflow".to_owned()))?;
        let (width, height) = self.level_size(index + 1)?;
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
        Ok(SamplingLevel {
            // C++ GDALTiler::createRasterTile recreates the transformer from
            // the overview dataset but never updates psWarpOptions->hSrcDS,
            // so the warp kernel reads from the base dataset at overview pixel
            // indices. level 0 preserves overview metadata for coordinate math
            // while reading from the base band, matching the C++ oracle.
            level: 0,
            data_width: self.metadata.width,
            data_height: self.metadata.height,
            metadata,
        })
    }

    fn read_sampling_window(
        &self,
        level: &SamplingLevel,
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

pub(crate) fn open_geotiff(path: &Path) -> Result<GeoTiffFile, CtbError> {
    GeoTiffFile::open(path).map_err(|error| CtbError::RasterRead(error.to_string()))
}

pub(crate) fn open_shared_geotiff(path: &Path) -> Result<SharedGeoTiffReader, CtbError> {
    let options = GeoTiffOpenOptions {
        block_cache_bytes: GEOTIFF_DECODED_BLOCK_CACHE_BYTES,
        block_cache_slots: GEOTIFF_DECODED_BLOCK_CACHE_SLOTS,
        ..GeoTiffOpenOptions::default()
    };
    GeoTiffFile::open_with_options(path, options)
        .map(Arc::new)
        .map_err(|error| CtbError::RasterRead(error.to_string()))
}

pub(crate) fn read_geotiff_band_window(
    file: &GeoTiffFile,
    band: usize,
    request: WindowRequest,
) -> Result<Vec<f64>, CtbError> {
    read_geotiff_window(file, None, band, request)
}

fn read_geotiff_overview_window(
    file: &GeoTiffFile,
    overview: usize,
    band: usize,
    request: WindowRequest,
) -> Result<Vec<f64>, CtbError> {
    read_geotiff_window(file, Some(overview), band, request)
}

fn read_geotiff_window(
    file: &GeoTiffFile,
    overview: Option<usize>,
    band: usize,
    request: WindowRequest,
) -> Result<Vec<f64>, CtbError> {
    let row = usize::try_from(request.y).map_err(|_| CtbError::InvalidRasterWindow)?;
    let column = usize::try_from(request.x).map_err(|_| CtbError::InvalidRasterWindow)?;
    let height = usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
    let width = usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
    let band_index = band.saturating_sub(1);

    macro_rules! read_as {
        ($sample_type:ty) => {{
            let result = if let Some(overview) = overview {
                file.read_overview_band_window::<$sample_type>(
                    overview, band_index, row, column, height, width,
                )
            } else {
                file.read_band_window::<$sample_type>(band_index, row, column, height, width)
            };
            result
                .map(|array| {
                    array
                        .iter()
                        .map(|sample| f64::from(*sample))
                        .collect::<Vec<_>>()
                })
                .map_err(|error| CtbError::RasterRead(error.to_string()))
        }};
    }

    match geotiff_sample_type(file)? {
        RasterSampleType::Unsigned8 => read_as!(u8),
        RasterSampleType::Signed8 => read_as!(i8),
        RasterSampleType::Unsigned16 => read_as!(u16),
        RasterSampleType::Signed16 => read_as!(i16),
        RasterSampleType::Unsigned32 => read_as!(u32),
        RasterSampleType::Signed32 => read_as!(i32),
        RasterSampleType::Float32 => read_as!(f32),
        RasterSampleType::Float64 => read_as!(f64),
    }
}

fn geotiff_sample_type(file: &GeoTiffFile) -> Result<RasterSampleType, CtbError> {
    let ifd = file
        .tiff()
        .ifd(file.base_ifd_index())
        .map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let format = ifd
        .sample_format()
        .map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let bits = ifd
        .bits_per_sample()
        .map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let sample_format = format
        .first()
        .copied()
        .ok_or_else(|| CtbError::UnsupportedRaster("GeoTIFF is missing SampleFormat".to_owned()))?;
    let bits = bits.first().copied().ok_or_else(|| {
        CtbError::UnsupportedRaster("GeoTIFF is missing BitsPerSample".to_owned())
    })?;
    match (sample_format, bits) {
        (1, 8) => Ok(RasterSampleType::Unsigned8),
        (2, 8) => Ok(RasterSampleType::Signed8),
        (1, 16) => Ok(RasterSampleType::Unsigned16),
        (2, 16) => Ok(RasterSampleType::Signed16),
        (1, 32) => Ok(RasterSampleType::Unsigned32),
        (2, 32) => Ok(RasterSampleType::Signed32),
        (3, 32) => Ok(RasterSampleType::Float32),
        (3, 64) => Ok(RasterSampleType::Float64),
        _ => Err(CtbError::UnsupportedRaster(format!(
            "unsupported GeoTIFF sample encoding SampleFormat={sample_format}, BitsPerSample={bits}"
        ))),
    }
}

fn build_metadata(
    width: u32,
    height: u32,
    crs: Crs,
    transform: &geotiff_reader::transform::GeoTransform,
    no_data: Option<f64>,
    sample_type: RasterSampleType,
) -> Result<RasterMetadata, CtbError> {
    if transform.skew_x != 0.0 || transform.skew_y != 0.0 {
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RasterFormat {
    GeoTiff,
    Vrt,
}

pub(crate) fn detect_raster_format(path: &Path) -> Result<RasterFormat, CtbError> {
    let mut file = File::open(path)
        .map_err(|error| CtbError::RasterRead(format!("cannot open raster {path:?}: {error}")))?;
    let mut prefix = [0_u8; 8];
    let mut count = 0_usize;
    while count < prefix.len() {
        let read = file.read(&mut prefix[count..]).map_err(|error| {
            CtbError::RasterRead(format!("cannot read raster {path:?}: {error}"))
        })?;
        if read == 0 {
            break;
        }
        count += read;
    }
    if count >= 4
        && (prefix[..4] == [0x49, 0x49, 0x2a, 0x00]
            || prefix[..4] == [0x49, 0x49, 0x2b, 0x00]
            || prefix[..4] == [0x4d, 0x4d, 0x00, 0x2a]
            || prefix[..4] == [0x4d, 0x4d, 0x00, 0x2b])
    {
        return Ok(RasterFormat::GeoTiff);
    }

    let mut bytes = prefix[..count].to_vec();
    let header_length =
        u64::try_from(count).expect("format probing never reads beyond its eight-byte header");
    let mut limited = file.take(RASTER_FORMAT_PROBE_LIMIT_BYTES - header_length);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| CtbError::RasterRead(format!("cannot read raster {path:?}: {error}")))?;
    let text = String::from_utf8_lossy(&bytes);
    if text.trim_start().starts_with('<') && text.contains("<VRTDataset") {
        return Ok(RasterFormat::Vrt);
    }
    Err(unsupported_raster_format(path))
}

fn unsupported_raster_format(path: &Path) -> CtbError {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("unknown"));
    CtbError::UnsupportedRaster(format!(
        "raster format {extension:?} is not supported; supported raster inputs are GeoTIFF and VRT"
    ))
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf};

    use geotiff_writer::{CogBuilder, Compression, GeoTiffBuilder, Predictor, TiffVariant};
    use ndarray::{Array2, array};

    use super::*;

    fn fixture_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!("ctb-rs-{name}-{}.tif", std::process::id()))
    }

    fn write_fixture(
        path: &Path,
        epsg: u16,
        nodata: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let samples = array![[10.0_f64, 11.0], [12.0, 13.0]];
        let mut builder = GeoTiffBuilder::new(2, 2)
            .epsg(epsg)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0);
        if let Some(value) = nodata {
            builder = builder.nodata(value);
        }
        builder.write_2d(path, samples.view())?;
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
    fn opens_a_little_endian_bigtiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("bigtiff");
        let samples = array![[10.0_f64, 11.0], [12.0, 13.0]];
        GeoTiffBuilder::new(2, 2)
            .geographic_epsg(4326)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0)
            .tiff_variant(TiffVariant::BigTiff)
            .write_2d(&path, samples.view())?;
        assert_eq!(
            fs::read(&path)?.get(..4),
            Some(&[0x49, 0x49, 0x2b, 0x00][..])
        );
        assert_eq!(detect_raster_format(&path)?, RasterFormat::GeoTiff);
        let source = GeoTiffRasterSource::open(&path)?;
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
    fn format_detection_uses_all_tiff_byte_orders() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("format-header");
        for header in [
            [0x49, 0x49, 0x2a, 0x00],
            [0x49, 0x49, 0x2b, 0x00],
            [0x4d, 0x4d, 0x00, 0x2a],
            [0x4d, 0x4d, 0x00, 0x2b],
        ] {
            fs::write(&path, header)?;
            assert_eq!(detect_raster_format(&path)?, RasterFormat::GeoTiff);
        }
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn format_detection_reads_a_bounded_vrt_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("format-prefix").with_extension("vrt");
        let xml = r#"<?xml version="1.0"?><VRTDataset rasterXSize="1"/>"#;
        fs::write(&path, xml)?;
        assert_eq!(detect_raster_format(&path)?, RasterFormat::Vrt);

        let mut large_non_vrt = String::from("<not-vrt>");
        large_non_vrt.push_str(&"x".repeat(128 * 1024));
        fs::write(&path, large_non_vrt)?;
        assert!(matches!(
            detect_raster_format(&path),
            Err(CtbError::UnsupportedRaster(_))
        ));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn shifts_pixel_is_point_origin_like_gdal() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("pixel-is-point");
        let samples = array![[10.0_f64, 11.0], [12.0, 13.0]];
        GeoTiffBuilder::new(2, 2)
            .geographic_epsg(4326)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0)
            .raster_type(geotiff_writer::RasterType::PixelIsPoint)
            .write_2d(&path, samples.view())?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().transform.origin_x, -180.0);
        assert_eq!(source.metadata().transform.origin_y, 90.0);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn opens_an_arbitrary_epsg_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg32630");
        let samples = array![[10.0_f64, 11.0], [12.0, 13.0]];
        GeoTiffBuilder::new(2, 2)
            .epsg(32630)
            .pixel_scale(1.0, 1.0)
            .origin(500_000.0, 0.0)
            .write_2d(&path, samples.view())?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().crs, Crs::Epsg(32630));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rejects_an_unknown_epsg_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg-unknown");
        let samples = array![[10.0_f64, 11.0], [12.0, 13.0]];
        GeoTiffBuilder::new(2, 2)
            .epsg(9999)
            .pixel_scale(1.0, 1.0)
            .origin(500_000.0, 0.0)
            .write_2d(&path, samples.view())?;
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
        assert_eq!(window.samples[0], 10.0);
        assert_eq!(window.samples[1], 11.0);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn selects_and_reads_internal_overviews() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("overviews");
        let samples = Array2::from_shape_fn((8, 8), |(row, column)| (row * 8 + column) as f64);
        let builder = GeoTiffBuilder::new(8, 8)
            .geographic_epsg(4326)
            .pixel_scale(1.0, 1.0)
            .origin(0.0, 8.0)
            .tile_size(16, 16);
        CogBuilder::new(builder)
            .overview_levels(vec![2, 4])
            .write_2d(&path, samples.view())?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.overview_count(), 2);
        let half = source.sampling_level_for_ratio(2.0)?;
        assert_eq!(half.level, 0);
        assert_eq!(half.metadata.width, 4);
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
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rejects_a_truncated_geotiff_without_panicking() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("truncated");
        fs::write(&path, [0x49_u8, 0x49, 0x2a, 0x00])?;
        assert!(matches!(
            GeoTiffRasterSource::open(&path),
            Err(CtbError::RasterRead(_))
        ));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn geo_tiff_source_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GeoTiffRasterSource>();
    }

    #[test]
    fn compression_and_tiff_variant_types_are_available() {
        assert_eq!(Compression::None, Compression::None);
        assert_eq!(Predictor::None, Predictor::None);
        assert_eq!(TiffVariant::Auto, TiffVariant::Auto);
    }
}
