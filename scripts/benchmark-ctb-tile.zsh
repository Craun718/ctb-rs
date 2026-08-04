#!/usr/bin/env zsh
# Measure ctb-rs tile generation on a reproducible tiled/DEFLATE DEM.
#
# Usage:
#   CTB_RS_BIN=target/release/ctb-tile scripts/benchmark-ctb-tile.zsh [size] [workers]
#
# Prerequisite: gdal_translate and a built ctb-tile executable. This developer
# benchmark does not enter cargo test and writes all generated data under a
# temporary directory.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
fixture="$repo_root/tests/fixtures/oracle-source.asc"
ctb_bin="${CTB_RS_BIN:-$repo_root/target/debug/ctb-tile}"
size="${1:-512}"
workers="${2:-2}"

if [[ ! -x "$ctb_bin" ]]; then
  print -u2 -- "ctb-tile is not executable: $ctb_bin"
  exit 2
fi
if ! command -v gdal_translate >/dev/null 2>&1; then
  print -u2 -- "gdal_translate is required"
  exit 2
fi
if [[ ! "$size" =~ '^[1-9][0-9]*$' || ! "$workers" =~ '^[1-9][0-9]*$' ]]; then
  print -u2 -- "size and workers must be positive integers"
  exit 2
fi

work_directory="$(mktemp -d "${TMPDIR:-/tmp}/ctb-rs-benchmark.XXXXXX")"
cleanup() { rm -rf -- "$work_directory"; }
trap cleanup EXIT INT TERM

source_tiff="$work_directory/source.tif"
gdal_translate -q -of GTiff -a_srs EPSG:4326 -r nearest -outsize "$size" "$size" \
  -co TILED=YES -co BLOCKXSIZE=128 -co BLOCKYSIZE=128 -co COMPRESS=DEFLATE \
  "$fixture" "$source_tiff"

run_case() {
  local name="$1"
  local count="$2"
  local output="$work_directory/$name"
  local start
  local finish
  mkdir -p "$output"
  start="$(date +%s)"
  "$ctb_bin" -q -c "$count" -o "$output" "$source_tiff"
  finish="$(date +%s)"
  print -- "$name workers=$count seconds=$((finish - start)) tiles=$(find "$output" -name '*.terrain' -type f | wc -l | tr -d ' ')"
}

run_case single 1
run_case parallel "$workers"

single="$work_directory/single"
parallel="$work_directory/parallel"
diff -u \
  <(cd "$single" && find . -name '*.terrain' -type f | sort) \
  <(cd "$parallel" && find . -name '*.terrain' -type f | sort)
while IFS= read -r path; do
  gzip -dc "$single/$path" > "$work_directory/single.raw"
  gzip -dc "$parallel/$path" > "$work_directory/parallel.raw"
  cmp "$work_directory/single.raw" "$work_directory/parallel.raw"
done < <(cd "$single" && find . -name '*.terrain' -type f | sort)
print -- "payloads=identical"
