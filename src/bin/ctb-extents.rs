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

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let source = GeoTiffRasterSource::open(&arguments.input)?;
    let grid = GlobalGeodeticGrid::new(65)?;
    let plan = TilesetPlan::from_raster(source.metadata(), grid)?;
    write_extents(&plan, grid, arguments.output_dir)?;
    Ok(())
}
