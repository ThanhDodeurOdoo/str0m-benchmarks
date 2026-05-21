#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
    printf 'usage: %s <str0m-before> <str0m-after> <result-dir>\n' "$0" >&2
    exit 2
fi

bench_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
before_path=$(cd "$1" && pwd)
after_path=$(cd "$2" && pwd)
mkdir -p "$3"
result_root=$(cd "$3" && pwd)

run_case() {
    local label=$1
    local str0m_path=$2
    local src_dir="$result_root/$label-src"
    local target_dir="$result_root/$label-target"

    rm -rf "$src_dir" "$target_dir"
    rsync -a --exclude target --exclude logs --exclude .git "$bench_root/" "$src_dir/"
    perl -0pi -e "s#^str0m = \\{[^\\n]*\\}#str0m = { path = \"$str0m_path\", default-features = false, features = [\"aws-lc-rs\"] }#m" "$src_dir/Cargo.toml"

    printf '== %s ==\n' "$label"
    CARGO_TARGET_DIR="$target_dir" cargo bench \
        --manifest-path "$src_dir/Cargo.toml" \
        --no-default-features \
        --features rx-lookup-cleanup,jemalloc \
        --bench rx_lookup_cleanup \
        -- --noplot --quiet | tee "$result_root/$label.log"
}

run_case before "$before_path"
run_case after "$after_path"
