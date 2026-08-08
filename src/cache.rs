use std::{collections::VecDeque, sync::Mutex};

use crate::{
    CtbError,
    raster::{RasterMetadata, RasterSource, RasterWindow, SamplingLevel, WindowRequest},
};

#[derive(Debug, Clone)]
struct CachedBlock {
    request: WindowRequest,
    samples: Vec<f64>,
}

#[derive(Debug, Default)]
struct CacheState {
    blocks: VecDeque<CachedBlock>,
}

/// A bounded, thread-safe read-through block cache for a raster source.
///
/// Sources declaring NoData keep exact source windows by default, because the
/// current RasterSource contract reports NoData at window granularity rather
/// than providing a per-pixel validity mask. CTB's OxiGeo direct source keeps
/// NoData sentinels as ordinary f64 samples, so it can opt into block caching
/// through [`Self::new_with_nodata_cache`].
pub struct CachedRasterSource<S> {
    source: S,
    capacity: usize,
    block_size: u32,
    cache_nodata: bool,
    state: Mutex<CacheState>,
}

impl<S> CachedRasterSource<S> {
    pub fn new(source: S, capacity: usize, block_size: u32) -> Self {
        Self {
            source,
            capacity,
            block_size: block_size.max(1),
            cache_nodata: false,
            state: Mutex::new(CacheState::default()),
        }
    }

    pub fn new_with_nodata_cache(source: S, capacity: usize, block_size: u32) -> Self {
        Self {
            source,
            capacity,
            block_size: block_size.max(1),
            cache_nodata: true,
            state: Mutex::new(CacheState::default()),
        }
    }

    pub fn into_inner(self) -> S {
        self.source
    }
}

impl<S: RasterSource> CachedRasterSource<S> {
    fn block_request(
        &self,
        level: &SamplingLevel,
        request: WindowRequest,
    ) -> Result<WindowRequest, CtbError> {
        let x = request.x / self.block_size * self.block_size;
        let y = request.y / self.block_size * self.block_size;
        let width = self.block_size.min(level.data_width.saturating_sub(x));
        let height = self.block_size.min(level.data_height.saturating_sub(y));
        if width == 0 || height == 0 {
            return Err(CtbError::InvalidRasterWindow);
        }
        Ok(WindowRequest {
            x,
            y,
            width,
            height,
            overview: request.overview,
        })
    }

    fn cached_block(
        &self,
        level: &SamplingLevel,
        request: WindowRequest,
    ) -> Result<CachedBlock, CtbError> {
        let block_request = self.block_request(level, request)?;
        if let Ok(mut state) = self.state.lock()
            && let Some(position) = state
                .blocks
                .iter()
                .position(|block| block.request == block_request)
            && let Some(block) = state.blocks.remove(position)
        {
            state.blocks.push_front(block.clone());
            return Ok(block);
        }

        let window = self.source.read_sampling_window(level, block_request)?;
        let block = CachedBlock {
            request: block_request,
            samples: window.samples,
        };
        if let Ok(mut state) = self.state.lock() {
            state.blocks.push_front(block.clone());
            while state.blocks.len() > self.capacity {
                state.blocks.pop_back();
            }
        }
        Ok(block)
    }
}

impl<S: RasterSource> RasterSource for CachedRasterSource<S> {
    fn metadata(&self) -> &RasterMetadata {
        self.source.metadata()
    }

    fn overview_count(&self) -> u16 {
        self.source.overview_count()
    }

    fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
        let level = SamplingLevel {
            level: 0,
            data_width: self.metadata().width,
            data_height: self.metadata().height,
            metadata: self.metadata().clone(),
        };
        self.read_sampling_window(&level, request)
    }

    fn sampling_level_for_ratio(&self, target_ratio: f64) -> Result<SamplingLevel, CtbError> {
        self.source.sampling_level_for_ratio(target_ratio)
    }

    fn read_sampling_window(
        &self,
        level: &SamplingLevel,
        request: WindowRequest,
    ) -> Result<RasterWindow, CtbError> {
        if self.capacity == 0 || (level.metadata.no_data.is_some() && !self.cache_nodata) {
            return self.source.read_sampling_window(level, request);
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
            || end_x > level.data_width
            || end_y > level.data_height
        {
            return Err(CtbError::InvalidRasterWindow);
        }
        let mut samples = Vec::with_capacity(request.width as usize * request.height as usize);
        for y in request.y..end_y {
            for x in request.x..end_x {
                let block = self.cached_block(
                    level,
                    WindowRequest {
                        x,
                        y,
                        width: 1,
                        height: 1,
                        overview: request.overview,
                    },
                )?;
                let local_x = (x - block.request.x) as usize;
                let local_y = (y - block.request.y) as usize;
                let index = local_y * block.request.width as usize + local_x;
                let sample = block.samples.get(index).copied().ok_or_else(|| {
                    CtbError::RasterRead(
                        "cached raster block did not contain its requested sample".to_owned(),
                    )
                })?;
                samples.push(sample);
            }
        }
        Ok(RasterWindow { request, samples })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::raster::{AffineTransform, Crs, RasterSampleType};

    use super::*;

    struct CountingRaster {
        metadata: RasterMetadata,
        reads: Arc<AtomicUsize>,
    }

    impl CountingRaster {
        fn new(reads: Arc<AtomicUsize>, no_data: Option<f64>) -> Result<Self, CtbError> {
            Ok(Self {
                metadata: RasterMetadata {
                    width: 8,
                    height: 8,
                    band_count: 1,
                    crs: Crs::Epsg4326,
                    transform: AffineTransform::north_up(0.0, 8.0, 1.0, -1.0)?,
                    no_data,
                    sample_type: RasterSampleType::Float64,
                },
                reads,
            })
        }
    }

    impl RasterSource for CountingRaster {
        fn metadata(&self) -> &RasterMetadata {
            &self.metadata
        }

        fn overview_count(&self) -> u16 {
            0
        }

        fn read_window(&self, request: WindowRequest) -> Result<RasterWindow, CtbError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let end_x = request
                .x
                .checked_add(request.width)
                .ok_or(CtbError::InvalidRasterWindow)?;
            let end_y = request
                .y
                .checked_add(request.height)
                .ok_or(CtbError::InvalidRasterWindow)?;
            if end_x > self.metadata.width || end_y > self.metadata.height {
                return Err(CtbError::InvalidRasterWindow);
            }
            let mut samples = Vec::new();
            for y in request.y..end_y {
                for x in request.x..end_x {
                    samples.push(f64::from(y * self.metadata.width + x));
                }
            }
            Ok(RasterWindow { request, samples })
        }
    }

    #[test]
    fn reuses_a_block_for_neighbouring_pixel_reads() -> Result<(), CtbError> {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CachedRasterSource::new(CountingRaster::new(Arc::clone(&reads), None)?, 2, 4);
        let first = source.read_window(WindowRequest {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
            overview: 0,
        })?;
        let second = source.read_window(WindowRequest {
            x: 2,
            y: 2,
            width: 1,
            height: 1,
            overview: 0,
        })?;
        assert_eq!(first.samples, vec![9.0]);
        assert_eq!(second.samples, vec![18.0]);
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn bounds_cache_capacity_with_lru_eviction() -> Result<(), CtbError> {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CachedRasterSource::new(CountingRaster::new(Arc::clone(&reads), None)?, 1, 4);
        for (x, y) in [(0, 0), (4, 0), (0, 0)] {
            source.read_window(WindowRequest {
                x,
                y,
                width: 1,
                height: 1,
                overview: 0,
            })?;
        }
        assert_eq!(reads.load(Ordering::Relaxed), 3);
        Ok(())
    }

    #[test]
    fn keeps_exact_reads_for_sources_declaring_nodata() -> Result<(), CtbError> {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CachedRasterSource::new(
            CountingRaster::new(Arc::clone(&reads), Some(-9999.0))?,
            2,
            4,
        );
        for x in [1, 2] {
            source.read_window(WindowRequest {
                x,
                y: 1,
                width: 1,
                height: 1,
                overview: 0,
            })?;
        }
        assert_eq!(reads.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn caches_nodata_blocks_when_direct_source_opt_in() -> Result<(), CtbError> {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CachedRasterSource::new_with_nodata_cache(
            CountingRaster::new(Arc::clone(&reads), Some(-9999.0))?,
            2,
            4,
        );
        for (x, y) in [(1, 1), (2, 2)] {
            source.read_window(WindowRequest {
                x,
                y,
                width: 1,
                height: 1,
                overview: 0,
            })?;
        }
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn uses_data_dimensions_for_overview_block_requests() -> Result<(), CtbError> {
        let reads = Arc::new(AtomicUsize::new(0));
        let source = CachedRasterSource::new_with_nodata_cache(
            CountingRaster::new(Arc::clone(&reads), Some(-9999.0))?,
            2,
            4,
        );
        let mut metadata = source.metadata().clone();
        metadata.width = 4;
        metadata.height = 4;
        let level = SamplingLevel {
            level: 0,
            data_width: 8,
            data_height: 8,
            metadata,
        };
        let window = source.read_sampling_window(
            &level,
            WindowRequest {
                x: 6,
                y: 6,
                width: 1,
                height: 1,
                overview: 0,
            },
        )?;
        assert_eq!(window.samples, vec![54.0]);
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        Ok(())
    }
}
