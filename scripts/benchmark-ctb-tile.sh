#!/bin/sh
# Measure ctb-rs tile generation on a reproducible tiled/DEFLATE DEM.
#
# Usage:
#   CTB_RS_BIN=target/release/ctb-tile scripts/benchmark-ctb-tile.sh [size] [workers]
#
# Prerequisite: gdal_translate and a built ctb-tile executable. This developer
# benchmark does not enter cargo test and writes all generated data under a
# temporary directory.

set -eu

repo_root="$(git rev-parse --show-toplevel)"
fixture="$repo_root/tests/fixtures/oracle-source.asc"
ctb_bin="${CTB_RS_BIN:-$repo_root/target/debug/ctb-tile}"
size="${1:-512}"
workers="${2:-2}"

if [ ! -x "$ctb_bin" ]; then
  printf 'ctb-tile is not executable: %s\n' "$ctb_bin" >&2
  exit 2
fi
if ! command -v gdal_translate >/dev/null 2>&1; then
  printf 'gdal_translate is required\n' >&2
  exit 2
fi
case "$size" in
  ''|*[!0-9]*|0*) printf 'size and workers must be positive integers\n' >&2; exit 2;;
esac
case "$workers" in
  ''|*[!0-9]*|0*) printf 'size and workers must be positive integers\n' >&2; exit 2;;
esac

work_directory="$(mktemp -d "${TMPDIR:-/tmp}/ctb-rs-benchmark.XXXXXX")"
cleanup() { rm -rf -- "$work_directory"; }
trap cleanup EXIT INT TERM

source_tiff="$work_directory/source.tif"
gdal_translate -q -of GTiff -a_srs EPSG:4326 -r nearest -outsize "$size" "$size" \
  -co TILED=YES -co BLOCKXSIZE=128 -co BLOCKYSIZE=128 -co COMPRESS=DEFLATE \
  "$fixture" "$source_tiff"

run_case() {
  name="$1"
  count="$2"
  output="$work_directory/$name"
  mkdir -p "$output"
  start="$(date +%s)"
  "$ctb_bin" -q -c "$count" -o "$output" "$source_tiff"
  finish="$(date +%s)"
  printf '%s workers=%s seconds=%s tiles=%s\n' \
    "$name" "$count" "$((finish - start))" \
    "$(find "$output" -name '*.terrain' -type f | wc -l | tr -d ' ')"
}

run_case single 1
run_case parallel "$workers"

single="$work_directory/single"
parallel="$work_directory/parallel"

find "$single" -name '*.terrain' -type f | sort > "$work_directory/single.list"
find "$parallel" -name '*.terrain' -type f | sort > "$work_directory/parallel.list"
diff -u "$work_directory/single.list" "$work_directory/parallel.list"

while IFS= read -r path; do
  gzip -dc "$single/$path" > "$work_directory/single.raw"
  gzip -dc "$parallel/$path" > "$work_directory/parallel.raw"
  cmp "$work_directory/single.raw" "$work_directory/parallel.raw"
done < "$work_directory/single.list"
printf 'payloads=identical\n'
