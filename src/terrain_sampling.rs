use crate::{
    CtbError,
    grid::{Bounds, GlobalGeodeticGrid, TileCoord, TileGrid},
    raster::{AffineTransform, RasterSource, RasterWindow, transform_bounds, transform_coordinate},
    sampling::{ResamplingMethod, sample_with_footprint_level},
    terrain::{HEIGHTMAP_SAMPLE_COUNT, HEIGHTMAP_TILE_SIZE},
};

const SOURCE_WINDOW_STEP_COUNT: usize = 21;
const SOURCE_WINDOW_EPS: f64 = 1e-6;
const COORD_EPS: f64 = 1e-10;
const APPROX_TRANSFORM_MAX_ERROR: f64 = 0.125;

/// GDAL `VRTWarpedDataset` defaults to `min(nXSize,512)` by
/// `min(nYSize,128)` blocks (`vrtwarped.cpp`).
const VRT_BLOCK_MAX_WIDTH: u32 = 512;
const VRT_BLOCK_MAX_HEIGHT: u32 = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSample {
    pub world_x: f64,
    pub world_y: f64,
    pub footprint: Bounds,
}

/// Plans the source-space pixels of one CTB terrain heightmap.
///
/// CTB creates the warp VRT with one pixel of west and north overlap.  Its
/// public GeoTransform is subsequently shifted back to the tile bounds, but
/// the warp transformer keeps the overlapped source-space pixels.  Therefore
/// a heightmap value represents the centre of that VRT pixel, not a node on
/// the nominal tile bounds.
///
/// The grid tile size and the heightmap tile size are independent: C++ uses
/// the profile default (65 for geodetic, 256 for mercator) for the grid, but
/// always reads `TILE_SIZE` (65) heights from the VRT (`config.hpp`).  For
/// mercator terrain the VRT is 256x256 but only the upper-left 65x65 pixels
/// are read. GDAL warps that VRT as 256x128 blocks, so the pooled source
/// window and margins must use the block size rather than the heightmap size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainSamplePlan {
    bounds: Bounds,
    grid_tile_size: u32,
    warp_block_width: u32,
    warp_block_height: u32,
    heightmap_size: u32,
    cell_width: f64,
    cell_height: f64,
    target_crs: crate::raster::Crs,
}

impl TerrainSamplePlan {
    pub fn new(grid: GlobalGeodeticGrid, coord: TileCoord) -> Result<Self, CtbError> {
        Self::from_grid(&grid, coord)
    }

    pub fn from_grid(grid: &dyn TileGrid, coord: TileCoord) -> Result<Self, CtbError> {
        let bounds = grid.tile_bounds(coord)?;
        let grid_tile_size = grid.tile_size();
        let warp_block_width = grid_tile_size.min(VRT_BLOCK_MAX_WIDTH);
        let warp_block_height = grid_tile_size.min(VRT_BLOCK_MAX_HEIGHT);
        let cells_per_edge = f64::from(grid_tile_size - 1);
        Ok(Self {
            bounds,
            grid_tile_size,
            warp_block_width,
            warp_block_height,
            heightmap_size: HEIGHTMAP_TILE_SIZE as u32,
            cell_width: bounds.width() / cells_per_edge,
            cell_height: bounds.height() / cells_per_edge,
            target_crs: grid.crs(),
        })
    }

    pub fn heightmap_size(self) -> u32 {
        self.heightmap_size
    }

    pub fn sample(self, row: u32, column: u32) -> Option<TerrainSample> {
        if row >= self.heightmap_size || column >= self.heightmap_size {
            return None;
        }
        let world_x = self.bounds.min_x + self.cell_width * (f64::from(column) - 0.5);
        let world_y = self.bounds.max_y - self.cell_height * (f64::from(row) - 0.5);
        let half_width = self.cell_width / 2.0;
        let half_height = self.cell_height / 2.0;
        Some(TerrainSample {
            world_x,
            world_y,
            footprint: Bounds::new(
                world_x - half_width,
                world_y - half_height,
                world_x + half_width,
                world_y + half_height,
            )
            .ok()?,
        })
    }

    pub fn sample_heights(
        self,
        source: &dyn RasterSource,
        method: ResamplingMethod,
    ) -> Result<Vec<f64>, CtbError> {
        // This mirrors the `GDALSuggestedWarpOutput2` ratio used by CTB's
        // overview chooser after reprojection to the source CRS.
        let target_ratio = 1.0 / source.metadata().transform.pixel_width;
        let level = source.sampling_level_for_ratio(target_ratio)?;
        if method == ResamplingMethod::Average && self.target_crs == source.metadata().crs {
            return self.sample_average_with_gdal_window(source, &level);
        }
        let mut heights = Vec::new();
        for row in 0..self.heightmap_size {
            for column in 0..self.heightmap_size {
                let sample = self
                    .sample(row, column)
                    .ok_or(CtbError::InvalidRasterWindow)?;
                let (world_x, world_y) = transform_coordinate(
                    sample.world_x,
                    sample.world_y,
                    &self.target_crs,
                    &source.metadata().crs,
                )?;
                let footprint =
                    transform_bounds(sample.footprint, &self.target_crs, &source.metadata().crs)?;
                heights.push(sample_with_footprint_level(
                    source, &level, world_x, world_y, footprint, method,
                )?);
            }
        }
        Ok(heights)
    }

    /// C++ TerrainTiler keeps the overlap destination transform even though it
    /// later overwrites the VRT's public GeoTransform. The warp kernel then
    /// computes one pooled base-dataset window from that overlap transform and
    /// averages each destination pixel against the pooled source window.
    fn sample_average_with_gdal_window(
        self,
        source: &dyn RasterSource,
        level: &crate::raster::SamplingLevel,
    ) -> Result<Vec<f64>, CtbError> {
        let overlap =
            overlap_destination_transform(self.bounds, self.cell_width, self.cell_height)?;
        let source_transform = &level.metadata.transform;
        let (src_x_off, src_y_off, src_x_size, src_y_size) = compute_source_window(
            &overlap,
            source_transform,
            level.data_width,
            level.data_height,
            self.warp_block_width,
            self.warp_block_height,
        );
        let margin_x = average_margin(self.warp_block_width as i32, src_x_size);
        let margin_y = average_margin(self.warp_block_height as i32, src_y_size);
        if src_x_size == 0 || src_y_size == 0 {
            // GDAL WarpRegion skips the warp entirely for an empty source
            // window and leaves the destination buffer initialized to zero.
            return Ok(vec![0.0; HEIGHTMAP_SAMPLE_COUNT]);
        }
        let width = usize::try_from(src_x_size).map_err(|_| CtbError::InvalidRasterWindow)?;
        let height = usize::try_from(src_y_size).map_err(|_| CtbError::InvalidRasterWindow)?;
        let count = width
            .checked_mul(height)
            .ok_or(CtbError::InvalidRasterWindow)?;
        let window = source.read_sampling_window(
            level,
            crate::raster::WindowRequest {
                x: u32::try_from(src_x_off).map_err(|_| CtbError::InvalidRasterWindow)?,
                y: u32::try_from(src_y_off).map_err(|_| CtbError::InvalidRasterWindow)?,
                width: u32::try_from(src_x_size).map_err(|_| CtbError::InvalidRasterWindow)?,
                height: u32::try_from(src_y_size).map_err(|_| CtbError::InvalidRasterWindow)?,
                overview: level.level,
            },
        )?;
        if window.samples.len() != count {
            return Err(CtbError::RasterRead(format!(
                "source window returned {} samples, expected {count}",
                window.samples.len()
            )));
        }
        let mut heights = Vec::with_capacity(HEIGHTMAP_SAMPLE_COUNT);
        let mut x1 = vec![0.0; self.warp_block_width as usize];
        let mut y1 = vec![0.0; self.warp_block_width as usize];
        let mut x2 = vec![0.0; self.warp_block_width as usize];
        let mut y2 = vec![0.0; self.warp_block_width as usize];
        for row in 0..self.heightmap_size {
            compute_average_line_coords(
                &mut x1,
                &mut y1,
                &mut x2,
                &mut y2,
                f64::from(row),
                &overlap,
                source_transform,
            );
            for column in 0..self.heightmap_size {
                let value = sample_average_pixel(
                    &window,
                    x1[column as usize],
                    y1[column as usize],
                    x2[column as usize],
                    y2[column as usize],
                    margin_x,
                    margin_y,
                );
                heights.push(round_to_working_type(value, level.metadata.sample_type));
            }
        }
        Ok(heights)
    }
}

/// GWKAverageOrModeComputeLineCoords with the CTB approx transformer
/// (gdalwarpkernel.cpp:6873). The transform is run for the full VRT block
/// scanline, even though only the upper-left heightmap-sized columns are
/// consumed.
fn compute_average_line_coords(
    x1: &mut [f64],
    y1: &mut [f64],
    x2: &mut [f64],
    y2: &mut [f64],
    dst_y: f64,
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
) {
    debug_assert_eq!(x1.len(), y1.len());
    debug_assert_eq!(x2.len(), y2.len());
    debug_assert_eq!(x1.len(), x2.len());
    for (index, x) in x1.iter_mut().enumerate() {
        *x = index as f64;
    }
    for y in y1.iter_mut() {
        *y = dst_y;
    }
    for (index, x) in x2.iter_mut().enumerate() {
        *x = (index + 1) as f64;
    }
    for y in y2.iter_mut() {
        *y = dst_y + 1.0;
    }
    let success = gdal_approx_transform_row(x1, y1, dst_gt, src_gt);
    debug_assert!(success);
    let success = gdal_approx_transform_row(x2, y2, dst_gt, src_gt);
    debug_assert!(success);
}

fn overlap_destination_transform(
    bounds: Bounds,
    cell_width: f64,
    cell_height: f64,
) -> Result<AffineTransform, CtbError> {
    AffineTransform::north_up(
        bounds.min_x - cell_width,
        bounds.max_y + cell_height,
        cell_width,
        -cell_height,
    )
}

fn dst_to_src(
    dst_x: f64,
    dst_y: f64,
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
) -> (f64, f64) {
    // GDALGenImgProjTransformer's forward pass is written as
    // `origin + pixel * pixel_size` (gdaltransformer.cpp:3124-3140). The C++
    // build contracts that expression to FMA on this platform, so use mul_add
    // to reproduce the exact oracle bits.
    let world_x = dst_x.mul_add(dst_gt.pixel_width, dst_gt.origin_x);
    let world_y = dst_y.mul_add(dst_gt.pixel_height, dst_gt.origin_y);
    (
        src_gt.world_to_pixel_x(world_x),
        src_gt.world_to_pixel_y(world_y),
    )
}

/// GDALGenImgProjTransform applied to a whole destination scanline.
fn gdal_gen_img_proj_transform_row(
    dst_x: &mut [f64],
    dst_y: &mut [f64],
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
) -> bool {
    for index in 0..dst_x.len() {
        let (src_x, src_y) = dst_to_src(dst_x[index], dst_y[index], dst_gt, src_gt);
        dst_x[index] = src_x;
        dst_y[index] = src_y;
    }
    true
}

/// GDALApproxTransform for the constant-y scanline used by
/// GWKAverageOrModeComputeLineCoords.
fn gdal_approx_transform_row(
    dst_x: &mut [f64],
    dst_y: &mut [f64],
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
) -> bool {
    let n_points = dst_x.len();
    if n_points == 0 {
        return true;
    }
    let n_middle = (n_points - 1) / 2;
    if dst_y[0] != dst_y[n_points - 1]
        || dst_y[0] != dst_y[n_middle]
        || dst_x[0] == dst_x[n_points - 1]
        || dst_x[0] == dst_x[n_middle]
        || n_points <= 5
    {
        return gdal_gen_img_proj_transform_row(dst_x, dst_y, dst_gt, src_gt);
    }

    let mut sme_x = [dst_x[0], dst_x[n_middle], dst_x[n_points - 1]];
    let mut sme_y = [dst_y[0], dst_y[n_middle], dst_y[n_points - 1]];
    if !gdal_gen_img_proj_transform_row(&mut sme_x, &mut sme_y, dst_gt, src_gt) {
        return gdal_gen_img_proj_transform_row(dst_x, dst_y, dst_gt, src_gt);
    }

    gdal_approx_transform_internal(dst_x, dst_y, dst_gt, src_gt, &sme_x, &sme_y)
}

fn gdal_approx_transform_internal(
    dst_x: &mut [f64],
    dst_y: &mut [f64],
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
    sme_x: &[f64; 3],
    sme_y: &[f64; 3],
) -> bool {
    let n_points = dst_x.len();
    let n_middle = (n_points - 1) / 2;
    let df_delta_x = (sme_x[2] - sme_x[0]) / (dst_x[n_points - 1] - dst_x[0]);
    let df_delta_y = (sme_y[2] - sme_y[0]) / (dst_x[n_points - 1] - dst_x[0]);
    // The C++ build contracts the interpolation expressions to FMA; without
    // mul_add the recursive approximation drifts 1e-14 and changes rounded
    // Mercator terrain values on source-pixel boundaries.
    let df_error = ((dst_x[n_middle] - dst_x[0]).mul_add(df_delta_x, sme_x[0]) - sme_x[1]).abs()
        + ((dst_x[n_middle] - dst_x[0]).mul_add(df_delta_y, sme_y[0]) - sme_y[1]).abs();

    if df_error <= APPROX_TRANSFORM_MAX_ERROR {
        for index in (0..n_points).rev() {
            let df_dist = dst_x[index] - dst_x[0];
            dst_x[index] = df_dist.mul_add(df_delta_x, sme_x[0]);
            dst_y[index] = df_dist.mul_add(df_delta_y, sme_y[0]);
        }
        return true;
    }

    let x_middle = [
        dst_x[(n_middle - 1) / 2],
        dst_x[n_middle - 1],
        dst_x[n_middle + (n_points - n_middle - 1) / 2],
    ];
    let y_middle = [
        dst_y[(n_middle - 1) / 2],
        dst_y[n_middle - 1],
        dst_y[n_middle + (n_points - n_middle - 1) / 2],
    ];

    let use_base_transform_half1 = n_middle <= 5
        || dst_y[0] != dst_y[n_middle - 1]
        || dst_y[0] != dst_y[(n_middle - 1) / 2]
        || dst_x[0] == dst_x[n_middle - 1]
        || dst_x[0] == dst_x[(n_middle - 1) / 2];
    let use_base_transform_half2 = n_points - n_middle <= 5
        || dst_y[n_middle] != dst_y[n_points - 1]
        || dst_y[n_middle] != dst_y[n_middle + (n_points - n_middle - 1) / 2]
        || dst_x[n_middle] == dst_x[n_points - 1]
        || dst_x[n_middle] == dst_x[n_middle + (n_points - n_middle - 1) / 2];

    let mut transformed_middle_x = x_middle;
    let mut transformed_middle_y = y_middle;
    let mut success = true;
    if !use_base_transform_half1 && !use_base_transform_half2 {
        success = gdal_gen_img_proj_transform_row(
            &mut transformed_middle_x,
            &mut transformed_middle_y,
            dst_gt,
            src_gt,
        );
    } else if !use_base_transform_half1 {
        success = gdal_gen_img_proj_transform_row(
            &mut transformed_middle_x[..2],
            &mut transformed_middle_y[..2],
            dst_gt,
            src_gt,
        );
    } else if !use_base_transform_half2 {
        success = gdal_gen_img_proj_transform_row(
            &mut transformed_middle_x[2..],
            &mut transformed_middle_y[2..],
            dst_gt,
            src_gt,
        );
    }

    if !success {
        return fallback_approx_transform_halves(
            dst_x, dst_y, dst_gt, src_gt, n_middle, sme_x, sme_y,
        );
    }

    if !use_base_transform_half1 {
        let half_sme_x = [sme_x[0], transformed_middle_x[0], transformed_middle_x[1]];
        let half_sme_y = [sme_y[0], transformed_middle_y[0], transformed_middle_y[1]];
        if !gdal_approx_transform_internal(
            &mut dst_x[..n_middle],
            &mut dst_y[..n_middle],
            dst_gt,
            src_gt,
            &half_sme_x,
            &half_sme_y,
        ) {
            return false;
        }
    } else if !gdal_gen_img_proj_transform_row(
        &mut dst_x[1..n_middle],
        &mut dst_y[1..n_middle],
        dst_gt,
        src_gt,
    ) {
        return false;
    } else {
        dst_x[0] = sme_x[0];
        dst_y[0] = sme_y[0];
    }

    if !use_base_transform_half2 {
        let half_sme_x = [sme_x[1], transformed_middle_x[2], sme_x[2]];
        let half_sme_y = [sme_y[1], transformed_middle_y[2], sme_y[2]];
        if !gdal_approx_transform_internal(
            &mut dst_x[n_middle..],
            &mut dst_y[n_middle..],
            dst_gt,
            src_gt,
            &half_sme_x,
            &half_sme_y,
        ) {
            return false;
        }
    } else if !gdal_gen_img_proj_transform_row(
        &mut dst_x[n_middle + 1..n_points - 1],
        &mut dst_y[n_middle + 1..n_points - 1],
        dst_gt,
        src_gt,
    ) {
        return false;
    } else {
        dst_x[n_middle] = sme_x[1];
        dst_y[n_middle] = sme_y[1];
        dst_x[n_points - 1] = sme_x[2];
        dst_y[n_points - 1] = sme_y[2];
    }

    true
}

fn fallback_approx_transform_halves(
    dst_x: &mut [f64],
    dst_y: &mut [f64],
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
    n_middle: usize,
    sme_x: &[f64; 3],
    sme_y: &[f64; 3],
) -> bool {
    let n_points = dst_x.len();
    let mut success = gdal_gen_img_proj_transform_row(
        &mut dst_x[1..n_middle],
        &mut dst_y[1..n_middle],
        dst_gt,
        src_gt,
    );
    success &= gdal_gen_img_proj_transform_row(
        &mut dst_x[n_middle + 1..n_points - 1],
        &mut dst_y[n_middle + 1..n_points - 1],
        dst_gt,
        src_gt,
    );
    dst_x[0] = sme_x[0];
    dst_y[0] = sme_y[0];
    dst_x[n_middle] = sme_x[1];
    dst_y[n_middle] = sme_y[1];
    dst_x[n_points - 1] = sme_x[2];
    dst_y[n_points - 1] = sme_y[2];
    success
}

fn round_if_close(value: f64) -> f64 {
    let rounded = value.round();
    if (rounded - value).abs() < SOURCE_WINDOW_EPS {
        rounded
    } else {
        value
    }
}

/// GDALWarpOperation::ComputeSourceWindow for GRA_Average
/// (gdalwarpoperation.cpp:3037). The transformer uses the overview
/// GeoTransform, but psWarpOptions->hSrcDS remains the base dataset, so the
/// final clamp and window size use level.data_width/data_height. The
/// destination edge is sampled with independent X/Y block dimensions.
fn compute_source_window(
    dst_gt: &AffineTransform,
    src_gt: &AffineTransform,
    base_width: u32,
    base_height: u32,
    destination_width: u32,
    destination_height: u32,
) -> (i32, i32, i32, i32) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for step in 0..SOURCE_WINDOW_STEP_COUNT {
        let ratio = step as f64 / (SOURCE_WINDOW_STEP_COUNT - 1) as f64;
        let destination_width_f = f64::from(destination_width);
        let destination_height_f = f64::from(destination_height);
        let x = ratio * destination_width_f;
        let y = ratio * destination_height_f;
        for (dst_x, dst_y) in [
            (x, 0.0),
            (x, destination_height_f),
            (0.0, y),
            (destination_width_f, y),
        ] {
            let (src_x, src_y) = dst_to_src(dst_x, dst_y, dst_gt, src_gt);
            if !src_x.is_finite() || !src_y.is_finite() {
                continue;
            }
            min_x = min_x.min(src_x);
            min_y = min_y.min(src_y);
            max_x = max_x.max(src_x);
            max_y = max_y.max(src_y);
        }
    }

    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return (0, 0, 0, 0);
    }

    let min_x = round_if_close(min_x);
    let min_y = round_if_close(min_y);
    let max_x = round_if_close(max_x);
    let max_y = round_if_close(max_y);

    let base_width_f = f64::from(base_width);
    let base_height_f = f64::from(base_height);
    let min_x_clamped = min_x.max(0.0) as i32;
    let min_y_clamped = min_y.max(0.0) as i32;
    let max_x_clamped = max_x.ceil().min(base_width_f) as i32;
    let max_y_clamped = max_y.ceil().min(base_height_f) as i32;

    let (src_x_off, src_x_size) = if f64::from(max_x_clamped - min_x_clamped) > 0.9 * base_width_f {
        (0i32, base_width as i32)
    } else {
        let offset = min_x_clamped.max(0).min(base_width as i32);
        (
            offset,
            (max_x_clamped - offset)
                .max(0)
                .min(base_width as i32 - offset),
        )
    };
    let (src_y_off, src_y_size) = if f64::from(max_y_clamped - min_y_clamped) > 0.9 * base_height_f
    {
        (0i32, base_height as i32)
    } else {
        let offset = min_y_clamped.max(0).min(base_height as i32);
        (
            offset,
            (max_y_clamped - offset)
                .max(0)
                .min(base_height as i32 - offset),
        )
    };

    (src_x_off, src_y_off, src_x_size, src_y_size)
}

/// GDALWarpKernel::PerformWarp margin verified against the real COG oracle.
///
/// GDAL computes `df_scale` from the current pooled source window size, not
/// from the source/destination pixel-size ratio. X and Y margins use their
/// own window dimensions.
fn average_margin(destination_size: i32, source_size: i32) -> i32 {
    let df_scale = warp_scale(destination_size, source_size);
    2i32.saturating_mul(1i32.max((1.0 / df_scale).ceil() as i32))
}

fn warp_scale(destination_size: i32, source_size: i32) -> f64 {
    let destination = f64::from(destination_size);
    let source = f64::from(source_size);
    let source_extra = 0.0;
    let mut df_scale = destination / (source - source_extra);
    if source >= destination && source <= destination + source_extra {
        df_scale = 1.0;
    }
    if df_scale < 1.0 {
        let df_reciprocal = 1.0 / df_scale;
        let n_reciprocal = (df_reciprocal + 0.5) as i32;
        if (df_reciprocal - f64::from(n_reciprocal)).abs() < 0.05 {
            df_scale = 1.0 / f64::from(n_reciprocal);
        }
    }
    df_scale
}

/// GWKAverageOrModeComputeSourceCoords plus the GRA_Average weighted
/// incremental average loop (gdalwarpkernel.cpp:6919, 7140).
fn sample_average_pixel(
    window: &RasterWindow,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    margin_x: i32,
    margin_y: i32,
) -> f64 {
    let window_width = usize::try_from(window.request.width).expect(
        "RasterWindow width is a u32 from a validated request and fits usize on supported targets",
    );
    let margin_x_f = f64::from(margin_x);
    let margin_y_f = f64::from(margin_y);
    let offset_x = f64::from(window.request.x);
    let offset_y = f64::from(window.request.y);
    let window_width_f = f64::from(window.request.width);
    let window_height_f = f64::from(window.request.height);

    if !(x1 - offset_x >= -margin_x_f
        && x2 - offset_x >= -margin_x_f
        && y1 - offset_y >= -margin_y_f
        && y2 - offset_y >= -margin_y_f
        && x1 - offset_x - window_width_f <= margin_x_f
        && x2 - offset_x - window_width_f <= margin_x_f
        && y1 - offset_y - window_height_f <= margin_y_f
        && y2 - offset_y - window_height_f <= margin_y_f)
    {
        return 0.0;
    }

    let df_x_min = x1.min(x2) - offset_x;
    let df_x_max = x1.max(x2) - offset_x;
    let df_y_min = y1.min(y2) - offset_y;
    let df_y_max = y1.max(y2) - offset_y;

    if !(df_x_max > -COORD_EPS && df_x_min < window_width_f + COORD_EPS) {
        return 0.0;
    }
    if !(df_y_max > -COORD_EPS && df_y_min < window_height_f + COORD_EPS) {
        return 0.0;
    }

    let i_src_x_min = ((df_x_min + COORD_EPS).max(0.0)).floor() as i32;
    let mut i_src_x_max = ((df_x_max - COORD_EPS).ceil().min(window_width_f)) as i32;
    if i_src_x_min == i_src_x_max && f64::from(i_src_x_max) < window_width_f {
        i_src_x_max += 1;
    }
    let i_src_y_min = ((df_y_min + COORD_EPS).max(0.0)).floor() as i32;
    let mut i_src_y_max = ((df_y_max - COORD_EPS).ceil().min(window_height_f)) as i32;
    if i_src_y_min == i_src_y_max && f64::from(i_src_y_max) < window_height_f {
        i_src_y_max += 1;
    }

    let mut value = 0.0;
    let mut total_weight = 0.0;
    for i_src_y in i_src_y_min..i_src_y_max {
        let df_weight_y = compute_weight_y(i_src_y, i_src_y_min, i_src_y_max, df_y_min, df_y_max);
        for i_src_x in i_src_x_min..i_src_x_max {
            let df_weight = compute_weight(
                i_src_x,
                df_weight_y,
                i_src_x_min,
                i_src_x_max,
                df_x_min,
                df_x_max,
            );
            if df_weight <= 0.0 {
                continue;
            }
            let local_x = usize::try_from(i_src_x).expect(
                "loop index is clamped to [0, src_x_size), and the window length was validated",
            );
            let local_y = usize::try_from(i_src_y).expect(
                "loop index is clamped to [0, src_y_size), and the window length was validated",
            );
            let sample = window.samples[local_y * window_width + local_x];
            if !sample.is_finite() {
                continue;
            }
            total_weight += df_weight;
            let ratio = df_weight / total_weight;
            value = ratio.mul_add(sample - value, value);
        }
    }
    value
}

fn compute_weight_y(
    i_src_y: i32,
    i_src_y_min: i32,
    i_src_y_max: i32,
    df_y_min: f64,
    df_y_max: f64,
) -> f64 {
    if i_src_y == i_src_y_min {
        if i_src_y_min + 1 == i_src_y_max {
            1.0
        } else {
            1.0 - (df_y_min - f64::from(i_src_y_min))
        }
    } else if i_src_y + 1 == i_src_y_max {
        1.0 - (f64::from(i_src_y_max) - df_y_max)
    } else {
        1.0
    }
}

fn compute_weight(
    i_src_x: i32,
    df_weight_y: f64,
    i_src_x_min: i32,
    i_src_x_max: i32,
    df_x_min: f64,
    df_x_max: f64,
) -> f64 {
    if i_src_x == i_src_x_min {
        if i_src_x_min + 1 == i_src_x_max {
            df_weight_y
        } else {
            df_weight_y * (1.0 - (df_x_min - f64::from(i_src_x_min)))
        }
    } else if i_src_x + 1 == i_src_x_max {
        df_weight_y * (1.0 - (f64::from(i_src_x_max) - df_x_max))
    } else {
        df_weight_y
    }
}

fn round_to_working_type(value: f64, sample_type: crate::raster::RasterSampleType) -> f64 {
    match sample_type {
        crate::raster::RasterSampleType::Float32 | crate::raster::RasterSampleType::Float64 => {
            value
        }
        crate::raster::RasterSampleType::Signed8 => round_clamped(value, -128.0, 127.0),
        crate::raster::RasterSampleType::Unsigned8 => round_clamped(value, 0.0, 255.0),
        crate::raster::RasterSampleType::Signed16 => round_clamped(value, -32768.0, 32767.0),
        crate::raster::RasterSampleType::Unsigned16 => round_clamped(value, 0.0, 65535.0),
        crate::raster::RasterSampleType::Signed32 => {
            round_clamped(value, -2147483648.0, 2147483647.0)
        }
        crate::raster::RasterSampleType::Unsigned32 => round_clamped(value, 0.0, 4294967295.0),
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

#[cfg(test)]
mod tests {
    use crate::{
        CtbError,
        raster::{
            AffineTransform, Crs, RasterMetadata, RasterSampleType, RasterWindow, WindowRequest,
        },
    };

    use super::*;
    use crate::sampling::sample_with_footprint;

    struct TestRaster {
        metadata: RasterMetadata,
    }

    impl RasterSource for TestRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            let value = f64::from(request.y * 2 + request.x);
            Ok(RasterWindow {
                request,
                samples: vec![value],
            })
        }
    }

    struct ConstantRaster {
        metadata: RasterMetadata,
        value: f64,
    }

    struct GeoTiffWindowRaster {
        metadata: RasterMetadata,
    }

    impl RasterSource for ConstantRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            let width =
                usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
            let height =
                usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
            let count = width
                .checked_mul(height)
                .ok_or(CtbError::InvalidRasterWindow)?;
            Ok(RasterWindow {
                request,
                samples: vec![self.value; count],
            })
        }
    }

    impl RasterSource for GeoTiffWindowRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            // GeoTiffRasterSource::validate_window rejects empty windows before
            // reading; the warp path must therefore avoid issuing them.
            if request.width == 0 || request.height == 0 {
                return Err(CtbError::InvalidRasterWindow);
            }
            let width =
                usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
            let height =
                usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
            let count = width
                .checked_mul(height)
                .ok_or(CtbError::InvalidRasterWindow)?;
            Ok(RasterWindow {
                request,
                samples: vec![0.0; count],
            })
        }
    }

    #[test]
    fn neighbouring_tiles_share_east_west_and_north_south_samples() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        let west = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let east = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 1,
                y: 0,
            },
        )?;
        // FMA-contracted tile_bounds means adjacent edges differ by ~4.4e-15.
        let wx_west = west
            .sample(0, 64)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_x;
        let wx_east = east
            .sample(0, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_x;
        assert!((wx_west - wx_east).abs() < 1e-10);

        let south = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 1,
                x: 0,
                y: 0,
            },
        )?;
        let north = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 1,
                x: 0,
                y: 1,
            },
        )?;
        let wy_south = south
            .sample(0, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_y;
        let wy_north = north
            .sample(64, 0)
            .ok_or(CtbError::InvalidRasterWindow)?
            .world_y;
        assert!((wy_south - wy_north).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn heightmap_size_is_always_65_regardless_of_grid_tile_size() -> Result<(), CtbError> {
        let geodetic = GlobalGeodeticGrid::new(65)?;
        let plan_geo = TerrainSamplePlan::new(
            geodetic,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan_geo.heightmap_size(), HEIGHTMAP_TILE_SIZE as u32);
        assert_eq!(plan_geo.warp_block_width, 65);
        assert_eq!(plan_geo.warp_block_height, 65);

        let mercator = crate::grid::GlobalMercatorGrid::new(256)?;
        let plan_merc = TerrainSamplePlan::from_grid(
            &mercator,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        assert_eq!(plan_merc.heightmap_size(), HEIGHTMAP_TILE_SIZE as u32);
        assert_eq!(plan_merc.warp_block_width, 256);
        assert_eq!(plan_merc.warp_block_height, 128);
        Ok(())
    }

    #[test]
    fn matches_ctb_west_and_north_overlap_at_a_source_pixel_edge() -> Result<(), CtbError> {
        let source = TestRaster {
            metadata: RasterMetadata {
                width: 2,
                height: 2,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: AffineTransform::north_up(-1.0, 1.0, 1.0, -1.0)?,
                no_data: None,
                sample_type: RasterSampleType::Float64,
            },
        };
        let grid = GlobalGeodeticGrid::new(65)?;
        let plan = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;

        let upper = plan.sample(32, 64).ok_or(CtbError::InvalidRasterWindow)?;
        let lower = plan.sample(33, 64).ok_or(CtbError::InvalidRasterWindow)?;
        assert_eq!(upper.footprint, Bounds::new(-2.8125, 0.0, 0.0, 2.8125)?);
        assert_eq!(lower.footprint, Bounds::new(-2.8125, -2.8125, 0.0, 0.0)?);
        assert_eq!(
            sample_with_footprint(
                &source,
                upper.world_x,
                upper.world_y,
                upper.footprint,
                ResamplingMethod::Average,
            )?,
            0.0,
        );
        assert_eq!(
            sample_with_footprint(
                &source,
                lower.world_x,
                lower.world_y,
                lower.footprint,
                ResamplingMethod::Average,
            )?,
            2.0,
        );
        Ok(())
    }

    #[test]
    fn overlap_destination_transform_matches_ctb() -> Result<(), CtbError> {
        let bounds = Bounds::new(-10.0, -5.0, 10.0, 5.0)?;
        assert_eq!(
            overlap_destination_transform(bounds, 2.0, 2.0)?,
            AffineTransform::north_up(-12.0, 7.0, 2.0, -2.0)?,
        );
        Ok(())
    }

    #[test]
    fn compute_source_window_matches_real_cog_oracle() -> Result<(), CtbError> {
        let grid = GlobalGeodeticGrid::new(65)?;
        let source_transform = AffineTransform::north_up(
            108.0 - 1.0 / 7200.0,
            23.0 + 1.0 / 7200.0,
            1.0 / 450.0,
            -1.0 / 450.0,
        )?;
        for (coord, expected) in [
            (
                TileCoord {
                    zoom: 0,
                    x: 1,
                    y: 0,
                },
                (0, 0, 3600, 3600),
            ),
            (
                TileCoord {
                    zoom: 1,
                    x: 3,
                    y: 1,
                },
                (0, 0, 3600, 3600),
            ),
            (
                TileCoord {
                    zoom: 2,
                    x: 6,
                    y: 2,
                },
                (0, 0, 3600, 3600),
            ),
            (
                TileCoord {
                    zoom: 3,
                    x: 12,
                    y: 5,
                },
                (0, 0, 2026, 226),
            ),
            (
                TileCoord {
                    zoom: 4,
                    x: 25,
                    y: 10,
                },
                (0, 0, 2026, 226),
            ),
            (
                TileCoord {
                    zoom: 5,
                    x: 51,
                    y: 20,
                },
                (0, 0, 2026, 226),
            ),
            (
                TileCoord {
                    zoom: 6,
                    x: 102,
                    y: 40,
                },
                (0, 0, 760, 226),
            ),
            (
                TileCoord {
                    zoom: 9,
                    x: 819,
                    y: 321,
                },
                (0, 0, 127, 67),
            ),
            (
                TileCoord {
                    zoom: 9,
                    x: 820,
                    y: 321,
                },
                (124, 0, 161, 67),
            ),
            (
                TileCoord {
                    zoom: 9,
                    x: 821,
                    y: 321,
                },
                (282, 0, 162, 67),
            ),
            (
                TileCoord {
                    zoom: 9,
                    x: 822,
                    y: 321,
                },
                (440, 0, 162, 67),
            ),
            (
                TileCoord {
                    zoom: 14,
                    x: 26214,
                    y: 10194,
                },
                (0, 447, 4, 6),
            ),
        ] {
            let plan = TerrainSamplePlan::new(grid, coord)?;
            let overlap =
                overlap_destination_transform(plan.bounds, plan.cell_width, plan.cell_height)?;
            assert_eq!(
                compute_source_window(&overlap, &source_transform, 3600, 3600, 65, 65),
                expected,
            );
        }
        Ok(())
    }

    #[test]
    fn compute_source_window_matches_mercator_vrt_block_oracle() -> Result<(), CtbError> {
        let grid = crate::grid::GlobalMercatorGrid::new(256)?;
        let plan = TerrainSamplePlan::from_grid(
            &grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let source_transform = AffineTransform::north_up(
            -20_037_508.342_789_244,
            20_037_508.342_789_244,
            55_659.745_396_636_79,
            -55_659.745_396_636_79,
        )?;
        let overlap =
            overlap_destination_transform(plan.bounds, plan.cell_width, plan.cell_height)?;
        let window = compute_source_window(
            &overlap,
            &source_transform,
            720,
            720,
            plan.warp_block_width,
            plan.warp_block_height,
        );
        assert_eq!(window, (0, 0, 720, 359));
        assert_eq!(average_margin(plan.warp_block_width as i32, window.2), 6);
        assert_eq!(average_margin(plan.warp_block_height as i32, window.3), 6);
        Ok(())
    }

    #[test]
    fn average_margin_matches_gdal_warp_scale() {
        for (destination_size, source_size, expected) in [
            (65, 3600, 112),
            (65, 2026, 64),
            (65, 226, 8),
            (65, 127, 4),
            (65, 67, 2),
            (65, 4, 2),
            (65, 6, 2),
            (65, 16, 2),
            (256, 720, 6),
            (128, 359, 6),
        ] {
            assert_eq!(average_margin(destination_size, source_size), expected,);
        }
        assert_eq!(average_margin(65, 65), 2);
    }

    #[test]
    fn margin_gate_rejects_pixels_outside_the_pooled_window() -> Result<(), CtbError> {
        let window = RasterWindow {
            request: WindowRequest {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
                overview: 0,
            },
            samples: vec![7.0],
        };
        let source = AffineTransform::north_up(0.0, 0.0, 1.0, -1.0)?;
        let far = AffineTransform::north_up(10.0, 10.0, 1.0, -1.0)?;
        let (x1, y1) = dst_to_src(0.0, 0.0, &far, &source);
        let (x2, y2) = dst_to_src(1.0, 1.0, &far, &source);
        assert_eq!(sample_average_pixel(&window, x1, y1, x2, y2, 2, 2), 0.0,);
        let (x1, y1) = dst_to_src(0.0, 0.0, &source, &source);
        let (x2, y2) = dst_to_src(1.0, 1.0, &source, &source);
        assert_eq!(sample_average_pixel(&window, x1, y1, x2, y2, 2, 2), 7.0,);
        Ok(())
    }

    #[test]
    fn average_weighted_incremental_loop_matches_gdal() -> Result<(), CtbError> {
        let window = RasterWindow {
            request: WindowRequest {
                x: 0,
                y: 0,
                width: 3,
                height: 3,
                overview: 0,
            },
            samples: vec![1.0, 2.0, 3.0, 10.0, 20.0, 30.0, 100.0, 200.0, 300.0],
        };
        let source = AffineTransform::north_up(0.0, 0.0, 1.0, -1.0)?;
        let destination = AffineTransform::north_up(0.0, 0.0, 2.0, -2.0)?;
        let mut x1 = vec![0.0; 256];
        let mut y1 = vec![0.0; 256];
        let mut x2 = vec![0.0; 256];
        let mut y2 = vec![0.0; 256];
        compute_average_line_coords(
            &mut x1,
            &mut y1,
            &mut x2,
            &mut y2,
            0.25,
            &destination,
            &source,
        );
        // A real VRT block scanline samples destination pixel edges at integer
        // column 0 (0..1), not at synthetic half-pixel offsets. The source
        // footprint is therefore (0,0.5)-(2,2.5), giving a weighted mean of
        // 45.375 for this 3x3 window.
        let value = sample_average_pixel(&window, x1[0], y1[0], x2[0], y2[0], 2, 2);
        assert!((value - 45.375).abs() < 1e-12, "value was {value}");
        Ok(())
    }

    #[test]
    fn compute_weight_functions_match_gdal_boundaries() {
        assert!((compute_weight_y(0, 0, 3, 0.2, 2.8) - 0.8).abs() < 1e-12);
        assert_eq!(compute_weight_y(1, 0, 3, 0.2, 2.8), 1.0);
        assert!((compute_weight_y(2, 0, 3, 0.2, 2.8) - 0.8).abs() < 1e-12);
        assert!((compute_weight(0, 0.5, 0, 3, 0.2, 2.8) - 0.4).abs() < 1e-12);
        assert_eq!(compute_weight(1, 0.5, 0, 3, 0.2, 2.8), 0.5);
        assert!((compute_weight(2, 0.5, 0, 3, 0.2, 2.8) - 0.4).abs() < 1e-12);
    }

    #[test]
    fn mercator_approx_line_coords_match_cpp_oracle() -> Result<(), CtbError> {
        let source = AffineTransform::north_up(
            -20_037_508.342_789_244,
            20_037_508.342_789_244,
            55_659.745_396_636_79,
            -55_659.745_396_636_79,
        )?;
        let destination = AffineTransform::north_up(
            -20_076_797.574_833_93,
            -9_979_464.939_349_936,
            39_289.232_044_684_795,
            -39_289.232_044_684_795,
        )?;
        let mut x1 = vec![0.0; 256];
        let mut y1 = vec![0.0; 256];
        let mut x2 = vec![0.0; 256];
        let mut y2 = vec![0.0; 256];
        compute_average_line_coords(
            &mut x1,
            &mut y1,
            &mut x2,
            &mut y2,
            46.0,
            &destination,
            &source,
        );
        assert_eq!(x1[49], 33.882_352_941_176_45);
        assert_eq!(y1[49], 571.764_705_882_352_9);
        assert_eq!(x2[49], 34.588_235_294_117_645);
        assert_eq!(y2[49], 572.470_588_235_294_1);
        Ok(())
    }

    #[test]
    fn average_pooled_path_reads_a_full_constant_source() -> Result<(), CtbError> {
        let source = ConstantRaster {
            metadata: RasterMetadata {
                width: 65,
                height: 65,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: AffineTransform::north_up(-180.0, 90.0, 360.0 / 65.0, -180.0 / 65.0)?,
                no_data: None,
                sample_type: RasterSampleType::Float64,
            },
            value: 100.0,
        };
        let grid = GlobalGeodeticGrid::new(65)?;
        let plan = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
        )?;
        let heights = plan.sample_heights(&source, ResamplingMethod::Average)?;
        assert_eq!(heights.len(), HEIGHTMAP_TILE_SIZE * HEIGHTMAP_TILE_SIZE);
        assert!(heights.iter().all(|value| (*value - 100.0).abs() < 1e-12));
        Ok(())
    }

    #[test]
    fn empty_average_window_returns_zeros_without_a_zero_read() -> Result<(), CtbError> {
        let source = GeoTiffWindowRaster {
            metadata: RasterMetadata {
                width: 65,
                height: 65,
                band_count: 1,
                crs: Crs::Epsg4326,
                transform: AffineTransform::north_up(-180.0, 90.0, 360.0 / 65.0, -180.0 / 65.0)?,
                no_data: None,
                sample_type: RasterSampleType::Float32,
            },
        };
        let grid = GlobalGeodeticGrid::new(65)?;
        let plan = TerrainSamplePlan::new(
            grid,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 1,
            },
        )?;

        let heights = plan.sample_heights(&source, ResamplingMethod::Average)?;
        assert_eq!(heights.len(), HEIGHTMAP_SAMPLE_COUNT);
        assert!(heights.iter().all(|value| *value == 0.0));
        Ok(())
    }
}
