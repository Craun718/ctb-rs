use std::{error::Error, path::PathBuf};

use clap::{ArgAction, Parser, ValueEnum};
use ctb_rs::{
    cache::CachedRasterSource,
    geotiff::GeoTiffRasterSource,
    grid::{GlobalGeodeticGrid, GlobalMercatorGrid, TileGrid},
    raster_geotiff::RasterGeoTiffCompression,
    raster_tileset::{
        RasterTilesetOptions, raster_geotiff_path, write_raster_geotiff_tileset_with_factory,
    },
    sampling::ResamplingMethod::{self, Average},
    tileset::{
        HeightmapTilesetOptions, TileWriteProgress, terrain_path,
        write_heightmap_tileset_with_factory,
    },
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ResamplingArgument {
    Nearest,
    Bilinear,
    Cubic,
    Cubicspline,
    Lanczos,
    Average,
    Mode,
    Max,
    Min,
    Med,
    Q1,
    Q3,
}

impl From<ResamplingArgument> for ResamplingMethod {
    fn from(value: ResamplingArgument) -> Self {
        match value {
            ResamplingArgument::Nearest => Self::Nearest,
            ResamplingArgument::Bilinear => Self::Bilinear,
            ResamplingArgument::Cubic => Self::Cubic,
            ResamplingArgument::Cubicspline => Self::CubicSpline,
            ResamplingArgument::Lanczos => Self::Lanczos,
            ResamplingArgument::Average => Self::Average,
            ResamplingArgument::Mode => Self::Mode,
            ResamplingArgument::Max => Self::Max,
            ResamplingArgument::Min => Self::Min,
            ResamplingArgument::Med => Self::Med,
            ResamplingArgument::Q1 => Self::Q1,
            ResamplingArgument::Q3 => Self::Q3,
        }
    }
}

fn gtiff_compression(options: &[String]) -> Result<RasterGeoTiffCompression, Box<dyn Error>> {
    let mut compression = RasterGeoTiffCompression::None;
    for option in options {
        compression = match option.as_str() {
            "COMPRESS=NONE" => RasterGeoTiffCompression::None,
            "COMPRESS=DEFLATE" => RasterGeoTiffCompression::Deflate,
            _ => return Err(format!("GTiff creation option {option} is not implemented").into()),
        };
    }
    Ok(compression)
}

#[derive(Debug, Parser)]
#[command(about = "Create CTB heightmap terrain tiles from an EPSG:4326 GeoTIFF DEM")]
struct Arguments {
    /// Directory in which {z}/{x}/{y}.terrain tiles are written.
    #[arg(short, long, default_value = ".")]
    output_dir: PathBuf,

    /// CTB output format. Terrain uses heightmap-1.0; GTiff uses RasterTiler.
    #[arg(short = 'f', long, default_value = "Terrain")]
    output_format: String,

    /// GDAL creation option. GTiff options are introduced only after an oracle.
    #[arg(short = 'n', long = "creation-option")]
    creation_options: Vec<String>,

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

    /// TMS profile. GTiff direct-source supports geodetic and mercator.
    #[arg(short, long, default_value = "geodetic")]
    profile: String,

    /// Tile edge length. Terrain requires 65; RasterTiler accepts CTB grid sizes.
    #[arg(short, long)]
    tile_size: Option<u32>,

    /// Highest zoom level to generate; defaults to the source-derived maximum.
    #[arg(short = 's', long)]
    start_zoom: Option<u8>,

    /// Lowest zoom level to generate; defaults to zero.
    #[arg(short = 'e', long)]
    end_zoom: Option<u8>,

    /// Raster resampling algorithm, matching CTB's `-r` values.
    #[arg(short = 'r', long, value_enum, default_value_t = ResamplingArgument::Average)]
    resampling_method: ResamplingArgument,

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
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
    let terrain_output = arguments.output_format == "Terrain";
    let progress: Option<Box<dyn Fn(TileWriteProgress) + Sync>> = if verbosity > 1 {
        let output_directory = arguments.output_dir.clone();
        Some(Box::new(move |event| {
            let percentage = event.completed.saturating_mul(100) / event.total.max(1);
            let path = if terrain_output {
                terrain_path(&output_directory, event.tile)
            } else {
                raster_geotiff_path(&output_directory, event.tile)
            };
            println!(
                "[{percentage}%] created {} in thread {:?}",
                path.display(),
                std::thread::current().id()
            );
        }))
    } else {
        None
    };
    match arguments.output_format.as_str() {
        "Terrain" => {
            if let Some(tile_size) = arguments.tile_size
                && tile_size != 65
            {
                return Err("CTB heightmap-1.0 output requires --tile-size 65".into());
            }
            let grid: Box<dyn TileGrid> = match arguments.profile.as_str() {
                "geodetic" => Box::new(GlobalGeodeticGrid::new(65)?),
                "mercator" => Box::new(GlobalMercatorGrid::new(65)?),
                profile => return Err(format!("unsupported TMS profile {profile}").into()),
            };
            write_heightmap_tileset_with_factory(
                &source_factory,
                grid.as_ref(),
                &arguments.output_dir,
                HeightmapTilesetOptions {
                    resume: arguments.resume,
                    start_zoom: arguments.start_zoom,
                    end_zoom: arguments.end_zoom,
                    // CTB parses `-r`, but its Terrain branch constructs TerrainTiler
                    // without TilerOptions, leaving the GDAL default GRA_Average.
                    resampling: Average,
                    worker_count: worker_count(arguments.thread_count),
                },
                progress.as_deref(),
            )?;
        }
        "GTiff" => {
            let grid: Box<dyn TileGrid> = match arguments.profile.as_str() {
                "geodetic" => Box::new(GlobalGeodeticGrid::new(arguments.tile_size.unwrap_or(65))?),
                "mercator" => {
                    Box::new(GlobalMercatorGrid::new(arguments.tile_size.unwrap_or(256))?)
                }
                profile => return Err(format!("unsupported TMS profile {profile}").into()),
            };
            write_raster_geotiff_tileset_with_factory(
                &source_factory,
                grid.as_ref(),
                &arguments.output_dir,
                RasterTilesetOptions {
                    resume: arguments.resume,
                    start_zoom: arguments.start_zoom,
                    end_zoom: arguments.end_zoom,
                    resampling: arguments.resampling_method.into(),
                    compression: gtiff_compression(&arguments.creation_options)?,
                    worker_count: worker_count(arguments.thread_count),
                },
                progress.as_deref(),
            )?;
        }
        other => {
            return Err(format!(
                "output format {other} is not implemented by the pure-Rust CTB port"
            )
            .into());
        }
    }
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
    fn parses_every_ctb_resampling_name() {
        for name in [
            "nearest",
            "bilinear",
            "cubic",
            "cubicspline",
            "lanczos",
            "average",
            "mode",
            "max",
            "min",
            "med",
            "q1",
            "q3",
        ] {
            let arguments = Arguments::try_parse_from(["ctb-tile", "-r", name, "dem.tif"]);
            assert!(arguments.is_ok(), "CTB resampling name must parse: {name}");
        }
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
