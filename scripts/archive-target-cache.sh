#!/usr/bin/env bash
set -euo pipefail

mode="${1:-archive}"
crate_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
archive_dir="${GRAPH_DISCOVERY_BUILD_CACHE_DIR:-$crate_dir/../Runs/rust/build-cache}"
archive_path="$archive_dir/rust-target-cache.tar.zst"

case "$mode" in
  archive)
    if [[ ! -d "$crate_dir/target" ]]; then
      echo "no target directory to archive: $crate_dir/target" >&2
      exit 1
    fi
    mkdir -p "$archive_dir"
    tar --zstd -cf "$archive_path.tmp" -C "$crate_dir" target
    mv "$archive_path.tmp" "$archive_path"
    echo "archived target cache to $archive_path"
    ;;
  restore)
    if [[ ! -f "$archive_path" ]]; then
      echo "no target cache archive found: $archive_path" >&2
      exit 1
    fi
    tar --zstd -xf "$archive_path" -C "$crate_dir"
    echo "restored target cache from $archive_path"
    ;;
  path)
    echo "$archive_path"
    ;;
  *)
    echo "usage: $0 [archive|restore|path]" >&2
    exit 2
    ;;
esac
