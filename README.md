# str0m-benchmarks

this crate benchmarks str0m's default `Vec<u8>` payload storage against a
custom `Meta` implementation that uses `Arc<[u8]>` for both RTP packets and
media frames

the default `Cargo.toml` points at the pushed `meta-byteref-2` str0m branch so
github runners can fetch it without a local sibling checkout

useful commands:

```bash
cargo bench --bench fanout -- --noplot --quiet
cargo run --release --bin memory
cargo bench --bench full_relay_callgrind
```

the helper scripts copy the benchmark crate to a result directory and rewrite
the str0m dependency to a local path for that run:

```bash
scripts/run-stack-bench.sh ../str0m.worktrees/meta-byteref logs
scripts/run-stack-memory.sh ../str0m.worktrees/meta-byteref logs
scripts/run-stack-callgrind.sh ../str0m.worktrees/meta-byteref logs
CALLGRIND_DIR=logs/meta-callgrind-target/iai node scripts/summarize_full_relay.mjs
```
