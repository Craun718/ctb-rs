use std::{error::Error, path::PathBuf};

use clap::Parser;
use ctb_rs::{
    extents::write_extents, geotiff::GeoTiffRasterSource, grid::GlobalGeodeticGrid,
    raster::RasterSource, tileset::TilesetPlan,
};

#[derive(Debug, Parser)]
#[command(about = "Write GeoJSON coverage extents for CTB terrain tiles")]
struct Arguments {
    /// Directory in which {zoom}.geojson files are written.
    #[arg(short, long, default_value = ".")]
    output_dir: PathBuf,

    /// TMS profile. The pure-Rust implementation currently supports geodetic only.
    #[arg(short, long, default_value = "geodetic")]
    profile: String,

    /// TMS tile edge length in pixels; defaults to the original terrain size of 65.
    #[arg(short = 't', long, default_value_t = 65)]
    tile_size: u32,

    /// Highest zoom level to include; defaults to the source-derived maximum.
    #[arg(short = 's', long)]
    start_zoom: Option<u8>,

    /// Lowest zoom level to include; defaults to zero.
    #[arg(short = 'e', long)]
    end_zoom: Option<u8>,

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    if arguments.profile != "geodetic" {
        return Err("only the geodetic profile is currently supported".into());
    }
    let source = GeoTiffRasterSource::open(&arguments.input)?;
    let grid = GlobalGeodeticGrid::new(arguments.tile_size)?;
    let plan = TilesetPlan::from_raster_with_zoom_range(
        source.metadata(),
        grid,
        arguments.start_zoom,
        arguments.end_zoom,
    )?;
    if let Err(error) = write_extents(&plan, grid, arguments.output_dir) {
        eprintln!("File could not be opened: {error}");
    }
    Ok(())
}
