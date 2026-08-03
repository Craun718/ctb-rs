use std::{error::Error, path::PathBuf};

use clap::{Parser, ValueEnum};
use ctb_rs::{
    geotiff::GeoTiffRasterSource,
    grid::GlobalGeodeticGrid,
    sampling::ResamplingMethod,
    tileset::{HeightmapTilesetOptions, write_heightmap_tileset_with_options},
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ResamplingArgument {
    Nearest,
    Bilinear,
    Average,
}

impl From<ResamplingArgument> for ResamplingMethod {
    fn from(value: ResamplingArgument) -> Self {
        match value {
            ResamplingArgument::Nearest => Self::Nearest,
            ResamplingArgument::Bilinear => Self::Bilinear,
            ResamplingArgument::Average => Self::Average,
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Create CTB heightmap terrain tiles from an EPSG:4326 GeoTIFF DEM")]
struct Arguments {
    /// Directory in which {z}/{x}/{y}.terrain tiles are written.
    #[arg(short, long, default_value = ".")]
    output_dir: PathBuf,

    /// Do not overwrite already written final terrain files.
    #[arg(short = 'R', long)]
    resume: bool,

    /// TMS profile. The pure-Rust heightmap MVP supports geodetic only.
    #[arg(short, long, default_value = "geodetic")]
    profile: String,

    /// Terrain heightmap edge length. CTB heightmap-1.0 requires 65.
    #[arg(short, long)]
    tile_size: Option<u32>,

    /// Highest zoom level to generate; defaults to the source-derived maximum.
    #[arg(short = 's', long)]
    start_zoom: Option<u8>,

    /// Lowest zoom level to generate; defaults to zero.
    #[arg(short = 'e', long)]
    end_zoom: Option<u8>,

    /// Resampling method: nearest, bilinear, or average (the default).
    #[arg(short = 'r', long, value_enum, default_value_t = ResamplingArgument::Average)]
    resampling_method: ResamplingArgument,

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    if arguments.profile != "geodetic" {
        return Err("only the geodetic profile is supported by the pure-Rust heightmap MVP".into());
    }
    if let Some(tile_size) = arguments.tile_size
        && tile_size != 65
    {
        return Err("CTB heightmap-1.0 output requires --tile-size 65".into());
    }
    let source = GeoTiffRasterSource::open(&arguments.input)?;
    let plan = write_heightmap_tileset_with_options(
        &source,
        GlobalGeodeticGrid::new(65)?,
        &arguments.output_dir,
        HeightmapTilesetOptions {
            resume: arguments.resume,
            start_zoom: arguments.start_zoom,
            end_zoom: arguments.end_zoom,
            resampling: arguments.resampling_method.into(),
        },
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

    #[test]
    fn parses_ctb_zoom_and_resampling_options() {
        let arguments = Arguments::try_parse_from([
            "ctb-tile", "-s", "3", "-e", "1", "-r", "bilinear", "-t", "65", "dem.tif",
        ]);
        assert!(arguments.is_ok());
    }
}
