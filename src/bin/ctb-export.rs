use std::{error::Error, path::PathBuf};

use clap::Parser;
use ctb_rs::{export::export_heightmap_to_geotiff, grid::TileCoord, terrain::HeightmapTerrain};

#[derive(Debug, Parser)]
#[command(about = "Export a CTB terrain tile to a GeoTIFF")]
struct Arguments {
    /// Input gzip-compressed CTB terrain file.
    #[arg(short = 'i', long)]
    input_filename: PathBuf,

    /// Zoom level represented by the terrain file.
    #[arg(short = 'z', long)]
    zoom_level: u8,

    /// TMS X coordinate represented by the terrain file.
    #[arg(short = 'x', long)]
    tile_x: u32,

    /// TMS Y coordinate represented by the terrain file.
    #[arg(short = 'y', long)]
    tile_y: u32,

    /// Output GeoTIFF filename.
    #[arg(short = 'o', long)]
    output_filename: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let terrain = HeightmapTerrain::read_gzip(&arguments.input_filename)?;
    export_heightmap_to_geotiff(
        &terrain,
        TileCoord {
            zoom: arguments.zoom_level,
            x: arguments.tile_x,
            y: arguments.tile_y,
        },
        arguments.output_filename,
    )?;
    Ok(())
}
