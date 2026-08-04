use std::{error::Error, path::PathBuf};

use clap::{ArgAction, Parser, ValueEnum};
use ctb_rs::{
    cache::CachedRasterSource,
    geotiff::GeoTiffRasterSource,
    grid::GlobalGeodeticGrid,
    sampling::ResamplingMethod::Average,
    tileset::{
        HeightmapTilesetOptions, TileWriteProgress, terrain_path,
        write_heightmap_tileset_with_factory,
    },
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ResamplingArgument {
    Nearest,
    Bilinear,
    Average,
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

    /// Number of tile workers. Non-positive values use the CPU count.
    #[arg(short = 'c', long, allow_hyphen_values = true)]
    thread_count: Option<i32>,

    /// Suppress normal progress output.
    #[arg(short = 'q', long, action = ArgAction::Count)]
    quiet: u8,

    /// Print each completed tile, matching CTB's verbose progress mode.
    #[arg(short = 'v', long, action = ArgAction::Count)]
    verbose: u8,

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

    /// Accepted for CTB CLI compatibility. Heightmap Terrain always uses average.
    #[arg(short = 'r', long, value_enum, default_value_t = ResamplingArgument::Average)]
    _resampling_method: ResamplingArgument,

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
    if !arguments.output_dir.exists() {
        return Err(format!(
            "The output directory does not exist: {}",
            arguments.output_dir.display()
        )
        .into());
    }
    if !arguments.output_dir.is_dir() {
        return Err(format!(
            "The output filepath is not a directory: {}",
            arguments.output_dir.display()
        )
        .into());
    }
    let input = arguments.input.clone();
    let source_factory = move || {
        GeoTiffRasterSource::open(&input).map(|source| {
            Box::new(CachedRasterSource::new(source, 64, 64))
                as Box<dyn ctb_rs::raster::RasterSource>
        })
    };
    let verbosity = 1_i16 + i16::from(arguments.verbose) - i16::from(arguments.quiet);
    let progress: Option<Box<dyn Fn(TileWriteProgress) + Sync>> = if verbosity > 1 {
        let output_directory = arguments.output_dir.clone();
        Some(Box::new(move |event| {
            let percentage = event.completed.saturating_mul(100) / event.total.max(1);
            let path = terrain_path(&output_directory, event.tile);
            println!(
                "[{percentage}%] created {} in thread {:?}",
                path.display(),
                std::thread::current().id()
            );
        }))
    } else {
        None
    };
    write_heightmap_tileset_with_factory(
        &source_factory,
        GlobalGeodeticGrid::new(65)?,
        &arguments.output_dir,
        HeightmapTilesetOptions {
            resume: arguments.resume,
            start_zoom: arguments.start_zoom,
            end_zoom: arguments.end_zoom,
            // Original CTB parses this option but does not pass it to TerrainTiler.
            resampling: Average,
            worker_count: worker_count(arguments.thread_count),
        },
        progress.as_deref(),
    )?;
    Ok(())
}

fn worker_count(requested: Option<i32>) -> usize {
    match requested {
        Some(count) if count > 0 => count as usize,
        _ => std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Arguments, worker_count};

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

    #[test]
    fn parses_ctb_worker_and_verbosity_options() {
        let arguments =
            Arguments::try_parse_from(["ctb-tile", "-c", "2", "-q", "-v", "-v", "dem.tif"]);
        assert!(arguments.is_ok());
    }

    #[test]
    fn non_positive_worker_counts_use_available_parallelism() {
        assert!(worker_count(Some(0)) >= 1);
        assert!(worker_count(Some(-1)) >= 1);
    }
}
