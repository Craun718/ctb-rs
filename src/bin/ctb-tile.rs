use std::{error::Error, path::PathBuf};

use clap::{ArgAction, Parser, ValueEnum};
use ctb_rs::{
    cache::CachedRasterSource,
    geotiff::GeoTiffRasterSource,
    grid::{GlobalGeodeticGrid, GlobalMercatorGrid, TileGrid},
    raster_geotiff::{RasterGeoTiffCompression, RasterGeoTiffWriteOptions},
    raster_tileset::{
        RasterTilesetOptions, raster_geotiff_path, write_raster_geotiff_tileset_with_factory,
    },
    sampling::ResamplingMethod::{self, Average},
    tileset::{
        HeightmapTilesetOptions, TileWriteProgress, terrain_path,
        write_heightmap_tileset_with_factory,
    },
};
use geotiff_writer::{Predictor, TiffVariant};

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

fn gtiff_options(options: &[String]) -> Result<RasterGeoTiffWriteOptions, Box<dyn Error>> {
    let mut result = RasterGeoTiffWriteOptions::default();
    let mut block_x = None;
    let mut block_y = None;
    for option in options {
        let (name, value) = option
            .split_once('=')
            .ok_or_else(|| format!("GTiff creation option {option} must be NAME=VALUE"))?;
        match (name, value) {
            ("COMPRESS", "NONE") => result.compression = RasterGeoTiffCompression::None,
            ("COMPRESS", "DEFLATE") => result.compression = RasterGeoTiffCompression::Deflate,
            ("COMPRESS", "LZW") => result.compression = RasterGeoTiffCompression::Lzw,
            ("COMPRESS", "ZSTD") => result.compression = RasterGeoTiffCompression::Zstd,
            ("COMPRESS", "JPEG") => result.compression = RasterGeoTiffCompression::Jpeg,
            ("COMPRESS", "LERC") => result.compression = RasterGeoTiffCompression::Lerc,
            ("BIGTIFF", "NO") => result.tiff_variant = TiffVariant::Classic,
            ("BIGTIFF", "YES") => result.tiff_variant = TiffVariant::BigTiff,
            ("BIGTIFF", "IF_NEEDED") => result.tiff_variant = TiffVariant::Auto,
            ("PREDICTOR", "1") => result.predictor = Some(Predictor::None),
            ("PREDICTOR", "2") => result.predictor = Some(Predictor::Horizontal),
            ("PREDICTOR", "3") => result.predictor = Some(Predictor::FloatingPoint),
            ("TILED", "YES") => result.tile_size = Some((256, 256)),
            ("TILED", "NO") => result.tile_size = None,
            ("BLOCKXSIZE", value) => block_x = Some(parse_block_size(option, value)?),
            ("BLOCKYSIZE", value) => block_y = Some(parse_block_size(option, value)?),
            _ => return Err(format!("GTiff creation option {option} is not implemented").into()),
        }
    }
    if block_x.is_some() || block_y.is_some() {
        let tile_width = block_x.map_or(256, |value| value);
        let tile_height = block_y.map_or(256, |value| value);
        result.tile_size = Some((tile_width, tile_height));
    }
    Ok(result)
}

fn parse_block_size(option: &str, value: &str) -> Result<u32, Box<dyn Error>> {
    let size = value
        .parse::<u32>()
        .map_err(|_| format!("GTiff creation option {option} must be a positive integer"))?;
    if size == 0 || !size.is_multiple_of(16) {
        return Err(
            format!("GTiff creation option {option} must be a positive multiple of 16").into(),
        );
    }
    Ok(size)
}

#[derive(Debug, Parser)]
#[command(
    about = "Create CTB heightmap terrain tiles from an EPSG:4326 GeoTIFF DEM",
    version = env!("CARGO_PKG_VERSION")
)]
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

    /// GDAL approximate transformer error threshold in pixels.
    #[arg(short = 'z', long = "error-threshold", default_value_t = 0.125_f32)]
    error_threshold: f32,

    /// GDAL warp memory limit in bytes; zero uses the GDAL default.
    #[arg(short = 'm', long = "warp-memory", default_value_t = 0.0_f64)]
    warp_memory_limit: f64,

    /// Input single-band, north-up EPSG:4326 GeoTIFF DEM.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().any(|argument| argument == "--version" || argument == "-V") {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
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
    validate_warp_options(arguments.error_threshold, arguments.warp_memory_limit)?;
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
            if !arguments.creation_options.is_empty() {
                return Err("creation options are not valid for Terrain output".into());
            }
            // C++ ctb-tile.cpp:499-507 uses profile-based tile_size for the grid
            // (geodetic=65, mercator=256) regardless of output format. The terrain
            // heightmap TILE_SIZE=65 is a compile-time constant (config.hpp),
            // independent of the grid tile_size.
            let terrain_grid_size = arguments
                .tile_size
                .unwrap_or(profile_default_tile_size(&arguments.profile)?);
            let grid: Box<dyn TileGrid> = match arguments.profile.as_str() {
                "geodetic" => Box::new(GlobalGeodeticGrid::new(terrain_grid_size)?),
                "mercator" => Box::new(GlobalMercatorGrid::new(terrain_grid_size)?),
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
            let geotiff_options = gtiff_options(&arguments.creation_options)?;
            let raster_tile_size = arguments
                .tile_size
                .unwrap_or(profile_default_tile_size(&arguments.profile)?);
            let grid: Box<dyn TileGrid> = match arguments.profile.as_str() {
                "geodetic" => Box::new(GlobalGeodeticGrid::new(raster_tile_size)?),
                "mercator" => Box::new(GlobalMercatorGrid::new(raster_tile_size)?),
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
                    compression: geotiff_options.compression,
                    tiff_variant: geotiff_options.tiff_variant,
                    predictor: geotiff_options.predictor,
                    block_size: geotiff_options.tile_size,
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

fn validate_warp_options(
    error_threshold: f32,
    warp_memory_limit: f64,
) -> Result<(), Box<dyn Error>> {
    if !error_threshold.is_finite() || error_threshold < 0.0 {
        return Err("--error-threshold must be a finite non-negative number".into());
    }
    if !warp_memory_limit.is_finite() || warp_memory_limit < 0.0 {
        return Err("--warp-memory must be a finite non-negative number".into());
    }
    if error_threshold != 0.125 {
        return Err(
            "--error-threshold is parsed but non-default GDAL approximation is not implemented by the pure-Rust CTB port".into(),
        );
    }
    if warp_memory_limit != 0.0 {
        return Err(
            "--warp-memory is parsed but GDAL warp memory control is not implemented by the pure-Rust CTB port".into(),
        );
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

/// Resolve the profile-based default tile size.
///
/// Matches CTB `ctb-tile.cpp` lines 503-507: geodetic defaults to 65,
/// mercator to 256, regardless of output format.
fn profile_default_tile_size(profile: &str) -> Result<u32, String> {
    match profile {
        "geodetic" => Ok(65),
        "mercator" => Ok(256),
        other => Err(format!("unsupported TMS profile {other}")),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Arguments, Predictor, RasterGeoTiffCompression, TiffVariant, gtiff_options,
        validate_warp_options, worker_count,
    };

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
    fn parses_warp_execution_options_with_cpp_defaults() {
        let arguments =
            Arguments::try_parse_from(["ctb-tile", "-z", "0.25", "-m", "1048576", "dem.tif"])
                .expect("warp execution options should be accepted by clap");
        assert_eq!(arguments.error_threshold, 0.25);
        assert_eq!(arguments.warp_memory_limit, 1_048_576.0);
    }

    #[test]
    fn rejects_invalid_or_non_default_warp_execution_options() {
        assert!(validate_warp_options(0.125, 0.0).is_ok());
        assert!(validate_warp_options(-1.0, 0.0).is_err());
        assert!(validate_warp_options(f32::NAN, 0.0).is_err());
        assert!(validate_warp_options(0.125, -1.0).is_err());
        assert!(validate_warp_options(0.25, 0.0).is_err());
        assert!(validate_warp_options(0.125, 1_048_576.0).is_err());
    }

    #[test]
    fn parses_supported_geotiff_container_options() {
        let options = gtiff_options(&[
            "COMPRESS=ZSTD".to_owned(),
            "BIGTIFF=YES".to_owned(),
            "PREDICTOR=3".to_owned(),
        ])
        .expect("supported GeoTIFF options should parse");
        assert_eq!(options.compression, RasterGeoTiffCompression::Zstd);
        assert_eq!(options.tiff_variant, TiffVariant::BigTiff);
        assert_eq!(options.predictor, Some(Predictor::FloatingPoint));
        assert_eq!(
            gtiff_options(&["BIGTIFF=NO".to_owned()])
                .expect("Classic TIFF option should parse")
                .tiff_variant,
            TiffVariant::Classic
        );
        assert_eq!(
            gtiff_options(&["BIGTIFF=IF_NEEDED".to_owned()])
                .expect("automatic TIFF option should parse")
                .tiff_variant,
            TiffVariant::Auto
        );
        assert_eq!(
            gtiff_options(&[
                "TILED=YES".to_owned(),
                "BLOCKXSIZE=32".to_owned(),
                "BLOCKYSIZE=16".to_owned(),
            ])
            .expect("tiled layout options should parse")
            .tile_size,
            Some((32, 16))
        );
    }

    #[test]
    fn rejects_malformed_geotiff_creation_options() {
        assert!(gtiff_options(&["COMPRESS".to_owned()]).is_err());
        assert!(gtiff_options(&["PREDICTOR=4".to_owned()]).is_err());
        assert!(gtiff_options(&["BLOCKXSIZE=17".to_owned()]).is_err());
        assert!(gtiff_options(&["BLOCKYSIZE=0".to_owned()]).is_err());
    }

    #[test]
    fn non_positive_worker_counts_use_available_parallelism() {
        assert!(worker_count(Some(0)) >= 1);
        assert!(worker_count(Some(-1)) >= 1);
    }
}
