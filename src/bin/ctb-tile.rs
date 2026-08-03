use std::{error::Error, path::PathBuf};

use clap::Parser;
use ctb_rs::{
    geotiff::GeoTiffRasterSource, grid::GlobalGeodeticGrid, tileset::write_heightmap_tileset,
};

#[derive(Debug, Parser)]
#[command(about = "Create CTB heightmap terrain tiles from an EPSG:4326 GeoTIFF DEM")]
struct Arguments {
    /// Directory in which {z}/{x}/{y}.terrain tiles are written.
    #[arg(short, long, default_value = ".")]
    output_dir: PathBuf,

    /// Do not overwrite already written final terrain files.
    #[arg(short = 'R', long)]
    resume: bool,

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let source = GeoTiffRasterSource::open(&arguments.input)?;
    let plan = write_heightmap_tileset(
        &source,
        GlobalGeodeticGrid::new(65)?,
        &arguments.output_dir,
        arguments.resume,
    )?;
    let tile_count = plan
        .levels
        .iter()
        .map(|level| level.tiles.len())
        .sum::<usize>();
    println!(
        "wrote {tile_count} CTB terrain tile(s) through zoom {}",
        plan.max_zoom
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Arguments;

    #[test]
    fn parses_output_dir_and_resume() {
        let arguments =
            Arguments::try_parse_from(["ctb-tile", "--output-dir", "tiles", "--resume", "dem.tif"]);
        assert!(arguments.is_ok());
    }
}
