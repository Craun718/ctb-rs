use std::{collections::VecDeque, path::Path, sync::Mutex};

use oxigeo::{
    DatasetFormat, RasterDataType,
    core_types::{buffer::convert_raw_into, io::FileDataSource},
    geotiff::{
        CogReader, GeoTiffReader, RasterType,
        tiff::{ImageInfo, TiffFile, TiffTag},
    },
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

const GEOTIFF_BLOCK_CACHE_BUDGET_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BlockKey {
    level: u16,
    tile_x: u32,
    tile_y: u32,
}

#[derive(Debug, Clone, Copy)]
struct BlockGeometry {
    width: u64,
    height: u64,
    block_width: u64,
    block_height: u64,
    blocks_across: u32,
    blocks_down: u32,
    is_tiled: bool,
    data_type: RasterDataType,
}

impl BlockGeometry {
    fn from_info(info: &ImageInfo, level: usize) -> Result<Self, CtbError> {
        let data_type = info.data_type().ok_or_else(|| {
            CtbError::RasterRead(format!(
                "level {level} has no supported GeoTIFF sample type"
            ))
        })?;
        let is_tiled = info.is_tiled();
        let (block_width, block_height, blocks_across, blocks_down) = if is_tiled {
            let block_width = u64::from(info.tile_width.unwrap_or_default());
            let block_height = u64::from(info.tile_height.unwrap_or_default());
            if block_width == 0 || block_height == 0 {
                return Err(CtbError::RasterRead(format!(
                    "level {level} declares zero-sized GeoTIFF tiles"
                )));
            }
            (
                block_width,
                block_height,
                u32::try_from(info.width.div_ceil(block_width)).map_err(|_| {
                    CtbError::RasterRead(format!("level {level} tile grid is too wide"))
                })?,
                u32::try_from(info.height.div_ceil(block_height)).map_err(|_| {
                    CtbError::RasterRead(format!("level {level} tile grid is too tall"))
                })?,
            )
        } else {
            let rows_per_strip = u64::from(info.rows_per_strip.unwrap_or(info.height as u32));
            if rows_per_strip == 0 {
                return Err(CtbError::RasterRead(format!(
                    "level {level} declares zero-sized GeoTIFF strips"
                )));
            }
            (
                info.width,
                rows_per_strip,
                1,
                u32::try_from(info.height.div_ceil(rows_per_strip)).map_err(|_| {
                    CtbError::RasterRead(format!("level {level} strip grid is too tall"))
                })?,
            )
        };
        Ok(Self {
            width: info.width,
            height: info.height,
            block_width,
            block_height,
            blocks_across,
            blocks_down,
            is_tiled,
            data_type,
        })
    }
}

#[derive(Debug, Clone)]
struct CachedBlock {
    key: BlockKey,
    bytes: Vec<u8>,
    width: u64,
    height: u64,
    data_type: RasterDataType,
}

#[derive(Debug)]
struct GeoTiffBlockCache {
    geometries: Vec<BlockGeometry>,
    blocks: VecDeque<CachedBlock>,
    used_bytes: usize,
    budget_bytes: usize,
}

impl GeoTiffBlockCache {
    fn new(geometries: Vec<BlockGeometry>) -> Self {
        Self {
            geometries,
            blocks: VecDeque::new(),
            used_bytes: 0,
            budget_bytes: GEOTIFF_BLOCK_CACHE_BUDGET_BYTES,
        }
    }

    fn geometry(&self, level: u16) -> Result<&BlockGeometry, CtbError> {
        self.geometries.get(usize::from(level)).ok_or_else(|| {
            CtbError::RasterRead(format!(
                "GeoTIFF level {level} has no cached block geometry"
            ))
        })
    }

    fn cached_block(
        &mut self,
        file: &GeoTiffReader<FileDataSource>,
        level: u16,
        tile_x: u32,
        tile_y: u32,
    ) -> Result<&CachedBlock, CtbError> {
        let geometry = *self.geometry(level)?;
        if tile_x >= geometry.blocks_across || tile_y >= geometry.blocks_down {
            let layout = if geometry.is_tiled { "tile" } else { "strip" };
            return Err(CtbError::RasterRead(format!(
                "GeoTIFF {layout} ({tile_x},{tile_y}) is out of bounds at level {level}"
            )));
        }
        let key = BlockKey {
            level,
            tile_x,
            tile_y,
        };
        if let Some(position) = self.blocks.iter().position(|block| block.key == key) {
            let block = self
                .blocks
                .remove(position)
                .expect("position was found by iteration");
            self.blocks.push_front(block);
        } else {
            let buffer = file
                .read_tile_band_buffer(usize::from(level), 0, tile_x, tile_y)
                .map_err(|error| CtbError::RasterRead(error.to_string()))?;
            let block = CachedBlock {
                key,
                width: buffer.width(),
                height: buffer.height(),
                data_type: buffer.data_type(),
                bytes: buffer.into_bytes(),
            };
            self.insert(block);
        }
        self.blocks.front().ok_or_else(|| {
            CtbError::RasterRead("GeoTIFF block cache is empty after block load".to_owned())
        })
    }

    fn insert(&mut self, block: CachedBlock) {
        self.used_bytes = self.used_bytes.saturating_add(block.bytes.len());
        self.blocks.push_front(block);
        // Keep the most recently used block even if a single decoded block
        // exceeds the byte budget; all older entries are still evicted.
        while self.used_bytes > self.budget_bytes && self.blocks.len() > 1 {
            if let Some(evicted) = self.blocks.pop_back() {
                self.used_bytes = self.used_bytes.saturating_sub(evicted.bytes.len());
            }
        }
    }

    fn read_window(
        &mut self,
        file: &GeoTiffReader<FileDataSource>,
        level: u16,
        request: WindowRequest,
        samples: &mut [f64],
    ) -> Result<(), CtbError> {
        if request.width == 0 || request.height == 0 {
            return Err(CtbError::InvalidRasterWindow);
        }
        let geometry = *self.geometry(level)?;
        let x = u64::from(request.x);
        let y = u64::from(request.y);
        let width = u64::from(request.width);
        let height = u64::from(request.height);
        let first_tile_x = (x / geometry.block_width) as u32;
        let first_tile_y = (y / geometry.block_height) as u32;
        let last_tile_x = ((x + width - 1) / geometry.block_width)
            .min(geometry.width.saturating_sub(1) / geometry.block_width)
            as u32;
        let last_tile_y = ((y + height - 1) / geometry.block_height)
            .min(geometry.height.saturating_sub(1) / geometry.block_height)
            as u32;

        for tile_y in first_tile_y..=last_tile_y {
            let block_y0 = u64::from(tile_y) * geometry.block_height;
            let block_y_end = block_y0
                .saturating_add(geometry.block_height)
                .min(geometry.height);
            let row_start = y.max(block_y0);
            let row_end = (y + height).min(block_y_end);
            if row_end <= row_start {
                continue;
            }
            for tile_x in first_tile_x..=last_tile_x {
                let block_x0 = u64::from(tile_x) * geometry.block_width;
                let block_x_end = block_x0
                    .saturating_add(geometry.block_width)
                    .min(geometry.width);
                let col_start = x.max(block_x0);
                let col_end = (x + width).min(block_x_end);
                if col_end <= col_start {
                    continue;
                }

                let cached = self.cached_block(file, level, tile_x, tile_y)?;
                if cached.data_type != geometry.data_type {
                    return Err(CtbError::RasterRead(format!(
                        "GeoTIFF block cache type changed at level {level} tile ({tile_x},{tile_y})"
                    )));
                }
                let src_col = col_start - block_x0;
                let run = col_end - col_start;
                if src_col >= cached.width {
                    return Err(CtbError::RasterRead(format!(
                        "GeoTIFF block ({tile_x},{tile_y}) at level {level} is smaller than its declared geometry"
                    )));
                }
                let bytes_per_sample = cached.data_type.size_bytes();
                let source_len =
                    usize::try_from(run.checked_mul(bytes_per_sample as u64).ok_or_else(|| {
                        CtbError::RasterRead("GeoTIFF block byte length overflow".to_owned())
                    })?)
                    .map_err(|_| {
                        CtbError::RasterRead("GeoTIFF block byte length overflow".to_owned())
                    })?;
                let out_col = col_start - x;
                let run = usize::try_from(run).map_err(|_| {
                    CtbError::RasterRead("GeoTIFF window width overflow".to_owned())
                })?;
                for row in 0..(row_end - row_start) {
                    let src_row = row_start - block_y0 + row;
                    if src_row >= cached.height {
                        return Err(CtbError::RasterRead(format!(
                            "GeoTIFF block ({tile_x},{tile_y}) at level {level} is smaller than its declared geometry"
                        )));
                    }
                    let source_offset = usize::try_from(
                        (src_row * cached.width + src_col)
                            .checked_mul(bytes_per_sample as u64)
                            .ok_or_else(|| {
                                CtbError::RasterRead(
                                    "GeoTIFF block byte offset overflow".to_owned(),
                                )
                            })?,
                    )
                    .map_err(|_| {
                        CtbError::RasterRead("GeoTIFF block byte offset overflow".to_owned())
                    })?;
                    let source_end = source_offset.checked_add(source_len).ok_or_else(|| {
                        CtbError::RasterRead("GeoTIFF block buffer overflow".to_owned())
                    })?;
                    let source = cached.bytes.get(source_offset..source_end).ok_or_else(|| {
                        CtbError::RasterRead(format!(
                            "GeoTIFF block buffer underrun at level {level} tile ({tile_x},{tile_y})"
                        ))
                    })?;

                    let out_row = row_start - y + row;
                    let dst_offset = usize::try_from(
                        out_row
                            .checked_mul(width)
                            .and_then(|value| value.checked_add(out_col))
                            .ok_or_else(|| {
                                CtbError::RasterRead("GeoTIFF window offset overflow".to_owned())
                            })?,
                    )
                    .map_err(|_| {
                        CtbError::RasterRead("GeoTIFF window offset overflow".to_owned())
                    })?;
                    let dst_end = dst_offset.checked_add(run).ok_or_else(|| {
                        CtbError::RasterRead("GeoTIFF window buffer overflow".to_owned())
                    })?;
                    let destination = samples.get_mut(dst_offset..dst_end).ok_or_else(|| {
                        CtbError::RasterRead("GeoTIFF window buffer underrun".to_owned())
                    })?;
                    convert_raw_into(source, cached.data_type, destination)
                        .map_err(|error| CtbError::RasterRead(error.to_string()))?;
                }
            }
        }
        Ok(())
    }
}

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
    geotiff_block_cache: Option<Mutex<GeoTiffBlockCache>>,
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
        let block_geometries = parse_geotiff_block_geometries(path, file.overview_count())?;
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
            geotiff_block_cache: Some(Mutex::new(GeoTiffBlockCache::new(block_geometries))),
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
            geotiff_block_cache: None,
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
                let cache = self.geotiff_block_cache.as_ref().ok_or_else(|| {
                    CtbError::UnsupportedRaster(
                        "GeoTIFF source has no native block cache".to_owned(),
                    )
                })?;
                let mut cache = cache.lock().map_err(|_| {
                    CtbError::RasterRead("GeoTIFF block cache lock poisoned".to_owned())
                })?;
                cache.read_window(file, level, request, &mut samples)?;
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

fn parse_geotiff_block_geometries(
    path: &Path,
    overview_count: usize,
) -> Result<Vec<BlockGeometry>, CtbError> {
    let source =
        FileDataSource::open(path).map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let tiff = TiffFile::parse(&source).map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let primary = ImageInfo::from_ifd(
        tiff.primary_ifd(),
        &source,
        tiff.byte_order(),
        tiff.header.variant,
    )
    .map_err(|error| CtbError::RasterRead(error.to_string()))?;
    let mut geometries = Vec::with_capacity(overview_count + 1);
    geometries.push(BlockGeometry::from_info(&primary, 0)?);
    for level in 1..=overview_count {
        let info = overview_info_from_tiff(&tiff, &primary, level)?;
        geometries.push(BlockGeometry::from_info(&info, level)?);
    }
    Ok(geometries)
}

fn overview_info_from_tiff(
    tiff: &TiffFile,
    primary: &ImageInfo,
    level: usize,
) -> Result<ImageInfo, CtbError> {
    let byte_order = tiff.byte_order();
    let ifd = tiff.ifds.get(level).ok_or_else(|| {
        CtbError::RasterRead(format!("GeoTIFF overview level {level} has no IFD"))
    })?;
    let scalar = |tag: TiffTag| {
        ifd.get_entry(tag)
            .and_then(|entry| entry.get_u64(byte_order).ok())
    };

    let mut info = primary.clone();
    info.width = scalar(TiffTag::ImageWidth).ok_or_else(|| {
        CtbError::RasterRead(format!("GeoTIFF overview level {level} has no image width"))
    })?;
    info.height = scalar(TiffTag::ImageLength).ok_or_else(|| {
        CtbError::RasterRead(format!(
            "GeoTIFF overview level {level} has no image height"
        ))
    })?;
    // Layout tags are per-level: an overview may be striped even when the
    // full-resolution image is tiled, so they are never inherited.
    info.tile_width = scalar(TiffTag::TileWidth).and_then(|value| u32::try_from(value).ok());
    info.tile_height = scalar(TiffTag::TileLength).and_then(|value| u32::try_from(value).ok());
    info.rows_per_strip = scalar(TiffTag::RowsPerStrip).and_then(|value| u32::try_from(value).ok());
    if let Some(value) = scalar(TiffTag::SamplesPerPixel) {
        info.samples_per_pixel = value as u16;
    }
    if let Some(value) = scalar(TiffTag::BitsPerSample) {
        info.bits_per_sample = vec![value as u16];
    }
    Ok(info)
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
            CogReader, GeoKey, GeoTiffReader, GeoTiffWriter, GeoTiffWriterOptions,
            OverviewResampling, TiffTag, WriterConfig,
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

    fn write_edge_fixture(path: &Path, tiled: bool) -> Result<(), Box<dyn std::error::Error>> {
        let width = 40_u64;
        let height = 24_u64;
        let mut bytes = Vec::with_capacity(usize::try_from(width * height * 4)?);
        for y in 0..height {
            for x in 0..width {
                let value = x as f32 + y as f32 * 100.0;
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut config = WriterConfig::new(width, height, 1, RasterDataType::Float32)
            .with_compression(Compression::None)
            .with_predictor(Predictor::None)
            .with_geo_transform(GeoTransform::north_up(0.0, height as f64, 1.0, -1.0))
            .with_epsg_code(4326)
            .with_overviews(false, OverviewResampling::Nearest);
        if tiled {
            config = config.with_tile_size(16, 16);
        } else {
            config.tile_width = None;
            config.tile_height = None;
        }
        let mut writer = GeoTiffWriter::create(path, config, GeoTiffWriterOptions::default())?;
        writer.write(&bytes)?;
        Ok(())
    }

    fn write_large_overview_fixture(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = (0..4096).map(|value| value as f64).collect::<Vec<_>>();
        let bytes = samples
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        write_bytes(
            path,
            (64, 64),
            RasterDataType::Float64,
            &bytes,
            4326,
            GeoTransform::north_up(0.0, 64.0, 1.0, -1.0),
            FixtureOptions {
                overviews: true,
                ..FixtureOptions::default()
            },
        )
    }

    fn open_direct_reader(
        path: &Path,
    ) -> Result<GeoTiffReader<FileDataSource>, Box<dyn std::error::Error>> {
        let source = FileDataSource::open(path)?;
        GeoTiffReader::open(source).map_err(Into::into)
    }

    fn read_direct_window(
        reader: &GeoTiffReader<FileDataSource>,
        level: u16,
        request: WindowRequest,
    ) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
        let width = usize::try_from(request.width)?;
        let height = usize::try_from(request.height)?;
        let mut samples = vec![0.0_f64; width * height];
        reader.read_window_into_typed::<f64>(
            usize::from(level),
            0,
            u64::from(request.x),
            u64::from(request.y),
            u64::from(request.width),
            u64::from(request.height),
            &mut samples,
        )?;
        Ok(samples)
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
    fn block_cache_matches_direct_read_across_tiled_edge_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("block-cache-tiled-edge");
        write_edge_fixture(&path, true)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 5,
            width: 20,
            height: 18,
            overview: 0,
        };
        let cached = source.read_samples(0, request)?;
        let direct_reader = open_direct_reader(&path)?;
        let direct = read_direct_window(&direct_reader, 0, request)?;
        assert_eq!(cached, direct);

        let repeated = source.read_samples(0, request)?;
        assert_eq!(repeated, direct);
        let cache = source
            .geotiff_block_cache
            .as_ref()
            .ok_or("GeoTIFF cache missing")?
            .lock()
            .map_err(|_| "GeoTIFF cache lock poisoned")?;
        assert_eq!(cache.blocks.len(), 6);
        assert!(cache.geometries[0].is_tiled);
        assert_eq!(cache.geometries[0].blocks_across, 3);
        assert_eq!(cache.geometries[0].blocks_down, 2);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn block_cache_matches_direct_read_across_striped_final_block()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("block-cache-striped-edge");
        write_edge_fixture(&path, false)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 5,
            width: 20,
            height: 18,
            overview: 0,
        };
        let cached = source.read_samples(0, request)?;
        let direct_reader = open_direct_reader(&path)?;
        let direct = read_direct_window(&direct_reader, 0, request)?;
        assert_eq!(cached, direct);

        let repeated = source.read_samples(0, request)?;
        assert_eq!(repeated, direct);
        let cache = source
            .geotiff_block_cache
            .as_ref()
            .ok_or("GeoTIFF cache missing")?
            .lock()
            .map_err(|_| "GeoTIFF cache lock poisoned")?;
        assert_eq!(cache.blocks.len(), 2);
        assert!(!cache.geometries[0].is_tiled);
        assert_eq!(cache.geometries[0].blocks_across, 1);
        assert_eq!(cache.geometries[0].blocks_down, 2);
        assert_eq!(cache.geometries[0].block_width, 40);
        assert_eq!(cache.geometries[0].block_height, 16);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn block_cache_matches_direct_read_on_explicit_overview_level()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = fixture_path("block-cache-overview");
        write_large_overview_fixture(&path)?;
        let source = GeoTiffRasterSource::open(&path)?;
        let request = WindowRequest {
            x: 15,
            y: 15,
            width: 17,
            height: 17,
            overview: 1,
        };
        let cached = source.read_samples(1, request)?;
        let direct_reader = open_direct_reader(&path)?;
        let direct = read_direct_window(&direct_reader, 1, request)?;
        assert_eq!(cached, direct);

        let cache = source
            .geotiff_block_cache
            .as_ref()
            .ok_or("GeoTIFF cache missing")?
            .lock()
            .map_err(|_| "GeoTIFF cache lock poisoned")?;
        assert_eq!(cache.geometries.len(), 3);
        assert_eq!(cache.geometries[1].width, 32);
        assert_eq!(cache.geometries[1].height, 32);
        assert_eq!(cache.geometries[1].block_width, 16);
        assert_eq!(cache.geometries[1].block_height, 16);
        assert_eq!(cache.geometries[1].blocks_across, 2);
        assert_eq!(cache.geometries[1].blocks_down, 2);
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
