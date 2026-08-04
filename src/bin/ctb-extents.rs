use std::{error::Error, path::PathBuf};

use clap::Parser;
use ctb_rs::{
    extents::write_extents,
    geotiff::GeoTiffRasterSource,
    grid::{GlobalGeodeticGrid, GlobalMercatorGrid, TileGrid},
    raster::RasterSource,
    tileset::TilesetPlan,
};

#[derive(Debug, Parser)]
#[command(
    about = "Write GeoJSON coverage extents for CTB terrain tiles",
    version = "0.4.1"
)]
struct Arguments {
    /// Directory in which {zoom}.geojson files are written.
    #[arg(short, long, default_value = ".")]
    output_dir: PathBuf,

    /// TMS profile. Direct-source extents support geodetic and mercator grids.
    #[arg(short, long, default_value = "geodetic")]
    profile: String,

    /// TMS tile edge length in pixels; defaults to 65 for geodetic and 256 for mercator.
    #[arg(short = 't', long)]
    tile_size: Option<u32>,

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
    if std::env::args().any(|argument| argument == "--version" || argument == "-V") {
        println!("0.4.1");
        return Ok(());
    }
    let arguments = Arguments::parse();
    let source = GeoTiffRasterSource::open(&arguments.input)?;
    let grid: Box<dyn TileGrid> = match arguments.profile.as_str() {
        "geodetic" => Box::new(GlobalGeodeticGrid::new(arguments.tile_size.unwrap_or(65))?),
        "mercator" => Box::new(GlobalMercatorGrid::new(arguments.tile_size.unwrap_or(256))?),
        profile => return Err(format!("unsupported TMS profile {profile}").into()),
    };
    let plan = TilesetPlan::from_raster_with_tile_grid(
        source.metadata(),
        grid.as_ref(),
        arguments.start_zoom,
        arguments.end_zoom,
    )?;
    if let Err(error) = write_extents(&plan, grid.as_ref(), arguments.output_dir) {
        eprintln!("File could not be opened: {error}");
    }
    Ok(())
}
