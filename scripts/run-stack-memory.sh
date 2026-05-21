#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
    printf 'usage: %s <str0m-meta-branch> <result-dir>\n' "$0" >&2
    exit 2
fi

bench_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
str0m_path=$(cd "$1" && pwd)
result_root=$2
src_dir="$result_root/meta-memory-src"
target_dir="$result_root/meta-memory-target"

mkdir -p "$result_root"
rm -rf "$src_dir" "$target_dir"
rsync -a --exclude target --exclude .git "$bench_root/" "$src_dir/"
perl -0pi -e "s#^str0m = \\{[^\\n]*\\}#str0m = { path = \"$str0m_path\", default-features = false, features = [\"aws-lc-rs\"] }#m" "$src_dir/Cargo.toml"

printf '== memory ==\n' | tee "$result_root/meta-memory.log"
CARGO_TARGET_DIR="$target_dir" cargo run \
    --release \
    --manifest-path "$src_dir/Cargo.toml" \
    --bin memory | tee -a "$result_root/meta-memory.log"
