use std::{fs, path::Path};

use crate::{
    CtbError,
    grid::TileGrid,
    tileset::{TilesetLevel, TilesetPlan},
};

pub fn geojson_for_level(grid: &dyn TileGrid, level: &TilesetLevel) -> Result<String, CtbError> {
    let mut features = Vec::with_capacity(level.tiles.len());
    for tile in &level.tiles {
        let bounds = grid.tile_bounds(*tile)?;
        features.push(format!(
            r#"{{ "type": "Feature", "geometry": {{ "type": "Polygon", "coordinates": [[[{min_x}, {min_y}], [{max_x}, {min_y}], [{max_x}, {max_y}], [{min_x}, {max_y}], [{min_x}, {min_y}]]]}}, "properties": {{"tx": {x}, "ty": {y}}}}}"#,
            min_x = format_scientific_15(bounds.min_x),
            min_y = format_scientific_15(bounds.min_y),
            max_x = format_scientific_15(bounds.max_x),
            max_y = format_scientific_15(bounds.max_y),
            x = tile.x,
            y = tile.y,
        ));
    }
    Ok(format!(
        "{{ \"type\": \"FeatureCollection\", \"features\": [\n{}]}}\n",
        features.join(",\n")
    ))
}

fn format_scientific_15(value: f64) -> String {
    let rendered = format!("{value:.15e}");
    let (mantissa, exponent) = rendered
        .split_once('e')
        .expect("scientific formatter always includes an exponent");
    let exponent = exponent
        .parse::<i32>()
        .expect("scientific formatter exponent is a valid integer");
    format!("{mantissa}e{exponent:+03}")
}

pub fn write_extents(
    plan: &TilesetPlan,
    grid: &dyn TileGrid,
    output_directory: impl AsRef<Path>,
) -> Result<(), CtbError> {
    let output_directory = output_directory.as_ref();
    // C++ ctb-extents.cpp writeBounds iterates from startZoom down to endZoom
    // (high to low); plan.levels is ascending, so iterate in reverse.
    for level in plan.levels.iter().rev() {
        let path = output_directory.join(format!("{}.geojson", level.zoom));
        println!("creating {}", path.display());
        fs::write(path, geojson_for_level(grid, level)?)
            .map_err(|error| CtbError::TilesetIo(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        CtbError,
        grid::{GlobalGeodeticGrid, TileCoord},
        tileset::TilesetLevel,
    };

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
        let grid = GlobalGeodeticGrid::new(65)?;
        let geojson = geojson_for_level(&grid, &level)?;
        assert!(geojson.contains(r#""tx": 0, "ty": 0"#));
        assert!(geojson.contains("[-1.800000000000000e+02, -9.000000000000000e+01]"));
        // max_x is FMA-contracted: -4.44e-15 instead of exact 0.0 (matches C++).
        assert!(geojson.contains("[-4.440892098500626e-15, 9.000000000000000e+01]"));
        Ok(())
    }

    #[test]
    fn formats_exponents_like_the_original_cpp_stream() {
        assert_eq!(format_scientific_15(-90.0), "-9.000000000000000e+01");
        assert_eq!(
            format_scientific_15(-4.440892098500626e-15),
            "-4.440892098500626e-15"
        );
    }
}
