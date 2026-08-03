use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use ctb_rs::terrain::{ChildMask, HEIGHTMAP_SAMPLE_COUNT, HeightmapTerrain, WaterMask};
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
    assert!(stdout.contains("Child tiles: None"));
    assert!(stdout.contains("Tile type: all land"));
    fs::remove_dir_all(directory)?;
    Ok(())
}

#[test]
fn ctb_extents_and_export_work_as_processes() -> Result<(), Box<dyn std::error::Error>> {
    let directory = temporary_directory("extents-export")?;
    let input = directory.join("dem.tif");
    let extents = directory.join("extents");
    write_world_geotiff(&input)?;

    let extents_output = Command::new(env!("CARGO_BIN_EXE_ctb-extents"))
        .args([
            "--output-dir",
            extents.to_str().ok_or("invalid extents path")?,
        ])
        .arg(&input)
        .output()?;
    assert!(extents_output.status.success());
    assert!(extents.join("0.geojson").exists());

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
