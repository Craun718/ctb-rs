use crate::{
    CtbError,
    grid::Bounds,
    raster::{RasterSource, SamplingLevel, WindowRequest},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResamplingMethod {
    Nearest,
    Bilinear,
    Cubic,
    CubicSpline,
    Lanczos,
    Average,
    Mode,
    Max,
    Min,
    Med,
    Q1,
    Q3,
}

/// Sample an EPSG:4326, north-up source at a world coordinate.
///
/// The source affine transform maps pixel corners. This function converts that
/// coordinate system to pixel centres only for bilinear interpolation.
pub fn sample_at(
    source: &dyn RasterSource,
    world_x: f64,
    world_y: f64,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    let level = source.sampling_level_for_ratio(1.0)?;
    sample_at_level(source, &level, world_x, world_y, method)
}

pub fn sample_at_level(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    world_x: f64,
    world_y: f64,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    sample_at_level_with_nearest_support(source, level, world_x, world_y, method, true)
}

fn sample_at_level_with_nearest_support(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    world_x: f64,
    world_y: f64,
    method: ResamplingMethod,
    nearest_half_pixel_support: bool,
) -> Result<f64, CtbError> {
    if !world_x.is_finite() || !world_y.is_finite() {
        return Err(CtbError::InvalidBounds);
    }
    let metadata = &level.metadata;
    if metadata.width == 0 || metadata.height == 0 {
        return Err(CtbError::InvalidRasterDimensions {
            width: metadata.width,
            height: metadata.height,
        });
    }
    let bounds = metadata.transform.bounds(metadata.width, metadata.height)?;
    let support = if method == ResamplingMethod::Nearest && nearest_half_pixel_support {
        Bounds::new(
            bounds.min_x - metadata.transform.pixel_width.abs() / 2.0,
            bounds.min_y - metadata.transform.pixel_height.abs() / 2.0,
            bounds.max_x + metadata.transform.pixel_width.abs() / 2.0,
            bounds.max_y + metadata.transform.pixel_height.abs() / 2.0,
        )?
    } else {
        bounds
    };
    if world_x < support.min_x
        || world_x > support.max_x
        || world_y < support.min_y
        || world_y > support.max_y
    {
        return Ok(0.0);
    }

    let pixel_x = (world_x - metadata.transform.origin_x) / metadata.transform.pixel_width;
    let pixel_y = (world_y - metadata.transform.origin_y) / metadata.transform.pixel_height;
    match method {
        ResamplingMethod::Nearest => {
            let column = clamped_pixel(pixel_x.floor(), metadata.width);
            let row = clamped_pixel(pixel_y.floor(), metadata.height);
            read_sample(source, level, column, row)
        }
        ResamplingMethod::Bilinear => bilinear(source, level, pixel_x - 0.5, pixel_y - 0.5),
        ResamplingMethod::Average => Err(CtbError::UnsupportedRaster(
            "average resampling requires an output-pixel footprint".to_owned(),
        )),
        method => Err(CtbError::UnsupportedRaster(format!(
            "{method:?} resampling is not implemented yet"
        ))),
    }
}

/// Sample a target pixel with both its centre and full world-coordinate footprint.
pub fn sample_with_footprint(
    source: &dyn RasterSource,
    world_x: f64,
    world_y: f64,
    footprint: Bounds,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    let level = source.sampling_level_for_ratio(1.0)?;
    sample_with_footprint_level(source, &level, world_x, world_y, footprint, method)
}

pub fn sample_with_footprint_level(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    world_x: f64,
    world_y: f64,
    footprint: Bounds,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    match method {
        ResamplingMethod::Average => average_at(source, level, footprint, world_x, world_y),
        ResamplingMethod::Max => extrema_at(source, level, footprint, true),
        ResamplingMethod::Min => extrema_at(source, level, footprint, false),
        ResamplingMethod::Mode => mode_at(source, level, footprint),
        ResamplingMethod::Med => quantile_at(source, level, footprint, 0.5),
        ResamplingMethod::Q1 => quantile_at(source, level, footprint, 0.25),
        ResamplingMethod::Q3 => quantile_at(source, level, footprint, 0.75),
        ResamplingMethod::Nearest | ResamplingMethod::Bilinear => {
            sample_at_level(source, level, world_x, world_y, method)
        }
        method => Err(CtbError::UnsupportedRaster(format!(
            "{method:?} resampling is not implemented yet"
        ))),
    }
}

fn samples_in_footprint(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    footprint: Bounds,
) -> Result<Vec<f64>, CtbError> {
    let metadata = &level.metadata;
    let columns = indices_overlapping_footprint(
        footprint.min_x,
        footprint.max_x,
        metadata.transform.origin_x,
        metadata.transform.pixel_width,
        metadata.width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        metadata.height,
    );
    let (Some((first_column, last_column)), Some((first_row, last_row))) = (columns, rows) else {
        return Ok(Vec::new());
    };

    let mut samples = Vec::new();
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            samples.push(read_sample(source, level, column, row)?);
        }
    }
    Ok(samples)
}

fn mode_at(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    footprint: Bounds,
) -> Result<f64, CtbError> {
    let samples = samples_in_footprint(source, level, footprint)?;
    let mut best: Option<(f64, usize, usize)> = None;
    for (index, &sample) in samples.iter().enumerate() {
        let count = samples[..=index]
            .iter()
            .filter(|candidate| **candidate == sample)
            .count()
            + samples[index + 1..]
                .iter()
                .filter(|candidate| **candidate == sample)
                .count();
        if best.is_none_or(|(_, best_count, best_index)| {
            count > best_count || (count == best_count && index < best_index)
        }) {
            best = Some((sample, count, index));
        }
    }
    Ok(best.map_or(0.0, |(sample, _, _)| sample))
}

fn quantile_at(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    footprint: Bounds,
    quantile: f64,
) -> Result<f64, CtbError> {
    let mut samples = samples_in_footprint(source, level, footprint)?;
    if samples.is_empty() {
        return Ok(0.0);
    }
    samples.sort_by(f64::total_cmp);
    let rank = (quantile * samples.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(samples.len() - 1);
    Ok(samples[index])
}

/// RasterTiler sampling uses the warped VRT's strict source bounds. This is
/// distinct from the terrain helper's nearest edge-support compatibility path.
pub fn sample_with_footprint_raster_tiler(
    source: &dyn RasterSource,
    world_x: f64,
    world_y: f64,
    footprint: Bounds,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    let level = source.sampling_level_for_ratio(1.0)?;
    match method {
        ResamplingMethod::Average => average_at(source, &level, footprint, world_x, world_y),
        ResamplingMethod::Max => extrema_at(source, &level, footprint, true),
        ResamplingMethod::Min => extrema_at(source, &level, footprint, false),
        ResamplingMethod::Nearest | ResamplingMethod::Bilinear => {
            sample_at_level_with_nearest_support(source, &level, world_x, world_y, method, false)
        }
        method => Err(CtbError::UnsupportedRaster(format!(
            "{method:?} resampling is not implemented yet"
        ))),
    }
}

fn extrema_at(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    footprint: Bounds,
    maximum: bool,
) -> Result<f64, CtbError> {
    let metadata = &level.metadata;
    let columns = indices_overlapping_footprint(
        footprint.min_x,
        footprint.max_x,
        metadata.transform.origin_x,
        metadata.transform.pixel_width,
        metadata.width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        metadata.height,
    );
    let (Some((first_column, last_column)), Some((first_row, last_row))) = (columns, rows) else {
        return Ok(0.0);
    };

    let mut result: Option<f64> = None;
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            let sample = read_sample(source, level, column, row)?;
            result = Some(match result {
                Some(current) if maximum => current.max(sample),
                Some(current) => current.min(sample),
                None => sample,
            });
        }
    }
    Ok(result.expect("non-empty source index ranges produce at least one extrema sample"))
}

fn average_at(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    footprint: Bounds,
    _fallback_x: f64,
    _fallback_y: f64,
) -> Result<f64, CtbError> {
    let metadata = &level.metadata;
    let columns = indices_overlapping_footprint(
        footprint.min_x,
        footprint.max_x,
        metadata.transform.origin_x,
        metadata.transform.pixel_width,
        metadata.width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        metadata.height,
    );
    let (Some((first_column, last_column)), Some((first_row, last_row))) = (columns, rows) else {
        return Ok(0.0);
    };

    let mut sum = 0.0;
    let mut total_area = 0.0;
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            let pixel_x = pixel_interval(
                metadata.transform.origin_x,
                metadata.transform.pixel_width,
                column,
            );
            let pixel_y = pixel_interval(
                metadata.transform.origin_y,
                metadata.transform.pixel_height,
                row,
            );
            let overlap_area =
                overlap_length(footprint.min_x, footprint.max_x, pixel_x.0, pixel_x.1)
                    * overlap_length(footprint.min_y, footprint.max_y, pixel_y.0, pixel_y.1);
            if overlap_area > 0.0 {
                sum += read_sample(source, level, column, row)? * overlap_area;
                total_area += overlap_area;
            }
        }
    }
    if total_area == 0.0 {
        Ok(0.0)
    } else {
        Ok(sum / total_area)
    }
}

fn indices_overlapping_footprint(
    first_world: f64,
    last_world: f64,
    origin: f64,
    pixel_size: f64,
    limit: u32,
) -> Option<(u32, u32)> {
    if limit == 0 {
        return None;
    }
    let first = (first_world - origin) / pixel_size;
    let last = (last_world - origin) / pixel_size;
    let lower = first.min(last).floor();
    let upper = first.max(last).ceil() - 1.0;
    let max_index = f64::from(limit - 1);
    if upper < 0.0 || lower > max_index {
        return None;
    }
    let first_index = lower.max(0.0) as u32;
    let last_index = upper.min(max_index) as u32;
    if first_index > last_index {
        return None;
    }
    Some((first_index, last_index))
}

fn pixel_interval(origin: f64, pixel_size: f64, index: u32) -> (f64, f64) {
    let first = origin + pixel_size * f64::from(index);
    let second = first + pixel_size;
    (first.min(second), first.max(second))
}

fn overlap_length(first_min: f64, first_max: f64, second_min: f64, second_max: f64) -> f64 {
    first_max.min(second_max) - first_min.max(second_min)
}

fn bilinear(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    centre_x: f64,
    centre_y: f64,
) -> Result<f64, CtbError> {
    let metadata = &level.metadata;
    let left = centre_x.floor();
    let top = centre_y.floor();
    let horizontal = centre_x - left;
    let vertical = centre_y - top;
    let x0 = clamped_pixel(left, metadata.width);
    let x1 = clamped_pixel(left + 1.0, metadata.width);
    let y0 = clamped_pixel(top, metadata.height);
    let y1 = clamped_pixel(top + 1.0, metadata.height);

    let top_value = interpolate(
        read_sample(source, level, x0, y0)?,
        read_sample(source, level, x1, y0)?,
        horizontal,
    );
    let bottom_value = interpolate(
        read_sample(source, level, x0, y1)?,
        read_sample(source, level, x1, y1)?,
        horizontal,
    );
    Ok(interpolate(top_value, bottom_value, vertical))
}

fn interpolate(first: f64, second: f64, proportion: f64) -> f64 {
    first + (second - first) * proportion
}

fn clamped_pixel(value: f64, limit: u32) -> u32 {
    value.clamp(0.0, f64::from(limit.saturating_sub(1))) as u32
}

fn read_sample(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    x: u32,
    y: u32,
) -> Result<f64, CtbError> {
    let window = source.read_sampling_window(
        level,
        WindowRequest {
            x,
            y,
            width: 1,
            height: 1,
            overview: level.level,
        },
    )?;
    match window.samples.as_slice() {
        [sample] => Ok(*sample),
        _ => Err(CtbError::RasterRead(
            "a one-pixel raster request did not return exactly one sample".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CtbError,
        raster::{AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterWindow},
    };

    use super::*;

    struct TestRaster {
        metadata: RasterMetadata,
        samples: Vec<f64>,
    }

    impl TestRaster {
        fn new() -> Result<Self, CtbError> {
            Ok(Self {
                metadata: RasterMetadata {
                    width: 2,
                    height: 2,
                    band_count: 1,
                    crs: Crs::Epsg4326,
                    transform: AffineTransform::north_up(0.0, 2.0, 1.0, -1.0)?,
                    no_data: None,
                    sample_type: RasterSampleType::Float64,
                },
                samples: vec![0.0, 10.0, 20.0, 30.0],
            })
        }
    }

    impl RasterSource for TestRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            if request.width != 1
                || request.height != 1
                || request.x >= self.metadata.width
                || request.y >= self.metadata.height
            {
                return Err(CtbError::InvalidRasterWindow);
            }
            let index = request.y as usize * self.metadata.width as usize + request.x as usize;
            Ok(RasterWindow {
                request,
                samples: vec![self.samples[index]],
            })
        }
    }

    #[test]
    fn nearest_uses_the_pixel_containing_the_world_coordinate() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        assert_eq!(
            sample_at(&source, 1.2, 0.8, ResamplingMethod::Nearest)?,
            30.0
        );
        assert_eq!(
            sample_at(&source, 2.0, 0.0, ResamplingMethod::Nearest)?,
            30.0
        );
        Ok(())
    }

    #[test]
    fn nearest_clamps_within_half_a_source_pixel_of_the_edge() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        assert_eq!(
            sample_at(&source, -0.25, 1.0, ResamplingMethod::Nearest)?,
            20.0
        );
        assert_eq!(
            sample_at(&source, -0.51, 1.0, ResamplingMethod::Nearest)?,
            0.0
        );
        Ok(())
    }

    #[test]
    fn bilinear_interpolates_pixel_centres() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        assert_eq!(
            sample_at(&source, 1.0, 1.0, ResamplingMethod::Bilinear)?,
            15.0
        );
        Ok(())
    }

    #[test]
    fn bilinear_clamps_neighbours_at_the_source_edge() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        assert_eq!(
            sample_at(&source, 0.0, 2.0, ResamplingMethod::Bilinear)?,
            0.0
        );
        Ok(())
    }

    #[test]
    fn average_uses_source_pixel_centres_within_the_footprint() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        let footprint = Bounds::new(0.0, 0.0, 2.0, 2.0)?;
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Average)?,
            15.0
        );
        Ok(())
    }

    #[test]
    fn discrete_statistics_use_row_major_samples_and_nearest_rank() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        let footprint = Bounds::new(0.0, 0.0, 2.0, 2.0)?;
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Mode)?,
            0.0,
            "an all-tied mode keeps the first row-major sample"
        );
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Q1)?,
            0.0
        );
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Med)?,
            10.0
        );
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Q3)?,
            20.0
        );
        Ok(())
    }

    #[test]
    fn discrete_statistics_return_destination_initial_value_outside_source() -> Result<(), CtbError>
    {
        let source = TestRaster::new()?;
        let footprint = Bounds::new(3.0, 3.0, 4.0, 4.0)?;
        for method in [
            ResamplingMethod::Mode,
            ResamplingMethod::Med,
            ResamplingMethod::Q1,
            ResamplingMethod::Q3,
        ] {
            assert_eq!(
                sample_with_footprint(&source, 3.5, 3.5, footprint, method)?,
                0.0,
                "{method:?} uses the VRT destination initial value"
            );
        }
        Ok(())
    }

    #[test]
    fn extrema_use_all_source_pixels_overlapping_the_footprint() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        let footprint = Bounds::new(0.0, 0.0, 2.0, 2.0)?;
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Max)?,
            30.0
        );
        assert_eq!(
            sample_with_footprint(&source, 1.0, 1.0, footprint, ResamplingMethod::Min)?,
            0.0
        );
        Ok(())
    }

    #[test]
    fn source_outside_samples_use_the_ctb_warp_destination_default() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        assert_eq!(
            sample_at(&source, -1.0, 1.0, ResamplingMethod::Nearest)?,
            0.0
        );
        assert_eq!(
            sample_with_footprint(
                &source,
                -1.0,
                1.0,
                Bounds::new(-2.0, 0.0, -1.0, 2.0)?,
                ResamplingMethod::Average,
            )?,
            0.0
        );
        Ok(())
    }
}
