#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
    printf 'usage: %s <str0m-vp8-rewrite-branch> <result-dir>\n' "$0" >&2
    exit 2
fi

bench_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
str0m_path=$(cd "$1" && pwd)
mkdir -p "$2"
result_root=$(cd "$2" && pwd)
src_dir="$result_root/vp8-rewrite-callgrind-src"
target_dir="$result_root/vp8-rewrite-callgrind-target"

rm -rf "$src_dir" "$target_dir"
rsync -a --exclude target --exclude logs --exclude .git "$bench_root/" "$src_dir/"
perl -0pi -e "s#^str0m = \\{[^\\n]*\\}#str0m = { path = \"$str0m_path\", default-features = false, features = [\"aws-lc-rs\"] }#m" "$src_dir/Cargo.toml"

printf '== vp8 rewrite callgrind ==\n'
CARGO_TARGET_DIR="$target_dir" cargo bench \
    --manifest-path "$src_dir/Cargo.toml" \
    --no-default-features \
    --features vp8-rewrite,jemalloc \
    --bench vp8_rewrite_callgrind | tee "$result_root/vp8-rewrite-callgrind.log"
