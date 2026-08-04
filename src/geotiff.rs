use std::path::Path;

use geotiff_reader::GeoTiffFile;

use crate::{
    CtbError,
    raster::{
        AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterSource, RasterWindow,
        WindowRequest,
    },
};

/// A restricted, pure-Rust GeoTIFF source for the direct-source input contract.
///
/// It accepts one north-up band in either CTB built-in grid CRS. Reprojection
/// remains at the sampling-plan boundary; overview selection is level-aware.
pub struct GeoTiffRasterSource {
    file: GeoTiffFile,
    metadata: RasterMetadata,
}

impl GeoTiffRasterSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CtbError> {
        let file =
            GeoTiffFile::open(path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let epsg = file.epsg().ok_or(CtbError::MissingCrs)?;
        let crs = match epsg {
            4326 => Crs::Epsg4326,
            3857 => Crs::Epsg3857,
            _ => return Err(CtbError::UnsupportedCrs(format!("EPSG:{epsg}"))),
        };
        if file.band_count() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one elevation band, found {} bands",
                file.band_count()
            )));
        }

        let transform = file.transform().ok_or_else(|| {
            CtbError::UnsupportedRaster("missing GeoTIFF affine transform".to_owned())
        })?;
        if transform.skew_x != 0.0 || transform.skew_y != 0.0 {
            return Err(CtbError::UnsupportedRaster(
                "rotated or sheared GeoTIFF transforms are not supported".to_owned(),
            ));
        }

        let no_data = match file.nodata() {
            Some(value) => Some(value.parse::<f64>().map_err(|_| {
                CtbError::UnsupportedRaster(format!("cannot parse GeoTIFF NoData value {value:?}"))
            })?),
            None => None,
        };
        let base_ifd = file
            .tiff()
            .ifd(file.base_ifd_index())
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let sample_type = raster_sample_type(
            base_ifd
                .sample_format()
                .map_err(|error| CtbError::RasterRead(error.to_string()))?,
            base_ifd
                .bits_per_sample()
                .map_err(|error| CtbError::RasterRead(error.to_string()))?,
        )?;
        let metadata = RasterMetadata {
            width: file.width(),
            height: file.height(),
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
        metadata.transform.bounds(metadata.width, metadata.height)?;

        Ok(Self { file, metadata })
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
        macro_rules! decode_as {
            ($sample_type:ty) => {
                (if level == 0 {
                    self.file.read_band_window::<$sample_type>(
                        0,
                        request.y as usize,
                        request.x as usize,
                        request.height as usize,
                        request.width as usize,
                    )
                } else {
                    self.file.read_overview_band_window::<$sample_type>(
                        usize::from(level - 1),
                        0,
                        request.y as usize,
                        request.x as usize,
                        request.height as usize,
                        request.width as usize,
                    )
                })
                .ok()
                .map(|samples| {
                    samples
                        .iter()
                        .map(|sample| *sample as f64)
                        .collect::<Vec<_>>()
                })
            };
        }

        if let Some(samples) = [
            decode_as!(f64),
            decode_as!(f32),
            decode_as!(u8),
            decode_as!(i8),
            decode_as!(u16),
            decode_as!(i16),
            decode_as!(u32),
            decode_as!(i32),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            return Ok(samples);
        }
        Err(CtbError::RasterRead(
            "the first GeoTIFF band is not a supported numeric sample type".to_owned(),
        ))
    }
}

impl RasterSource for GeoTiffRasterSource {
    fn metadata(&self) -> &RasterMetadata {
        &self.metadata
    }

    fn overview_count(&self) -> u16 {
        u16::try_from(self.file.overview_count()).map_or(u16::MAX, |count| count)
    }

    fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
        self.validate_window(&self.metadata, request)?;
        let samples = self.read_samples(0, request)?;

        if let Some(no_data) = self.metadata.no_data
            && samples.iter().any(|sample| {
                (no_data.is_nan() && sample.is_nan()) || (!no_data.is_nan() && *sample == no_data)
            })
        {
            return Err(CtbError::NoDataEncountered);
        }

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
                metadata: self.metadata.clone(),
            });
        }

        let mut selected: i32 = -1;
        for overview in -1..i32::from(overview_count - 1) {
            let ratio = if overview < 0 {
                1.0
            } else {
                f64::from(self.metadata.width)
                    / f64::from(
                        self.file
                            .overview_ifd(overview as usize)
                            .map_err(|error| CtbError::RasterRead(error.to_string()))?
                            .width(),
                    )
            };
            let next_ratio = f64::from(self.metadata.width)
                / f64::from(
                    self.file
                        .overview_ifd((overview + 1) as usize)
                        .map_err(|error| CtbError::RasterRead(error.to_string()))?
                        .width(),
                );
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
                metadata: self.metadata.clone(),
            });
        }

        let index = selected as usize;
        let overview = self
            .file
            .overview_ifd(index)
            .map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let width = overview.width();
        let height = overview.height();
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
            level: selected as u16 + 1,
            metadata,
        })
    }

    fn read_sampling_window(
        &self,
        level: &crate::raster::SamplingLevel,
        request: WindowRequest,
    ) -> Result<RasterWindow, CtbError> {
        self.validate_window(&level.metadata, request)?;
        let samples = self.read_samples(level.level, request)?;
        if let Some(no_data) = level.metadata.no_data
            && samples.iter().any(|sample| {
                (no_data.is_nan() && sample.is_nan()) || (!no_data.is_nan() && *sample == no_data)
            })
        {
            return Err(CtbError::NoDataEncountered);
        }
        Ok(RasterWindow { request, samples })
    }
}

fn raster_sample_type(
    sample_formats: Vec<u16>,
    bits_per_sample: Vec<u16>,
) -> Result<RasterSampleType, CtbError> {
    let sample_format = sample_formats
        .first()
        .copied()
        .ok_or_else(|| CtbError::UnsupportedRaster("GeoTIFF is missing SampleFormat".to_owned()))?;
    let bits = bits_per_sample.first().copied().ok_or_else(|| {
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

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use geotiff_writer::{CogBuilder, GeoTiffBuilder};
    use ndarray::{Array2, array};

    use super::*;

    fn fixture_path(name: &str) -> std::path::PathBuf {
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

    fn write_float32_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = array![[10.0_f32, 11.0], [12.0, 13.0]];
        GeoTiffBuilder::new(2, 2)
            .geographic_epsg(4326)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0)
            .write_2d(path, samples.view())?;
        Ok(())
    }

    fn write_signed_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = array![[-100_i16, -1], [0, 150]];
        GeoTiffBuilder::new(2, 2)
            .geographic_epsg(4326)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0)
            .write_2d(path, samples.view())?;
        Ok(())
    }

    fn write_unsigned_integer_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = array![[0_u16, 10], [1_000, u16::MAX]];
        GeoTiffBuilder::new(2, 2)
            .geographic_epsg(4326)
            .pixel_scale(0.5, 0.5)
            .origin(-180.0, 90.0)
            .write_2d(path, samples.view())?;
        Ok(())
    }

    fn write_overview_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = Array2::from_shape_fn((8, 8), |(row, column)| (row * 8 + column) as f64);
        let builder = GeoTiffBuilder::new(8, 8)
            .geographic_epsg(4326)
            .pixel_scale(1.0, 1.0)
            .origin(0.0, 8.0)
            .tile_size(16, 16);
        CogBuilder::new(builder)
            .overview_levels(vec![2, 4])
            .write_2d(path, samples.view())?;
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
    fn rejects_a_window_containing_nodata() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("nodata");
        write_fixture(&path, 4326, Some("10"))?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert!(matches!(
            source.read_window(WindowRequest {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                overview: 0,
            }),
            Err(CtbError::NoDataEncountered)
        ));
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
        assert_eq!(half.level, 1);
        assert_eq!(half.metadata.width, 4);
        assert_eq!(half.metadata.height, 4);
        assert_eq!(half.metadata.transform.pixel_width, 2.0);
        assert_eq!(half.metadata.transform.pixel_height, -2.0);

        let quarter = source.sampling_level_for_ratio(4.0)?;
        assert_eq!(quarter.level, 2);
        assert_eq!(quarter.metadata.width, 2);
        assert_eq!(quarter.metadata.height, 2);
        assert_eq!(quarter.metadata.transform.pixel_width, 4.0);
        assert_eq!(quarter.metadata.transform.pixel_height, -4.0);

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
        assert_eq!(window.samples, vec![0.0, 2.0, 16.0, 18.0]);
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
