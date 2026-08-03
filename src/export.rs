use std::path::Path;

use geotiff_writer::GeoTiffBuilder;
use ndarray::Array2;

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
    let samples = Array2::from_shape_vec((HEIGHTMAP_TILE_SIZE, HEIGHTMAP_TILE_SIZE), samples)
        .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    GeoTiffBuilder::new(HEIGHTMAP_TILE_SIZE as u32, HEIGHTMAP_TILE_SIZE as u32)
        .geographic_epsg(4326)
        .pixel_scale(pixel_width, pixel_height)
        .origin(bounds.min_x, bounds.max_y)
        .write_2d(output, samples.view())
        .map_err(|error| CtbError::TilesetIo(error.to_string()))
}

#[cfg(test)]
mod tests {
    use crate::{
        CtbError,
        terrain::{ChildMask, WaterMask},
    };
    use geotiff_reader::GeoTiffFile;

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
        let file = GeoTiffFile::open(&path)?;
        assert_eq!(file.epsg(), Some(4326));
        assert_eq!(file.width(), HEIGHTMAP_TILE_SIZE as u32);
        assert_eq!(file.height(), HEIGHTMAP_TILE_SIZE as u32);
        let transform = file.transform().ok_or(CtbError::InvalidBounds)?;
        assert_eq!(transform.origin_x, -180.0);
        assert_eq!(transform.origin_y, 90.0);
        assert_eq!(transform.pixel_width, 180.0 / HEIGHTMAP_TILE_SIZE as f64);
        assert_eq!(transform.pixel_height, -180.0 / HEIGHTMAP_TILE_SIZE as f64);
        let samples = file.read_band_window::<i16>(0, 0, 0, 1, 2)?;
        let actual = samples.iter().copied().collect::<Vec<_>>();
        assert_eq!(actual, vec![5000, -15536]);
        std::fs::remove_file(path)?;
        Ok(())
    }
}
