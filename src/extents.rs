use std::{fs, path::Path};

use crate::{
    CtbError,
    grid::GlobalGeodeticGrid,
    tileset::{TilesetLevel, TilesetPlan},
};

pub fn geojson_for_level(
    grid: GlobalGeodeticGrid,
    level: &TilesetLevel,
) -> Result<String, CtbError> {
    let mut features = Vec::with_capacity(level.tiles.len());
    for tile in &level.tiles {
        let bounds = grid.tile_bounds(*tile)?;
        features.push(format!(
            r#"{{"type":"Feature","geometry":{{"type":"Polygon","coordinates":[[[{min_x},{min_y}],[{max_x},{min_y}],[{max_x},{max_y}],[{min_x},{max_y}],[{min_x},{min_y}]]]}},"properties":{{"tx":{x},"ty":{y}}}}}"#,
            min_x = bounds.min_x,
            min_y = bounds.min_y,
            max_x = bounds.max_x,
            max_y = bounds.max_y,
            x = tile.x,
            y = tile.y,
        ));
    }
    Ok(format!(
        r#"{{"type":"FeatureCollection","features":[{}]}}"#,
        features.join(",")
    ))
}

pub fn write_extents(
    plan: &TilesetPlan,
    grid: GlobalGeodeticGrid,
    output_directory: impl AsRef<Path>,
) -> Result<(), CtbError> {
    let output_directory = output_directory.as_ref();
    fs::create_dir_all(output_directory).map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    for level in &plan.levels {
        let path = output_directory.join(format!("{}.geojson", level.zoom));
        fs::write(path, geojson_for_level(grid, level)?)
            .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{CtbError, grid::TileCoord, tileset::TilesetLevel};

    use super::*;

    #[test]
    fn serializes_tile_bounds_and_tms_properties() -> Result<(), CtbError> {
        let level = TilesetLevel {
            zoom: 0,
            tiles: vec![TileCoord {
                zoom: 0,
                x: 0,
                y: 0,
            }],
        };
        let geojson = geojson_for_level(GlobalGeodeticGrid::new(65)?, &level)?;
        assert!(geojson.contains(r#""tx":0,"ty":0"#));
        assert!(geojson.contains("[-180,-90]"));
        assert!(geojson.contains("[0,90]"));
        Ok(())
    }
}
