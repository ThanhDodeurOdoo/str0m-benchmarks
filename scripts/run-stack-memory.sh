#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 4 ]; then
    printf 'usage: %s <baseline-str0m> <ownership-str0m> <parts-str0m> <result-dir>\n' "$0" >&2
    exit 2
fi

bench_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
baseline_path=$(cd "$1" && pwd)
ownership_path=$(cd "$2" && pwd)
parts_path=$(cd "$3" && pwd)
result_root=$4

mkdir -p "$result_root"

run_one() {
    local name=$1
    local str0m_path=$2
    local features=${3:-}
    local src_dir="$result_root/$name-memory-src"
    local target_dir="$result_root/$name-memory-target"

    rm -rf "$src_dir" "$target_dir"
    rsync -a --exclude target --exclude .git "$bench_root/" "$src_dir/"
    perl -0pi -e "s#path = \"\\.\\./str0m\"#path = \"$str0m_path\"#" "$src_dir/Cargo.toml"

    printf '== %s ==\n' "$name" | tee "$result_root/$name-memory.log"
    if [ -n "$features" ]; then
        CARGO_TARGET_DIR="$target_dir" cargo run \
            --release \
            --manifest-path "$src_dir/Cargo.toml" \
            --features "$features" \
            --bin memory | tee -a "$result_root/$name-memory.log"
    else
        CARGO_TARGET_DIR="$target_dir" cargo run \
            --release \
            --manifest-path "$src_dir/Cargo.toml" \
            --bin memory | tee -a "$result_root/$name-memory.log"
    fi
}

run_one baseline "$baseline_path"
run_one ownership "$ownership_path" bytes-payload
run_one parts "$parts_path" bytes-payload
