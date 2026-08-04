use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use ctb_rs::terrain::{ChildMask, HEIGHTMAP_SAMPLE_COUNT, HeightmapTerrain, WaterMask};
use geotiff_reader::GeoTiffFile;
use geotiff_writer::GeoTiffBuilder;
use ndarray::Array2;

fn temporary_directory(label: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("ctb-rs-cli-{label}-{nanos}"));
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn write_world_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let samples = Array2::from_elem((65, 65), 100.0_f64);
    GeoTiffBuilder::new(65, 65)
        .geographic_epsg(4326)
        .pixel_scale(360.0 / 65.0, 180.0 / 65.0)
        .origin(-180.0, 90.0)
        .write_2d(path, samples.view())?;
    Ok(())
}

fn write_small_geotiff(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let samples = Array2::from_shape_vec((2, 2), vec![100.0_f64, 200.0, 300.0, 400.0])?;
    GeoTiffBuilder::new(2, 2)
        .geographic_epsg(4326)
        .pixel_scale(1.0, 1.0)
        .origin(-1.0, 1.0)
        .write_2d(path, samples.view())?;
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
    assert!(tile.status.success());
    let terrain = output.join("0/0/0.terrain");
    assert!(terrain.exists());

    let info = Command::new(env!("CARGO_BIN_EXE_ctb-info"))
        .arg(&terrain)
        .output()?;
    assert!(info.status.success());
    let stdout = String::from_utf8(info.stdout)?;
    assert_eq!(stdout, "Child tiles: None\nTile type: all land\n");

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
    assert!(extents.join("0.geojson").exists());
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
    let fallback = GeoTiffFile::open(&fallback_export)?;
    let fallback_sample = fallback
        .read_band_window::<i16>(0, 0, 0, 1, 1)?
        .iter()
        .copied()
        .next()
        .ok_or("fallback GeoTIFF did not contain a sample")?;
    assert_eq!(fallback_sample, 0);
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
        .args(["-p", "mercator"])
        .arg(&input)
        .output()?;
    assert!(!mercator.status.success());

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
fn ctb_tile_honours_zoom_range_and_supported_resampling() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = temporary_directory("tile-options")?;
    let input = directory.join("dem.tif");
    write_small_geotiff(&input)?;

    for method in ["nearest", "bilinear", "average"] {
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

    let invalid_method = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["-r", "cubic"])
        .arg(&input)
        .output()?;
    assert!(!invalid_method.status.success());

    let unsupported_profile = Command::new(env!("CARGO_BIN_EXE_ctb-tile"))
        .args(["--profile", "mercator"])
        .arg(&input)
        .output()?;
    assert!(!unsupported_profile.status.success());

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
