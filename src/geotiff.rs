use std::path::Path;

use geotiff_reader::GeoTiffFile;

use crate::{
    CtbError,
    raster::{AffineTransform, Crs, RasterMetadata, RasterSource, RasterWindow, WindowRequest},
};

/// A restricted, pure-Rust GeoTIFF source for the Phase 1 input contract.
///
/// It deliberately accepts only a single north-up EPSG:4326 band. Reprojection,
/// overview selection, and multi-band interpretation belong to later phases.
pub struct GeoTiffRasterSource {
    file: GeoTiffFile,
    metadata: RasterMetadata,
}

impl GeoTiffRasterSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CtbError> {
        let file =
            GeoTiffFile::open(path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
        let epsg = file.epsg().ok_or(CtbError::MissingCrs)?;
        if epsg != 4326 {
            return Err(CtbError::UnsupportedCrs(format!("EPSG:{epsg}")));
        }
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
        let metadata = RasterMetadata {
            width: file.width(),
            height: file.height(),
            band_count: 1,
            crs: Crs::Epsg4326,
            transform: AffineTransform::north_up(
                transform.origin_x,
                transform.origin_y,
                transform.pixel_width,
                transform.pixel_height,
            )?,
            no_data,
        };
        metadata.transform.bounds(metadata.width, metadata.height)?;

        Ok(Self { file, metadata })
    }

    fn validate_window(&self, request: WindowRequest) -> Result<(), CtbError> {
        if request.overview != 0 {
            return Err(CtbError::UnsupportedRaster(
                "overview reads are scheduled for the performance milestone".to_owned(),
            ));
        }
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
            || end_x > self.metadata.width
            || end_y > self.metadata.height
        {
            return Err(CtbError::InvalidRasterWindow);
        }
        Ok(())
    }

    fn read_samples(&self, request: WindowRequest) -> Result<Vec<f64>, CtbError> {
        macro_rules! decode_as {
            ($sample_type:ty) => {
                self.file
                    .read_band_window::<$sample_type>(
                        0,
                        request.y as usize,
                        request.x as usize,
                        request.height as usize,
                        request.width as usize,
                    )
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
        self.validate_window(request)?;
        let samples = self.read_samples(request)?;

        if let Some(no_data) = self.metadata.no_data
            && samples.iter().any(|sample| {
                (no_data.is_nan() && sample.is_nan()) || (!no_data.is_nan() && *sample == no_data)
            })
        {
            return Err(CtbError::NoDataEncountered);
        }

        Ok(RasterWindow { request, samples })
    }
}

#[cfg(test)]
mod tests {
    use std::{env, fs};

    use geotiff_writer::GeoTiffBuilder;
    use ndarray::array;

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
            .geographic_epsg(epsg)
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

    #[test]
    fn opens_epsg_4326_and_reads_a_window() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg4326");
        write_fixture(&path, 4326, None)?;
        let source = GeoTiffRasterSource::open(&path)?;
        assert_eq!(source.metadata().width, 2);
        assert_eq!(source.metadata().height, 2);
        assert_eq!(source.metadata().crs, Crs::Epsg4326);
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
    fn rejects_a_non_4326_geotiff() -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("epsg3857");
        write_fixture(&path, 3857, None)?;
        assert!(matches!(
            GeoTiffRasterSource::open(&path),
            Err(CtbError::UnsupportedCrs(value)) if value == "EPSG:3857"
        ));
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
    fn converts_float32_dem_samples_to_the_public_f64_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("float32");
        write_float32_fixture(&path)?;
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
}
