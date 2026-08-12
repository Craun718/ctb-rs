use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use ctb_rs::terrain::{ChildMask, HEIGHTMAP_SAMPLE_COUNT, HeightmapTerrain, WaterMask};
use oxigeo::{
    GeoTransform, RasterDataType,
    core_types::io::FileDataSource,
    geotiff::{
        GeoTiffReader, GeoTiffWriter, GeoTiffWriterOptions, OverviewResampling, WriterConfig,
        tiff::{Compression, Predictor},
    },
    vrt::{SourceWindow, VrtBand, VrtBuilder, VrtSource},
};

fn temporary_directory(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("ctb-rs-cli-{label}-{nanos}"));
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn write_geotiff_bytes(
    path: &Path,
    width: u64,
    height: u64,
    data_type: RasterDataType,
    bytes: &[u8],
    epsg: u32,
    transform: GeoTransform,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = WriterConfig::new(width, height, 1, data_type)
        .with_compression(Compression::None)
        .with_predictor(Predictor::None)
        .with_overviews(false, OverviewResampling::Average)
        .with_geo_transform(transform)
        .with_epsg_code(epsg);
    config.tile_width = None;
    config.tile_height = None;
    let mut writer = GeoTiffWriter::create(path, config, GeoTiffWriterOptions::default())?;
    writer.write(bytes)?;
    Ok(())
}

fn write_float64_geotiff(
    path: &Path,
    width: u64,
    height: u64,
    samples: &[f64],
    epsg: u32,
    transform: GeoTransform,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    write_geotiff_bytes(
        path,
        width,
        height,
        RasterDataType::Float64,
        &bytes,
        epsg,
        transform,
    )
}

fn write_world_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let samples = vec![100.0_f64; 65 * 65];
    write_float64_geotiff(
        path,
        65,
        65,
        &samples,
        4326,
        GeoTransform::north_up(-180.0, 90.0, 360.0 / 65.0, -180.0 / 65.0),
    )
}

fn write_small_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_float64_geotiff(
        path,
        2,
        2,
        &[100.0_f64, 200.0, 300.0, 400.0],
        4326,
        GeoTransform::north_up(-1.0, 1.0, 1.0, -1.0),
    )
}

fn write_u8_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = [10_u8, 20, 30, 40]
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    write_geotiff_bytes(
        path,
        2,
        2,
        RasterDataType::UInt8,
        &bytes,
        4326,
        GeoTransform::north_up(-1.0, 1.0, 1.0, -1.0),
    )
}

fn write_mercator_world_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let origin_shift = std::f64::consts::PI * 6_378_137.0;
    let samples = vec![7.0_f64; 16];
    write_float64_geotiff(
        path,
        4,
        4,
        &samples,
        3857,
        GeoTransform::north_up(
            -origin_shift,
            origin_shift,
            origin_shift / 2.0,
            -origin_shift / 2.0,
        ),
    )
}

fn write_utm_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let samples = vec![9.0_f64; 32 * 32];
    write_float64_geotiff(
        path,
        32,
        32,
        &samples,
        32630,
        GeoTransform::north_up(400_000.0, 100_000.0, 8_000.0, -8_000.0),
    )
}

fn open_geotiff(path: &Path) -> Result<GeoTiffReader<FileDataSource>, Box<dyn std::error::Error>> {
    Ok(GeoTiffReader::open(FileDataSource::open(path)?)?)
}

fn read_f64_window(
    file: &GeoTiffReader<FileDataSource>,
    width: u64,
    height: u64,
) -> Result<Vec<f64>, Box<dyn std::error::Error>> {
    let count = usize::try_from(width * height)?;
    let mut samples = vec![0.0_f64; count];
    file.read_window_into_typed::<f64>(0, 0, 0, 0, width, height, &mut samples)?;
    Ok(samples)
}

#[test]
fn ctb_cli_versions_match_cargo_package_version() -> Result<(), Box<dyn std::error::Error>> {
    let binaries = [
        ("ctb-tile", env!("CARGO_BIN_EXE_ctb-tile")),
        ("ctb-info", env!("CARGO_BIN_EXE_ctb-info")),
        ("ctb-export", env!("CARGO_BIN_EXE_ctb-export")),
        ("ctb-extents", env!("CARGO_BIN_EXE_ctb-extents")),
    ];

    for (name, binary) in binaries {
        for flag in ["--version", "-V"] {
            let output = Command::new(binary).arg(flag).output()?;
            assert!(
                output.status.success(),
                "{name} {flag} failed: {:?}",
                output.stderr
            );
            assert_eq!(
                String::from_utf8(output.stdout)?.trim(),
                env!("CARGO_PKG_VERSION"),
                "{name} {flag}"
            );
        }
    }

    Ok(())
}

#[test]
fn ctb_tile_and_info_work_as_processes() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-info")?;
    let input = directory.join("dem.tif");
    let output = directory.join("tiles");
    write_world_geotiff(&input)?;
    fs::create_dir(&output)?;

    let tile = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "--output-dir",
            output.to_str().ok_or("invalid output path")?,
        ])
        .arg(&input)
        .output()?;
    assert!(
        tile.status.success(),
        "ctb-tile failed: {}",
        String::from_utf8_lossy(&tile.stderr)
    );
    let terrain = output.join("0/0/0.terrain");
    assert!(terrain.exists());

    let info = Command::new(env!("CARGO_BIN_EXE_ctb-info"))
        .arg(&terrain)
        .output()?;
    assert!(info.status.success());
    let stdout = String::from_utf8(info.stdout)?;
    assert_eq!(stdout, " None\nTile type: all land\n");

    let heights_info = Command::new(env!("CARGO_BIN_EXE_ctb-info"))
        .args(["-e", "-c", "-t"])
        .arg(&terrain)
        .output()?;
    assert!(heights_info.status.success());
    let heights_stdout = String::from_utf8(heights_info.stdout)?;
    assert!(heights_stdout.starts_with("Heights:\n5500 5500 5500 "));
    assert!(heights_stdout.ends_with('\n'));
    assert_eq!(heights_stdout.lines().count(), 66);

    let invalid_info = Command::new(env!("CARGO_BIN_EXE_ctb-info"))
        .arg(directory.join("missing.terrain"))
        .output()?;
    assert!(!invalid_info.status.success());
    assert!(String::from_utf8(invalid_info.stderr)?.starts_with("Error: "));
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_extents_and_export_work_as_processes() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("extents-export")?;
    let input = directory.join("dem.tif");
    let extents = directory.join("extents");
    write_world_geotiff(&input)?;
    fs::create_dir(&extents)?;

    let extents_output = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args([
            "--output-dir",
            extents.to_str().ok_or("invalid extents path")?,
        ])
        .arg(&input)
        .output()?;
    assert!(extents_output.status.success());
    assert!(
        extents.join("0.geojson").exists(),
        "generated files: {:?}",
        fs::read_dir(&extents)?.collect::<Result<Vec<_>, _>>()?
    );
    assert!(String::from_utf8(extents_output.stdout)?.contains("creating "));

    let terrain_path = directory.join("tile.terrain");
    HeightmapTerrain::new(
        vec![5000; HEIGHTMAP_SAMPLE_COUNT],
        ChildMask::empty(),
        WaterMask::AllLand,
    )?
    .write_gzip(&terrain_path)?;
    let exported = directory.join("tile.tif");
    let export_output = Command::new(env!("CARGO_BIN_EXE_ctb-export"))
        .args([
            "-i",
            terrain_path.to_str().ok_or("invalid terrain path")?,
            "-z",
            "0",
            "-x",
            "0",
            "-y",
            "0",
            "-o",
            exported.to_str().ok_or("invalid export path")?,
        ])
        .output()?;
    assert!(export_output.status.success());
    assert!(exported.exists());
    assert_eq!(
        String::from_utf8(export_output.stdout)?,
        format!(
            "Creating {} using zoom 0 from tile 0,0\n",
            exported.display()
        )
    );

    let fallback_export = directory.join("fallback.tif");
    let missing_export = Command::new(env!("CARGO_BIN_EXE_ctb-export"))
        .args(["-i"])
        .arg(directory.join("missing.terrain"))
        .args(["-z", "0", "-x", "0", "-y", "0", "-o"])
        .arg(&fallback_export)
        .output()?;
    assert!(missing_export.status.success());
    assert!(String::from_utf8(missing_export.stderr)?.starts_with("Error: "));
    assert_eq!(
        String::from_utf8(missing_export.stdout)?,
        format!(
            "Creating {} using zoom 0 from tile 0,0\n",
            fallback_export.display()
        )
    );
    let fallback = open_geotiff(&fallback_export)?;
    let mut fallback_samples = vec![0_i16; 1];
    fallback.read_window_into_typed::<i16>(0, 0, 0, 0, 1, 1, &mut fallback_samples)?;
    assert_eq!(fallback_samples, [0]);
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_extents_honours_geodetic_zoom_options() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("extents-options")?;
    let input = directory.join("dem.tif");
    let extents = directory.join("extents");
    write_small_geotiff(&input)?;
    fs::create_dir(&extents)?;

    let output = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args(["-p", "geodetic", "-t", "65", "-s", "1", "-e", "1", "-o"])
        .arg(&extents)
        .arg(&input)
        .output()?;
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(extents.join("1.geojson").exists());
    assert!(!extents.join("0.geojson").exists());

    let mercator = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args(["-p", "mercator", "-o"])
        .arg(&extents)
        .arg(&input)
        .output()?;
    assert!(mercator.status.success(), "{:?}", mercator.stderr);
    assert!(
        extents.join("0.geojson").exists(),
        "generated files: {:?}",
        fs::read_dir(&extents)?.collect::<Result<Vec<_>, _>>()?
    );

    let missing_directory = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args(["-o"])
        .arg(directory.join("missing"))
        .arg(&input)
        .output()?;
    assert!(missing_directory.status.success());
    assert!(String::from_utf8(missing_directory.stderr)?.contains("File could not be opened:"));

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_and_extents_accept_vrt_input() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("vrt-input")?;
    let source = directory.join("dem.tif");
    let vrt = directory.join("dem.vrt");
    write_small_geotiff(&source)?;
    VrtBuilder::with_size(2, 2)
        .with_srs("EPSG:4326")
        .with_geo_transform(GeoTransform::north_up(-1.0, 1.0, 1.0, -1.0))
        .add_band(VrtBand::simple(
            1,
            RasterDataType::Float64,
            VrtSource::simple(&source, 1).with_window(SourceWindow::identity(2, 2)),
        ))?
        .build_file(&vrt)?;

    let terrain = directory.join("terrain");
    fs::create_dir(&terrain)?;
    let tile = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-s", "0", "-e", "0", "-o"])
        .arg(&terrain)
        .arg(&vrt)
        .output()?;
    assert!(tile.status.success(), "{:?}", tile.stderr);
    assert!(terrain.join("0/0/0.terrain").exists());

    let extents = directory.join("extents");
    fs::create_dir(&extents)?;
    let extents_output = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args(["-s", "0", "-e", "0", "-o"])
        .arg(&extents)
        .arg(&vrt)
        .output()?;
    assert!(
        extents_output.status.success(),
        "{:?}",
        extents_output.stderr
    );
    assert!(extents.join("0.geojson").exists());

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_rejects_non_geotiff_raster_formats() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("unsupported-raster")?;
    for extension in ["nc", "h5", "jp2"] {
        let input = directory.join(format!("dem.{extension}"));
        let output = directory.join(format!("output-{extension}"));
        fs::write(&input, [0_u8])?;
        fs::create_dir(&output)?;

        let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
            .args(["-s", "0", "-e", "0", "-o"])
            .arg(&output)
            .arg(&input)
            .output()?;
        assert!(
            !result.status.success(),
            "{extension} input must be rejected before tiles are written"
        );
        assert!(
            String::from_utf8(result.stderr)?.contains("OxiGeo 0.2.3"),
            "{extension} should be rejected by the OxiGeo capability guard"
        );
        assert!(!output.join("0/0/0.terrain").exists());
        assert!(!output.join("0/0/0.tif").exists());
    }
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_honours_zoom_range_and_supported_resampling() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = temporary_directory("tile-options")?;
    let input = directory.join("dem.tif");
    write_small_geotiff(&input)?;

    for method in [
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
        let output = directory.join(method);
        fs::create_dir(&output)?;
        let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
            .args(["-s", "1", "-e", "1", "-r", method, "-t", "65"])
            .arg("--output-dir")
            .arg(&output)
            .arg(&input)
            .output()?;
        assert!(result.status.success(), "{method}: {:?}", result.stderr);
        assert!(output.join("1/1/0.terrain").exists());
        assert!(!output.join("0").exists());
        assert!(!output.join("2").exists());
    }

    let terrain_option_output = directory.join("terrain-option");
    fs::create_dir(&terrain_option_output)?;
    let terrain_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-n", "COMPRESS=DEFLATE", "-o"])
        .arg(&terrain_option_output)
        .arg(&input)
        .output()?;
    assert!(!terrain_option.status.success());
    assert!(!terrain_option_output.join("0").exists());

    let mercator_output = directory.join("terrain-mercator");
    fs::create_dir(&mercator_output)?;
    let unsupported_profile = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["--profile", "mercator", "-s", "0", "-e", "0", "-o"])
        .arg(&mercator_output)
        .arg(&input)
        .output()?;
    assert!(
        unsupported_profile.status.success(),
        "{:?}",
        unsupported_profile.stderr
    );
    assert!(mercator_output.join("0/0/0.terrain").exists());

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_writes_geotiff_rastertiler_tiles() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-gtiff")?;
    let input = directory.join("dem.tif");
    let output = directory.join("tiles");
    write_small_geotiff(&input)?;
    fs::create_dir(&output)?;

    let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-t", "4", "-s", "0", "-e", "0", "-o"])
        .arg(&output)
        .arg(&input)
        .output()?;
    assert!(result.status.success(), "{:?}", result.stderr);
    let tile = output.join("0/0/0.tif");
    assert!(tile.exists());
    let file = open_geotiff(&tile)?;
    assert_eq!(file.width(), 4);
    assert_eq!(file.height(), 4);
    assert_eq!(file.epsg_code(), Some(4326));
    let transform = file.geo_transform().ok_or("missing GeoTIFF transform")?;
    assert_eq!(transform.origin_x, -180.0);
    assert_eq!(transform.origin_y, 90.0);
    assert_eq!(transform.pixel_width, 45.0);
    assert_eq!(transform.pixel_height, -45.0);

    let deflate_output = directory.join("deflate");
    fs::create_dir(&deflate_output)?;
    let deflate_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=DEFLATE", "-o"])
        .arg(&deflate_output)
        .arg(&input)
        .output()?;
    assert!(
        deflate_option.status.success(),
        "{:?}",
        deflate_option.stderr
    );
    let deflate_file = open_geotiff(&deflate_output.join("0/0/0.tif"))?;
    assert_eq!(deflate_file.width(), 65);

    let lzw_output = directory.join("lzw");
    fs::create_dir(&lzw_output)?;
    let lzw_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=LZW", "-o"])
        .arg(&lzw_output)
        .arg(&input)
        .output()?;
    assert!(lzw_option.status.success(), "{:?}", lzw_option.stderr);
    let lzw_file = open_geotiff(&lzw_output.join("0/0/0.tif"))?;
    assert_eq!(lzw_file.width(), 65);

    let zstd_output = directory.join("zstd");
    fs::create_dir(&zstd_output)?;
    let zstd_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=ZSTD", "-o"])
        .arg(&zstd_output)
        .arg(&input)
        .output()?;
    assert!(zstd_option.status.success(), "{:?}", zstd_option.stderr);
    let zstd_file = open_geotiff(&zstd_output.join("0/0/0.tif"))?;
    assert_eq!(zstd_file.width(), 65);
    assert_eq!(zstd_file.epsg_code(), Some(4326));

    let byte_input = directory.join("byte-dem.tif");
    write_u8_geotiff(&byte_input)?;
    let jpeg_output = directory.join("jpeg");
    fs::create_dir(&jpeg_output)?;
    let jpeg_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=JPEG", "-o"])
        .arg(&jpeg_output)
        .arg(&byte_input)
        .output()?;
    assert!(!jpeg_option.status.success());
    assert!(!jpeg_output.join("0/0/0.tif").exists());

    let lerc_output = directory.join("lerc");
    fs::create_dir(&lerc_output)?;
    let lerc_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=LERC", "-o"])
        .arg(&lerc_output)
        .arg(&input)
        .output()?;
    assert!(!lerc_option.status.success());
    assert!(!lerc_output.join("0/0/0.tif").exists());

    let invalid_jpeg_output = directory.join("invalid-jpeg");
    fs::create_dir(&invalid_jpeg_output)?;
    let invalid_jpeg = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "COMPRESS=JPEG", "-o"])
        .arg(&invalid_jpeg_output)
        .arg(&input)
        .output()?;
    assert!(!invalid_jpeg.status.success());
    assert!(!invalid_jpeg_output.join("0/0/0.tif").exists());

    let bigtiff_output = directory.join("bigtiff");
    fs::create_dir(&bigtiff_output)?;
    let bigtiff_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-n", "BIGTIFF=YES", "-o"])
        .arg(&bigtiff_output)
        .arg(&input)
        .output()?;
    assert!(
        bigtiff_option.status.success(),
        "{:?}",
        bigtiff_option.stderr
    );
    assert_eq!(
        fs::read(bigtiff_output.join("0/0/0.tif"))?.get(..4),
        Some([b'I', b'I', 43, 0].as_slice())
    );

    for (label, bigtiff_value, expected_header) in [
        ("classic", "NO", &[b'I', b'I', 42, 0][..]),
        ("auto", "IF_NEEDED", &[b'I', b'I', 42, 0][..]),
    ] {
        let output = directory.join(format!("bigtiff-{label}"));
        fs::create_dir(&output)?;
        let option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
            .args([
                "-f",
                "GTiff",
                "-n",
                &format!("BIGTIFF={bigtiff_value}"),
                "-o",
            ])
            .arg(&output)
            .arg(&input)
            .output()?;
        assert!(
            option.status.success(),
            "BIGTIFF={bigtiff_value}: {:?}",
            option.stderr
        );
        assert_eq!(
            fs::read(output.join("0/0/0.tif"))?.get(..4),
            Some(expected_header),
            "BIGTIFF={bigtiff_value} header"
        );
    }

    let predictor_output = directory.join("predictor");
    fs::create_dir(&predictor_output)?;
    let predictor_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "COMPRESS=DEFLATE",
            "-n",
            "PREDICTOR=3",
            "-o",
        ])
        .arg(&predictor_output)
        .arg(&input)
        .output()?;
    assert!(
        predictor_option.status.success(),
        "{:?}",
        predictor_option.stderr
    );
    let predictor_file = open_geotiff(&predictor_output.join("0/0/0.tif"))?;
    assert_eq!(predictor_file.width(), 65);

    let tiled_output = directory.join("tiled");
    fs::create_dir(&tiled_output)?;
    let tiled_option = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "TILED=YES",
            "-n",
            "BLOCKXSIZE=32",
            "-n",
            "BLOCKYSIZE=16",
            "-o",
        ])
        .arg(&tiled_output)
        .arg(&input)
        .output()?;
    assert!(tiled_option.status.success(), "{:?}", tiled_option.stderr);
    assert!(tiled_output.join("0/0/0.tif").exists());

    let invalid_predictor_output = directory.join("invalid-predictor");
    fs::create_dir(&invalid_predictor_output)?;
    let invalid_predictor = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "COMPRESS=DEFLATE",
            "-n",
            "PREDICTOR=2",
            "-o",
        ])
        .arg(&invalid_predictor_output)
        .arg(&input)
        .output()?;
    assert!(!invalid_predictor.status.success());
    assert!(!invalid_predictor_output.join("0/0/0.tif").exists());

    let integer_predictor_2_output = directory.join("integer-predictor-2");
    fs::create_dir(&integer_predictor_2_output)?;
    let integer_predictor_2 = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "COMPRESS=DEFLATE",
            "-n",
            "PREDICTOR=2",
            "-t",
            "4",
            "-s",
            "0",
            "-e",
            "0",
            "-o",
        ])
        .arg(&integer_predictor_2_output)
        .arg(&byte_input)
        .output()?;
    assert!(
        integer_predictor_2.status.success(),
        "{:?}",
        integer_predictor_2.stderr
    );
    let integer_predictor_2_file = open_geotiff(&integer_predictor_2_output.join("0/0/0.tif"))?;
    assert_eq!(integer_predictor_2_file.width(), 4);

    let integer_predictor_3_output = directory.join("integer-predictor-3");
    fs::create_dir(&integer_predictor_3_output)?;
    let integer_predictor_3 = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "COMPRESS=DEFLATE",
            "-n",
            "PREDICTOR=3",
            "-o",
        ])
        .arg(&integer_predictor_3_output)
        .arg(&byte_input)
        .output()?;
    assert!(!integer_predictor_3.status.success());
    assert!(!integer_predictor_3_output.join("0/0/0.tif").exists());

    let predictor_4_output = directory.join("predictor-4");
    fs::create_dir(&predictor_4_output)?;
    let predictor_4 = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f",
            "GTiff",
            "-n",
            "COMPRESS=DEFLATE",
            "-n",
            "PREDICTOR=4",
            "-o",
        ])
        .arg(&predictor_4_output)
        .arg(&input)
        .output()?;
    assert!(!predictor_4.status.success());
    assert!(!predictor_4_output.join("0/0/0.tif").exists());

    let incompatible_profile_output = directory.join("incompatible-mercator");
    fs::create_dir(&incompatible_profile_output)?;
    let incompatible_profile = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f", "GTiff", "-p", "mercator", "-t", "4", "-s", "0", "-e", "0", "-o",
        ])
        .arg(&incompatible_profile_output)
        .arg(&input)
        .output()?;
    assert!(
        incompatible_profile.status.success(),
        "4326 to 3857 reprojection failed: {:?}",
        incompatible_profile.stderr
    );
    let reprojected = open_geotiff(&incompatible_profile_output.join("0/0/0.tif"))?;
    assert_eq!(reprojected.epsg_code(), Some(3857));

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_writes_mercator_direct_source_gtiff() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-gtiff-mercator")?;
    let input = directory.join("dem-3857.tif");
    let output = directory.join("tiles");
    write_mercator_world_geotiff(&input)?;
    fs::create_dir(&output)?;

    let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f", "GTiff", "-p", "mercator", "-t", "4", "-s", "0", "-e", "0", "-o",
        ])
        .arg(&output)
        .arg(&input)
        .output()?;
    assert!(result.status.success(), "{:?}", result.stderr);
    let file = open_geotiff(&output.join("0/0/0.tif"))?;
    assert_eq!(file.epsg_code(), Some(3857));
    let transform = file.geo_transform().ok_or("missing GeoTIFF transform")?;
    let origin_shift = std::f64::consts::PI * 6_378_137.0;
    assert_eq!(transform.origin_x, -origin_shift);
    assert_eq!(transform.origin_y, origin_shift);
    assert_eq!(transform.pixel_width, origin_shift / 2.0);
    assert_eq!(transform.pixel_height, -origin_shift / 2.0);
    let values = read_f64_window(&file, 4, 4)?;
    assert!(values.iter().all(|value| *value == 7.0));

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_reprojects_arbitrary_epsg_input() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-projected-epsg")?;
    let input = directory.join("dem-32630.tif");
    write_utm_geotiff(&input)?;

    let terrain_output = directory.join("terrain");
    fs::create_dir(&terrain_output)?;
    let terrain_result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-p", "geodetic", "-s", "6", "-e", "6", "-o"])
        .arg(&terrain_output)
        .arg(&input)
        .output()?;
    assert!(
        terrain_result.status.success(),
        "{:?}",
        terrain_result.stderr
    );
    assert!(terrain_output.join("6/62/32.terrain").exists());

    let mercator_output = directory.join("mercator");
    fs::create_dir(&mercator_output)?;
    let mercator_result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "-f", "GTiff", "-p", "mercator", "-t", "4", "-s", "6", "-e", "6", "-o",
        ])
        .arg(&mercator_output)
        .arg(&input)
        .output()?;
    assert!(
        mercator_result.status.success(),
        "{:?}",
        mercator_result.stderr
    );
    let tile = open_geotiff(&mercator_output.join("6/31/32.tif"))?;
    assert_eq!(tile.epsg_code(), Some(3857));
    let values = read_f64_window(&tile, 4, 4)?;
    assert!(
        values.contains(&9.0),
        "reprojected Mercator tile did not sample the UTM fixture: {values:?}"
    );

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_gtiff_nearest_uses_strict_source_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-gtiff-nearest")?;
    let input = directory.join("dem.tif");
    let output = directory.join("tiles");
    write_small_geotiff(&input)?;
    fs::create_dir(&output)?;
    let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-f", "GTiff", "-r", "nearest", "-s", "0", "-e", "0", "-o"])
        .arg(&output)
        .arg(&input)
        .output()?;
    assert!(result.status.success(), "{:?}", result.stderr);
    let file = open_geotiff(&output.join("0/0/0.tif"))?;
    let samples = read_f64_window(&file, 65, 65)?;
    // The final z0 column is centred at -1.3846°, outside the [-1, 1]
    // source bounds. RasterTiler must retain its initial destination 0.
    assert_eq!(samples[32 * 65 + 64], 0.0);
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_matches_worker_progress_and_resume_contracts() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = temporary_directory("tile-workers")?;
    let input = directory.join("dem.tif");
    write_small_geotiff(&input)?;
    let sequential = directory.join("sequential");
    let parallel = directory.join("parallel");
    let quiet = directory.join("quiet");
    let verbose = directory.join("verbose");
    for output in [&sequential, &parallel, &quiet, &verbose] {
        fs::create_dir(output)?;
    }

    let one_worker = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-c", "1", "-s", "1", "-e", "1", "-o"])
        .arg(&sequential)
        .arg(&input)
        .output()?;
    assert!(one_worker.status.success());
    assert!(one_worker.stdout.is_empty());

    let two_workers = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-c", "2", "-s", "1", "-e", "1", "-o"])
        .arg(&parallel)
        .arg(&input)
        .output()?;
    assert!(two_workers.status.success());
    assert!(two_workers.stdout.is_empty());

    for x in 1..=2 {
        for y in 0..=1 {
            let tile = format!("1/{x}/{y}.terrain");
            let left = HeightmapTerrain::read_gzip(sequential.join(&tile))?;
            let right = HeightmapTerrain::read_gzip(parallel.join(&tile))?;
            assert_eq!(left, right);
        }
    }

    let quiet_output = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-q", "-s", "1", "-e", "1", "-o"])
        .arg(&quiet)
        .arg(&input)
        .output()?;
    assert!(quiet_output.status.success());
    assert!(quiet_output.stdout.is_empty());

    let verbose_output = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-v", "-c", "2", "-s", "1", "-e", "1", "-o"])
        .arg(&verbose)
        .arg(&input)
        .output()?;
    assert!(verbose_output.status.success());
    let verbose_stdout = String::from_utf8(verbose_output.stdout)?;
    assert!(verbose_stdout.contains("created "));
    assert!(verbose_stdout.contains(" in thread "));

    let sentinel = sequential.join("1/1/0.terrain");
    fs::write(&sentinel, b"preserve-existing-tile")?;
    let resumed = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-R", "-s", "1", "-e", "1", "-o"])
        .arg(&sequential)
        .arg(&input)
        .output()?;
    assert!(resumed.status.success());
    assert_eq!(fs::read(&sentinel)?, b"preserve-existing-tile");

    let absent_directory = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-o"])
        .arg(directory.join("absent"))
        .arg(&input)
        .output()?;
    assert!(!absent_directory.status.success());
    assert!(String::from_utf8(absent_directory.stderr)?.contains("does not exist"));

    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_tile_prints_display_for_invalid_zoom_range() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("tile-invalid-zoom")?;
    let input = directory.join("dem.tif");
    let output = directory.join("tiles");
    write_small_geotiff(&input)?;
    fs::create_dir(&output)?;

    let result = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args([
            "--output-dir",
            output.to_str().ok_or("invalid output path")?,
            "--start-zoom",
            "8",
            "--end-zoom",
            "12",
        ])
        .arg(&input)
        .output()?;

    assert!(!result.status.success());
    let stderr = String::from_utf8(result.stderr)?;
    assert!(stderr.starts_with("Error: "), "{stderr}");
    assert!(stderr.contains("maximum ("), "{stderr}");
    assert!(stderr.contains(">= start (8)"), "{stderr}");
    assert!(stderr.contains(">= end (12)"), "{stderr}");
    assert!(!stderr.contains("InvalidZoomRange"), "{stderr}");

    fs::remove_dir_all(directory)?;
    Ok(())
}
