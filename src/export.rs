use std::path::Path;

use oxigeo::{
    GeoTransform, RasterDataType,
    geotiff::{GeoTiffWriter, GeoTiffWriterOptions, OverviewResampling, WriterConfig},
};

use crate::{
    CtbError,
    grid::{GlobalGeodeticGrid, TileCoord},
    terrain::{HEIGHTMAP_TILE_SIZE, HeightmapTerrain},
};

/// Export a CTB terrain tile using the same signed 16-bit bit interpretation
/// as CTB's `TerrainTile::heightsToRaster` implementation.
pub fn export_heightmap_to_geotiff(
    terrain: &HeightmapTerrain,
    tile: TileCoord,
    output: impl AsRef<Path>,
) -> Result<(), CtbError> {
    let grid = GlobalGeodeticGrid::new(HEIGHTMAP_TILE_SIZE as u32)?;
    let bounds = grid.tile_bounds(tile)?;
    let pixel_width = bounds.width() / HEIGHTMAP_TILE_SIZE as f64;
    let pixel_height = bounds.height() / HEIGHTMAP_TILE_SIZE as f64;
    let samples = terrain
        .heights
        .iter()
        .map(|height| i16::from_ne_bytes(height.to_ne_bytes()))
        .collect::<Vec<_>>();
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let mut config = WriterConfig::new(
        HEIGHTMAP_TILE_SIZE as u64,
        HEIGHTMAP_TILE_SIZE as u64,
        1,
        RasterDataType::Int16,
    )
    .with_geo_transform(GeoTransform::north_up(
        bounds.min_x,
        bounds.max_y,
        pixel_width,
        -pixel_height,
    ))
    .with_epsg_code(4326)
    .with_overviews(false, OverviewResampling::Average);
    config.tile_width = None;
    config.tile_height = None;
    let mut writer = GeoTiffWriter::create(output, config, GeoTiffWriterOptions::default())
        .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    writer
        .write(&bytes)
        .map_err(|error| CtbError::TilesetIo(error.to_string()))
}

#[cfg(test)]
mod tests {
    use oxigeo::core_types::io::FileDataSource;

    use crate::{
        CtbError,
        terrain::{ChildMask, WaterMask},
    };
    use oxigeo::geotiff::GeoTiffReader;

    use super::*;

    #[test]
    fn preserves_ctb_signed_bit_patterns_and_georeferencing()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut heights = vec![5000_u16; HEIGHTMAP_TILE_SIZE * HEIGHTMAP_TILE_SIZE];
        heights[1] = 50_000;
        let terrain = HeightmapTerrain::new(heights, ChildMask::empty(), WaterMask::AllLand)?;
        let path = std::env::temp_dir().join(format!("ctb-rs-export-{}.tif", std::process::id()));
        export_heightmap_to_geotiff(
            &terrain,
            TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            },
            &path,
        )?;
        let file = GeoTiffReader::open(FileDataSource::open(&path)?)?;
        assert_eq!(file.epsg_code(), Some(4326));
        assert_eq!(file.width(), HEIGHTMAP_TILE_SIZE as u64);
        assert_eq!(file.height(), HEIGHTMAP_TILE_SIZE as u64);
        let transform = file.geo_transform().ok_or(CtbError::InvalidBounds)?;
        assert_eq!(transform.origin_x, -180.0);
        assert_eq!(transform.origin_y, 90.0);
        assert_eq!(transform.pixel_width, 180.0 / HEIGHTMAP_TILE_SIZE as f64);
        assert_eq!(transform.pixel_height, -180.0 / HEIGHTMAP_TILE_SIZE as f64);
        let mut samples = vec![0_i16; 2];
        file.read_window_into_typed::<i16>(0, 0, 0, 0, 2, 1, &mut samples)?;
        assert_eq!(samples, vec![5000, -15536]);
        std::fs::remove_file(path)?;
        Ok(())
    }
}
