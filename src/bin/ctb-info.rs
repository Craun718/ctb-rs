use std::{error::Error, path::PathBuf};

use clap::Parser;
use ctb_rs::terrain::{HEIGHTMAP_TILE_SIZE, HeightmapTerrain, WaterMask};

#[derive(Debug, Parser)]
#[command(about = "Inspect a CTB heightmap terrain file")]
struct Arguments {
    /// Print all raw CTB heightmap values as rows.
    #[arg(short = 'e', long)]
    show_heights: bool,

    /// Do not print child-tile availability.
    #[arg(short = 'c', long)]
    no_child: bool,

    /// Do not print water/land classification.
    #[arg(short = 't', long)]
    no_type: bool,

    /// Input gzip-compressed CTB terrain file.
    input: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let terrain = HeightmapTerrain::read_gzip(&arguments.input)?;

    if arguments.show_heights {
        print!("Heights:");
        for row in terrain.heights.chunks(HEIGHTMAP_TILE_SIZE) {
            println!();
            for value in row {
                print!("{value} ");
            }
        }
        println!();
    }
    if !arguments.no_child {
        let mut names = Vec::new();
        if terrain
            .children
            .contains(ctb_rs::terrain::ChildMask::SOUTH_WEST)
        {
            names.push("SW");
        }
        if terrain
            .children
            .contains(ctb_rs::terrain::ChildMask::SOUTH_EAST)
        {
            names.push("SE");
        }
        if terrain
            .children
            .contains(ctb_rs::terrain::ChildMask::NORTH_WEST)
        {
            names.push("NW");
        }
        if terrain
            .children
            .contains(ctb_rs::terrain::ChildMask::NORTH_EAST)
        {
            names.push("NE");
        }
        if names.is_empty() {
            println!("Child tiles: None");
        } else {
            println!("Child tiles: {}", names.join(" "));
        }
    }
    if !arguments.no_type {
        let description = match terrain.water_mask {
            WaterMask::AllLand => "all land",
            WaterMask::AllWater => "all water",
            WaterMask::Detailed(_) => "water mask",
        };
        println!("Tile type: {description}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Arguments;

    #[test]
    fn parses_original_info_flags() {
        let arguments = Arguments::try_parse_from(["ctb-info", "-e", "-c", "-t", "tile.terrain"]);
        assert!(arguments.is_ok());
    }
}
