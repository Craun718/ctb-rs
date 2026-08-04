use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::{
    CtbError,
    grid::{GlobalGeodeticGrid, TileCoord},
    raster::RasterSource,
    raster_geotiff::{RasterGeoTiffCompression, write_raster_tile_as_geotiff_with_compression},
    raster_sampling::RasterTileSamplePlan,
    sampling::ResamplingMethod,
    tileset::{TileWriteProgress, TilesetPlan},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterTilesetOptions {
    pub resume: bool,
    pub start_zoom: Option<u8>,
    pub end_zoom: Option<u8>,
    pub resampling: ResamplingMethod,
    pub compression: RasterGeoTiffCompression,
    pub worker_count: usize,
}

impl Default for RasterTilesetOptions {
    fn default() -> Self {
        Self {
            resume: false,
            start_zoom: None,
            end_zoom: None,
            resampling: ResamplingMethod::Average,
            compression: RasterGeoTiffCompression::None,
            worker_count: 1,
        }
    }
}

/// Write RasterTiler-compatible GeoTIFF tiles through independently opened
/// sources, matching CTB's worker ownership model.
pub fn write_raster_geotiff_tileset_with_factory(
    source_factory: &(dyn Fn() -> Result<Box<dyn RasterSource>, CtbError> + Sync),
    grid: GlobalGeodeticGrid,
    output_directory: impl AsRef<Path>,
    options: RasterTilesetOptions,
    progress: Option<&(dyn Fn(TileWriteProgress) + Sync)>,
) -> Result<TilesetPlan, CtbError> {
    let metadata_source = source_factory()?;
    let plan = TilesetPlan::from_raster_with_zoom_range(
        metadata_source.metadata(),
        grid,
        options.start_zoom,
        options.end_zoom,
    )?;
    drop(metadata_source);
    let tiles = plan
        .levels
        .iter()
        .rev()
        .flat_map(|level| level.tiles.iter().copied())
        .collect::<Vec<_>>();
    let output_directory = output_directory.as_ref();
    let next_index = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let first_error = Mutex::new(None::<CtbError>);
    let total = tiles.len();
    let worker_count = options.worker_count.max(1).min(total.max(1));

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                let source = match source_factory() {
                    Ok(source) => source,
                    Err(error) => {
                        set_first_error(&first_error, error);
                        return;
                    }
                };
                loop {
                    if first_error.lock().is_ok_and(|error| error.is_some()) {
                        return;
                    }
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(tile) = tiles.get(index).copied() else {
                        return;
                    };
                    let path = raster_geotiff_path(output_directory, tile);
                    let outcome = if options.resume && path.exists() {
                        Ok(())
                    } else {
                        RasterTileSamplePlan::new(grid, tile).and_then(|sample_plan| {
                            sample_plan
                                .sample_values(source.as_ref(), options.resampling)
                                .and_then(|values| {
                                    write_raster_geotiff_atomically(
                                        &sample_plan,
                                        source.metadata(),
                                        values,
                                        options.compression,
                                        &path,
                                    )
                                })
                        })
                    };
                    if let Err(error) = outcome {
                        set_first_error(&first_error, error);
                        return;
                    }
                    let finished = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Some(progress) = progress {
                        progress(TileWriteProgress {
                            tile,
                            completed: finished,
                            total,
                        });
                    }
                }
            });
        }
    });
    if let Ok(mut slot) = first_error.lock()
        && let Some(error) = slot.take()
    {
        return Err(error);
    }
    Ok(plan)
}

pub fn raster_geotiff_path(output_directory: impl AsRef<Path>, tile: TileCoord) -> PathBuf {
    output_directory
        .as_ref()
        .join(tile.zoom.to_string())
        .join(tile.x.to_string())
        .join(format!("{}.tif", tile.y))
}

fn write_raster_geotiff_atomically(
    plan: &RasterTileSamplePlan,
    metadata: &crate::raster::RasterMetadata,
    values: Vec<f64>,
    compression: RasterGeoTiffCompression,
    path: &Path,
) -> Result<(), CtbError> {
    let parent = path.parent().ok_or_else(|| {
        CtbError::TilesetIo(format!(
            "GeoTIFF path {} has no parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    let filename = path.file_name().ok_or_else(|| {
        CtbError::TilesetIo(format!("GeoTIFF path {} has no filename", path.display()))
    })?;
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        filename.to_string_lossy(),
        std::process::id()
    ));
    write_raster_tile_as_geotiff_with_compression(&temporary, plan, metadata, values, compression)?;
    fs::rename(&temporary, path).map_err(|error| CtbError::TilesetIo(error.to_string()))
}

fn set_first_error(slot: &Mutex<Option<CtbError>>, error: CtbError) {
    if let Ok(mut slot) = slot.lock()
        && slot.is_none()
    {
        *slot = Some(error);
    }
}
