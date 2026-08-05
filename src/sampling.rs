use crate::{
    CtbError,
    grid::Bounds,
    raster::{RasterSampleType, RasterSource, SamplingLevel, WindowRequest},
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

/// Round a resampled value to match GDAL's warp working data type.
///
/// GDAL resolves the warp working data type from the source band via
/// `GDALWarpResolveWorkingDataType`, which runs inside `GDALCreateWarpedVRT`
/// *before* `hDstDS` is assigned (vrtwarped.cpp:398-399). For an Int32 source
/// the working type becomes `GDT_Int32` and the VRT band is created as Int32.
/// The warp kernel accumulates in double but stores the result through
/// `ClampRoundAndAvoidNoData<T>` (gdalwarpkernel.cpp:1844-1862): signed
/// integers use `floor(dfReal + 0.5)`, unsigned use
/// `static_cast<T>(dfReal + 0.5)`, both preceded by range clamping.
fn round_to_working_type(value: f64, sample_type: RasterSampleType) -> f64 {
    match sample_type {
        RasterSampleType::Float32 | RasterSampleType::Float64 => value,
        RasterSampleType::Signed8 => round_clamped(value, -128.0, 127.0),
        RasterSampleType::Unsigned8 => round_clamped(value, 0.0, 255.0),
        RasterSampleType::Signed16 => round_clamped(value, -32768.0, 32767.0),
        RasterSampleType::Unsigned16 => round_clamped(value, 0.0, 65535.0),
        RasterSampleType::Signed32 => round_clamped(value, -2147483648.0, 2147483647.0),
        RasterSampleType::Unsigned32 => round_clamped(value, 0.0, 4294967295.0),
    }
}

fn round_clamped(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        (value + 0.5).floor()
    }
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
            let column = clamped_pixel(pixel_x.floor(), level.data_width);
            let row = clamped_pixel(pixel_y.floor(), level.data_height);
            read_sample(source, level, column, row)
        }
        ResamplingMethod::Bilinear => bilinear(source, level, pixel_x - 0.5, pixel_y - 0.5),
        ResamplingMethod::Cubic | ResamplingMethod::CubicSpline | ResamplingMethod::Lanczos => {
            filtered_sample(source, level, pixel_x - 0.5, pixel_y - 0.5, method)
        }
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
    let value = match method {
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
        ResamplingMethod::Cubic | ResamplingMethod::CubicSpline | ResamplingMethod::Lanczos => {
            sample_at_level_with_nearest_support(source, level, world_x, world_y, method, true)
        }
    }?;
    Ok(round_to_working_type(value, level.metadata.sample_type))
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
        level.data_width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        level.data_height,
    );
    let (Some((first_column, last_column)), Some((first_row, last_row))) = (columns, rows) else {
        return Ok(Vec::new());
    };

    let mut samples = Vec::new();
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            let sample = read_sample_raw(source, level, column, row)?;
            if sample.is_finite() {
                samples.push(sample);
            }
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
    sample_with_footprint_raster_tiler_level(source, &level, world_x, world_y, footprint, method)
}

pub fn sample_with_footprint_raster_tiler_level(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    world_x: f64,
    world_y: f64,
    footprint: Bounds,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    let value = match method {
        ResamplingMethod::Average => average_at(source, level, footprint, world_x, world_y),
        ResamplingMethod::Max => extrema_at(source, level, footprint, true),
        ResamplingMethod::Min => extrema_at(source, level, footprint, false),
        ResamplingMethod::Mode => mode_at(source, level, footprint),
        ResamplingMethod::Med => quantile_at(source, level, footprint, 0.5),
        ResamplingMethod::Q1 => quantile_at(source, level, footprint, 0.25),
        ResamplingMethod::Q3 => quantile_at(source, level, footprint, 0.75),
        ResamplingMethod::Nearest | ResamplingMethod::Bilinear => {
            // GDAL's GWKGeneralCase rejects destination pixels whose
            // transformed centre maps outside the source pixel index range.
            // For direct-source (same CRS) this is equivalent to a centre-
            // bounds test against the source extent.
            let source_bounds = level
                .metadata
                .transform
                .bounds(level.metadata.width, level.metadata.height)?;
            if world_x < source_bounds.min_x
                || world_x > source_bounds.max_x
                || world_y < source_bounds.min_y
                || world_y > source_bounds.max_y
            {
                return Ok(0.0);
            }
            sample_at_level_with_nearest_support(source, level, world_x, world_y, method, false)
        }
        ResamplingMethod::Cubic | ResamplingMethod::CubicSpline | ResamplingMethod::Lanczos => {
            let source_bounds = level
                .metadata
                .transform
                .bounds(level.metadata.width, level.metadata.height)?;
            if world_x < source_bounds.min_x
                || world_x > source_bounds.max_x
                || world_y < source_bounds.min_y
                || world_y > source_bounds.max_y
            {
                return Ok(0.0);
            }
            sample_at_level_with_nearest_support(source, level, world_x, world_y, method, false)
        }
    }?;
    Ok(round_to_working_type(value, level.metadata.sample_type))
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
        level.data_width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        level.data_height,
    );
    let (Some((first_column, last_column)), Some((first_row, last_row))) = (columns, rows) else {
        return Ok(0.0);
    };

    let mut result: Option<f64> = None;
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            let sample = read_sample_raw(source, level, column, row)?;
            if !sample.is_finite() {
                continue;
            }
            result = Some(match result {
                Some(current) if maximum => current.max(sample),
                Some(current) => current.min(sample),
                None => sample,
            });
        }
    }
    Ok(result.map_or(0.0, |value| value))
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
        level.data_width,
    );
    let rows = indices_overlapping_footprint(
        footprint.min_y,
        footprint.max_y,
        metadata.transform.origin_y,
        metadata.transform.pixel_height,
        level.data_height,
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
                let sample = read_sample_raw(source, level, column, row)?;
                if !sample.is_finite() {
                    continue;
                }
                sum += sample * overlap_area;
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
    let left = centre_x.floor();
    let top = centre_y.floor();
    let horizontal = centre_x - left;
    let vertical = centre_y - top;
    let x0 = clamped_pixel(left, level.data_width);
    let x1 = clamped_pixel(left + 1.0, level.data_width);
    let y0 = clamped_pixel(top, level.data_height);
    let y1 = clamped_pixel(top + 1.0, level.data_height);

    let top_value = interpolate_valid(
        read_sample_raw(source, level, x0, y0)?,
        read_sample_raw(source, level, x1, y0)?,
        horizontal,
    );
    let bottom_value = interpolate_valid(
        read_sample_raw(source, level, x0, y1)?,
        read_sample_raw(source, level, x1, y1)?,
        horizontal,
    );
    Ok(match (top_value, bottom_value) {
        (Some(top), Some(bottom)) => interpolate(top, bottom, vertical),
        (Some(value), None) | (None, Some(value)) => value,
        (None, None) => 0.0,
    })
}

fn filtered_sample(
    source: &dyn RasterSource,
    level: &SamplingLevel,
    centre_x: f64,
    centre_y: f64,
    method: ResamplingMethod,
) -> Result<f64, CtbError> {
    let radius = match method {
        ResamplingMethod::Cubic | ResamplingMethod::CubicSpline => 2,
        ResamplingMethod::Lanczos => 3,
        _ => {
            return Err(CtbError::UnsupportedRaster(
                "invalid filtered kernel".to_owned(),
            ));
        }
    };
    let start_offset = if radius == 2 { -1 } else { -2 };
    let x_base = centre_x.floor() as i32;
    let y_base = centre_y.floor() as i32;
    let mut weighted_sum = 0.0;
    let mut weight_sum = 0.0;
    for y_offset in start_offset..=radius - 1 {
        let y = y_base + y_offset;
        if y < 0 || y >= level.data_height as i32 {
            continue;
        }
        let y_weight = kernel_weight(centre_y - f64::from(y), method);
        for x_offset in start_offset..=radius - 1 {
            let x = x_base + x_offset;
            if x < 0 || x >= level.data_width as i32 {
                continue;
            }
            let x_weight = kernel_weight(centre_x - f64::from(x), method);
            let weight = x_weight * y_weight;
            if weight == 0.0 {
                continue;
            }
            let sample = read_sample_raw(source, level, x as u32, y as u32)?;
            if !sample.is_finite() {
                continue;
            }
            weighted_sum += sample * weight;
            weight_sum += weight;
        }
    }
    if weight_sum.abs() < f64::EPSILON {
        Ok(0.0)
    } else {
        Ok(weighted_sum / weight_sum)
    }
}

fn kernel_weight(distance: f64, method: ResamplingMethod) -> f64 {
    let absolute = distance.abs();
    match method {
        ResamplingMethod::Cubic => {
            if absolute <= 1.0 {
                distance * distance * (1.5 * absolute - 2.5) + 1.0
            } else if absolute <= 2.0 {
                distance * distance * (-0.5 * absolute + 2.5) - 4.0 * absolute + 2.0
            } else {
                0.0
            }
        }
        ResamplingMethod::CubicSpline => {
            if absolute < 1.0 {
                (4.0 - 6.0 * absolute * absolute + 3.0 * absolute.powi(3)) / 6.0
            } else if absolute < 2.0 {
                (2.0 - absolute).powi(3) / 6.0
            } else {
                0.0
            }
        }
        ResamplingMethod::Lanczos => {
            if absolute >= 3.0 {
                0.0
            } else if absolute == 0.0 {
                1.0
            } else {
                let pi_distance = std::f64::consts::PI * distance;
                (pi_distance.sin() / pi_distance)
                    * ((pi_distance / 3.0).sin() / (pi_distance / 3.0))
            }
        }
        _ => 0.0,
    }
}

fn interpolate(first: f64, second: f64, proportion: f64) -> f64 {
    first + (second - first) * proportion
}

fn interpolate_valid(first: f64, second: f64, proportion: f64) -> Option<f64> {
    match (first.is_finite(), second.is_finite()) {
        (true, true) => Some(interpolate(first, second, proportion)),
        (true, false) => Some(first),
        (false, true) => Some(second),
        (false, false) => None,
    }
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
    let sample = read_sample_raw(source, level, x, y)?;
    Ok(if sample.is_finite() { sample } else { 0.0 })
}

fn read_sample_raw(
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
    fn continuous_kernels_sample_pixel_centres_and_edges() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        for method in [
            ResamplingMethod::Cubic,
            ResamplingMethod::CubicSpline,
            ResamplingMethod::Lanczos,
        ] {
            let centre = sample_at(&source, 1.0, 1.0, method)?;
            assert!(
                (centre - 15.0).abs() < 1e-12,
                "{method:?} preserves the bilinear centre value: {centre}"
            );
            assert!(sample_at(&source, 0.0, 2.0, method)?.is_finite());
        }
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
    fn raster_tiler_supports_all_cli_resampling_methods() -> Result<(), CtbError> {
        let source = TestRaster::new()?;
        let footprint = Bounds::new(0.0, 0.0, 2.0, 2.0)?;
        for method in [
            ResamplingMethod::Nearest,
            ResamplingMethod::Bilinear,
            ResamplingMethod::Cubic,
            ResamplingMethod::CubicSpline,
            ResamplingMethod::Lanczos,
            ResamplingMethod::Average,
            ResamplingMethod::Mode,
            ResamplingMethod::Max,
            ResamplingMethod::Min,
            ResamplingMethod::Med,
            ResamplingMethod::Q1,
            ResamplingMethod::Q3,
        ] {
            let value = sample_with_footprint_raster_tiler(&source, 1.0, 1.0, footprint, method)?;
            assert!(value.is_finite(), "{method:?} returned a finite sample");
        }
        Ok(())
    }

    #[test]
    fn nodata_is_skipped_and_all_nodata_uses_destination_zero() -> Result<(), CtbError> {
        let mut source = TestRaster::new()?;
        source.samples[0] = f64::NAN;
        let footprint = Bounds::new(0.0, 0.0, 2.0, 2.0)?;
        for method in [
            ResamplingMethod::Nearest,
            ResamplingMethod::Bilinear,
            ResamplingMethod::Cubic,
            ResamplingMethod::CubicSpline,
            ResamplingMethod::Lanczos,
            ResamplingMethod::Average,
            ResamplingMethod::Mode,
            ResamplingMethod::Max,
            ResamplingMethod::Min,
            ResamplingMethod::Med,
            ResamplingMethod::Q1,
            ResamplingMethod::Q3,
        ] {
            let value = sample_with_footprint_raster_tiler(&source, 1.0, 1.0, footprint, method)?;
            assert!(value.is_finite(), "{method:?} filtered NoData");
        }

        source.samples.fill(f64::NAN);
        for method in [
            ResamplingMethod::Nearest,
            ResamplingMethod::Bilinear,
            ResamplingMethod::Cubic,
            ResamplingMethod::CubicSpline,
            ResamplingMethod::Lanczos,
            ResamplingMethod::Average,
            ResamplingMethod::Mode,
            ResamplingMethod::Max,
            ResamplingMethod::Min,
            ResamplingMethod::Med,
            ResamplingMethod::Q1,
            ResamplingMethod::Q3,
        ] {
            assert_eq!(
                sample_with_footprint_raster_tiler(&source, 1.0, 1.0, footprint, method,)?,
                0.0,
                "{method:?} uses the destination initial value",
            );
        }
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

    #[test]
    fn round_to_working_type_matches_gdal_clamp_round() {
        // Float types pass through unchanged.
        assert_eq!(round_to_working_type(103.5556, RasterSampleType::Float32), 103.5556);
        assert_eq!(round_to_working_type(103.5556, RasterSampleType::Float64), 103.5556);
        // Signed integers: floor(x + 0.5) — GDAL ClampRoundAndAvoidNoData signed path.
        assert_eq!(round_to_working_type(103.5556, RasterSampleType::Signed32), 104.0);
        assert_eq!(round_to_working_type(103.4, RasterSampleType::Signed32), 103.0);
        assert_eq!(round_to_working_type(-3.6, RasterSampleType::Signed32), -4.0);
        assert_eq!(round_to_working_type(-3.4, RasterSampleType::Signed32), -3.0);
        // Unsigned integers: clamp to [0, max], then floor(x + 0.5).
        assert_eq!(round_to_working_type(103.5556, RasterSampleType::Unsigned32), 104.0);
        assert_eq!(round_to_working_type(-0.4, RasterSampleType::Unsigned8), 0.0);
        assert_eq!(round_to_working_type(255.6, RasterSampleType::Unsigned8), 255.0);
    }

    #[test]
    fn average_rounds_to_source_integer_type() -> Result<(), CtbError> {
        // 2x2 source with Signed32 sample type. Footprint partially overlaps
        // pixels 0 (=100) and 1 (=200) so the weighted average is non-integer.
        let source = TestRaster {
            metadata: RasterMetadata {
                width: 2,
                height: 2,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: AffineTransform::north_up(0.0, 2.0, 1.0, -1.0)?,
                no_data: None,
                sample_type: RasterSampleType::Signed32,
            },
            samples: vec![100.0, 200.0, 300.0, 400.0],
        };
        let footprint = Bounds::new(0.0, 1.0, 1.3, 2.0)?;
        // Weighted avg = (100*1.0 + 200*0.3) / 1.3 = 123.0769..., rounded to 123.
        let result = sample_with_footprint(
            &source,
            0.65,
            1.5,
            footprint,
            ResamplingMethod::Average,
        )?;
        assert_eq!(result, 123.0);

        // Same source with Float64 should not round.
        let mut float_source = source;
        float_source.metadata.sample_type = RasterSampleType::Float64;
        let result_f = sample_with_footprint(
            &float_source,
            0.65,
            1.5,
            Bounds::new(0.0, 1.0, 1.3, 2.0)?,
            ResamplingMethod::Average,
        )?;
        assert!((result_f - 160.0 / 1.3).abs() < 1e-12);
        Ok(())
    }
}
