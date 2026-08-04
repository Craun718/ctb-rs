#!/usr/bin/env zsh
# Compare ctb-rs heightmap payloads with an original CTB executable.
#
# Required environment:
#   CTB_ORACLE_BIN=/path/to/original/ctb-tile
#   CTB_RS_BIN=/path/to/ctb-rs/ctb-tile
#
# Prerequisite: gdal_translate must be available on PATH. This is a developer
# oracle, not part of the normal Rust test suite or the production dependency
# graph.

set -euo pipefail

if [[ -z "${CTB_ORACLE_BIN:-}" || -z "${CTB_RS_BIN:-}" ]]; then
  print -u2 -- "set CTB_ORACLE_BIN and CTB_RS_BIN to the two ctb-tile executables"
  exit 2
fi
if [[ ! -x "$CTB_ORACLE_BIN" || ! -x "$CTB_RS_BIN" ]]; then
  print -u2 -- "CTB_ORACLE_BIN and CTB_RS_BIN must both be executable files"
  exit 2
fi
if ! command -v gdal_translate >/dev/null 2>&1 || ! command -v gdaladdo >/dev/null 2>&1; then
  print -u2 -- "gdal_translate and gdaladdo are required to create oracle GeoTIFFs"
  exit 2
fi

repo_root="$(git rev-parse --show-toplevel)"
fixture="$repo_root/tests/fixtures/oracle-source.asc"
if [[ ! -f "$fixture" ]]; then
  print -u2 -- "missing fixture: $fixture"
  exit 2
fi

work_directory="$(mktemp -d "${TMPDIR:-/tmp}/ctb-rs-oracle.XXXXXX")"
cleanup() {
  rm -rf -- "$work_directory"
}
trap cleanup EXIT INT TERM

source_tiff="$work_directory/oracle-source.tif"
gdal_translate -q -of GTiff -a_srs EPSG:4326 "$fixture" "$source_tiff"
float_negative_tiff="$work_directory/oracle-source-float-negative.tif"
gdal_translate -q -of GTiff -ot Float32 -scale 100 400 -100 50 "$source_tiff" "$float_negative_tiff"
compressed_overview_tiff="$work_directory/oracle-source-tiled-overview.tif"
gdal_translate -q -of GTiff -co TILED=YES -co COMPRESS=DEFLATE "$source_tiff" "$compressed_overview_tiff"
gdaladdo -q -r average "$compressed_overview_tiff" 2
high_resolution_overview_tiff="$work_directory/oracle-source-high-resolution-overview.tif"
gdal_translate -q -of GTiff -outsize 720 360 -a_ullr -179.9 89.9 179.9 -89.9 -co TILED=YES -co COMPRESS=DEFLATE "$source_tiff" "$high_resolution_overview_tiff"
high_resolution_tiff="$work_directory/oracle-source-high-resolution.tif"
gdal_translate -q -of GTiff "$high_resolution_overview_tiff" "$high_resolution_tiff"
gdaladdo -q -r average "$high_resolution_overview_tiff" 2

compare_tiles() {
  local oracle_directory="$1"
  local rust_directory="$2"
  local relative_path
  local oracle_tile
  local rust_tile
  local oracle_raw
  local rust_raw
  local oracle_paths="$work_directory/oracle-paths"
  local rust_paths="$work_directory/rust-paths"

  (cd "$oracle_directory" && find . -type f -name '*.terrain' -print | sort) > "$oracle_paths"
  (cd "$rust_directory" && find . -type f -name '*.terrain' -print | sort) > "$rust_paths"
  if ! cmp -s "$oracle_paths" "$rust_paths"; then
    print -u2 -- "terrain tile path sets differ"
    return 1
  fi

  while IFS= read -r relative_path; do
    oracle_tile="$oracle_directory/$relative_path"
    rust_tile="$rust_directory/$relative_path"
    if [[ ! -f "$rust_tile" ]]; then
      print -u2 -- "Rust output is missing tile: $relative_path"
      return 1
    fi
    oracle_raw="$work_directory/oracle.raw"
    rust_raw="$work_directory/rust.raw"
    gzip -dc "$oracle_tile" > "$oracle_raw"
    gzip -dc "$rust_tile" > "$rust_raw"
    if ! cmp -s "$oracle_raw" "$rust_raw"; then
      print -u2 -- "terrain payload differs: $relative_path"
      return 1
    fi
  done < <(cd "$oracle_directory" && find . -type f -name '*.terrain' -print | sed 's|^./||' | sort)
}

for source_name in plain float-negative tiled-overview high-resolution high-resolution-overview; do
  if [[ "$source_name" == plain ]]; then
    input_tiff="$source_tiff"
  elif [[ "$source_name" == float-negative ]]; then
    input_tiff="$float_negative_tiff"
  elif [[ "$source_name" == high-resolution-overview ]]; then
    input_tiff="$high_resolution_overview_tiff"
  elif [[ "$source_name" == high-resolution ]]; then
    input_tiff="$high_resolution_tiff"
  else
    input_tiff="$compressed_overview_tiff"
  fi
  for method in nearest bilinear average; do
    for range_name in automatic limited; do
      oracle_directory="$work_directory/original-$source_name-$method-$range_name"
      rust_directory="$work_directory/rust-$source_name-$method-$range_name"
      mkdir -p "$oracle_directory" "$rust_directory"
      range_arguments=()
      if [[ "$range_name" == limited ]]; then
        range_arguments=(-s 1 -e 1)
      fi

      "$CTB_ORACLE_BIN" -q -o "$oracle_directory" -r "$method" "${range_arguments[@]}" "$input_tiff"
      "$CTB_RS_BIN" -o "$rust_directory" -r "$method" "${range_arguments[@]}" "$input_tiff" >/dev/null
      compare_tiles "$oracle_directory" "$rust_directory"
      if [[ "$source_name" == tiled-overview ]]; then
        compare_tiles "$work_directory/rust-plain-$method-$range_name" "$rust_directory"
      fi
      print -- "verified $source_name $method ($range_name)"
    done
  done
done
