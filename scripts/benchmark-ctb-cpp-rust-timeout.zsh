#!/usr/bin/env zsh
# Run ctb-tile C++ first, record its wall time, then run Rust with a timeout
# of twice that wall time, and compare common .terrain payloads.
#
# Usage:
#   CTB_CPP_BIN=/path/to/ctb-tile \
#   CTB_CPP_LIBRARY_PATH=/path/to/cpp/lib \
#   CTB_CPP_GDAL_DATA=/path/to/gdal/share \
#   CTB_RS_BIN=target/release/ctb-tile \
#   GNU_TIMEOUT=/opt/homebrew/bin/timeout \
#   scripts/benchmark-ctb-cpp-rust-timeout.zsh \
#     input.tif cpp_output rust_output [--] [ctb-tile args...]
#
# Required tools: env, perl, GNU timeout, gzip, cmp.
# CTB_CPP_LIBRARY_PATH is passed to the C++ process as DYLD_LIBRARY_PATH.
# CTB_CPP_GDAL_DATA is passed to the C++ process as GDAL_DATA.
# The script intentionally does not record private benchmark metadata in the
# repository; it only reports timing, status, and payload comparison counts.

set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cpp_bin="${CTB_CPP_BIN:-}"
cpp_library_path="${CTB_CPP_LIBRARY_PATH:-}"
cpp_gdal_data="${CTB_CPP_GDAL_DATA:-}"
rust_bin="${CTB_RS_BIN:-$repo_root/target/release/ctb-tile}"
timeout_bin="${GNU_TIMEOUT:-}"

usage() {
  print -u2 -- "usage: $ZSH_ARGZERO input.tif cpp_output rust_output [--] [ctb-tile args...]"
  print -u2 -- "  CTB_CPP_BIN and CTB_RS_BIN are required overrides."
}

if (( $# < 3 )); then
  usage
  exit 2
fi

input="$1"
cpp_out="$2"
rust_out="$3"
shift 3
if [[ ${1:-} == "--" ]]; then
  shift
fi
ctb_args=("$@")

if [[ -z "$cpp_bin" ]]; then
  print -u2 -- "CTB_CPP_BIN is required"
  exit 2
fi
if [[ ! -x "$cpp_bin" ]]; then
  print -u2 -- "ctb-tile is not executable: $cpp_bin"
  exit 2
fi
if [[ ! -x "$rust_bin" ]]; then
  print -u2 -- "ctb-tile is not executable: $rust_bin"
  exit 2
fi
if [[ ! -f "$input" ]]; then
  print -u2 -- "input file not found: $input"
  exit 2
fi
if [[ "$cpp_out" == "$rust_out" ]]; then
  print -u2 -- "cpp_output and rust_output must differ"
  exit 2
fi

if [[ -z "$timeout_bin" ]]; then
  for candidate in /opt/homebrew/bin/timeout timeout; do
    if command -v "$candidate" >/dev/null 2>&1; then
      timeout_bin="$candidate"
      break
    fi
  done
fi
if [[ -z "$timeout_bin" ]]; then
  print -u2 -- "GNU timeout is required"
  exit 2
fi
for tool in env perl gzip cmp; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    print -u2 -- "required tool not found: $tool"
    exit 2
  fi
done

for dir in "$cpp_out" "$rust_out"; do
  if [[ -e "$dir" ]]; then
    entries="$(find "$dir" -mindepth 1 -print -quit 2>/dev/null)"
    if [[ -n "$entries" ]]; then
      print -u2 -- "output directory must be empty or not exist: $dir"
      exit 2
    fi
  fi
done
mkdir -p "$cpp_out" "$rust_out"

work_directory="$(mktemp -d "${TMPDIR:-/tmp}/ctb-cpp-rust.XXXXXX")"
cleanup() { rm -rf -- "$work_directory"; }
trap cleanup EXIT INT TERM

now_seconds() {
  perl -MTime::HiRes -e 'print Time::HiRes::time'
}

cpp_start="$(now_seconds)"
cpp_status=0
cpp_env_args=()
if [[ -n "$cpp_library_path" ]]; then
  cpp_env_args+=(DYLD_LIBRARY_PATH="$cpp_library_path")
fi
if [[ -n "$cpp_gdal_data" ]]; then
  cpp_env_args+=(GDAL_DATA="$cpp_gdal_data")
fi
env "${cpp_env_args[@]}" "$cpp_bin" "${ctb_args[@]}" -o "$cpp_out" "$input" \
    >"$work_directory/cpp.stdout" 2>"$work_directory/cpp.stderr" || cpp_status=$?
if (( cpp_status != 0 )); then
  print -u2 -- "C++ failed with status $cpp_status"
  cat "$work_directory/cpp.stderr" >&2 || true
  exit "$cpp_status"
fi
cpp_end="$(now_seconds)"
cpp_seconds="$(LC_ALL=C awk -v s="$cpp_start" -v e="$cpp_end" 'BEGIN { printf "%.3f", e - s }')"
rust_timeout="$(LC_ALL=C awk -v c="$cpp_seconds" 'BEGIN { printf "%.3f", c * 2 }')"

print -- "cpp_seconds=$cpp_seconds"
print -- "rust_timeout_seconds=$rust_timeout"

rust_start="$(now_seconds)"
rust_status=0
"$timeout_bin" "$rust_timeout" "$rust_bin" "${ctb_args[@]}" -o "$rust_out" "$input" \
    >"$work_directory/rust.stdout" 2>"$work_directory/rust.stderr" || rust_status=$?
if (( rust_status != 0 )); then
  if [[ $rust_status -eq 124 ]]; then
    rust_timed_out=true
  else
    print -u2 -- "Rust failed with status $rust_status"
    cat "$work_directory/rust.stderr" >&2 || true
    exit "$rust_status"
  fi
else
  rust_timed_out=false
fi
rust_end="$(now_seconds)"
rust_seconds="$(LC_ALL=C awk -v s="$rust_start" -v e="$rust_end" 'BEGIN { printf "%.3f", e - s }')"

print -- "rust_seconds=$rust_seconds"
print -- "rust_status=$rust_status"
print -- "rust_timed_out=$rust_timed_out"

cpp_terrain=()
while IFS= read -r -d '' file_path; do
  cpp_terrain+=("${file_path#$cpp_out/}")
done < <(find "$cpp_out" -name '*.terrain' -type f -print0 | sort -z)

rust_terrain=()
while IFS= read -r -d '' file_path; do
  rust_terrain+=("${file_path#$rust_out/}")
done < <(find "$rust_out" -name '*.terrain' -type f -print0 | sort -z)

if (( ${#cpp_terrain[@]} == 0 )); then
  print -u2 -- "C++ produced no .terrain files"
  exit 2
fi

common_terrain=()
for rel in "${cpp_terrain[@]}"; do
  if [[ -f "$rust_out/$rel" ]]; then
    common_terrain+=("$rel")
  fi
done

diff_count=0
for rel in "${common_terrain[@]}"; do
  if ! cmp -s <(gzip -dc "$cpp_out/$rel") <(gzip -dc "$rust_out/$rel"); then
    print -- "payload-diff $rel"
    diff_count=$((diff_count + 1))
  fi
done

print -- "cpp_terrain_count=${#cpp_terrain[@]}"
print -- "rust_terrain_count=${#rust_terrain[@]}"
print -- "common_terrain_count=${#common_terrain[@]}"
print -- "payload_diff_count=$diff_count"

if (( diff_count > 0 )); then
  print -u2 -- "payload differences found"
  exit 1
fi
