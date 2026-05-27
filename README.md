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
scripts/run-vp8-rewrite-callgrind.sh ../str0m.worktrees/payload-patches logs
CALLGRIND_DIR=logs/meta-callgrind-target/gungraun node scripts/summarize_full_relay.mjs
```

the VP8 rewrite Callgrind benchmark compares two VP8 fanout strategies against
a str0m checkout that supports `RtpWrite::vp8_patch`:

- `vp8_rewrite_copy_then_write_rtp` copies the full VP8 payload per destination
  and rewrites the two-byte PictureID before calling `write_rtp`
- `vp8_rewrite_shared_payload` keeps the shared VP8 payload and requests a
  PictureID rewrite through `Vp8Descriptor` and `RtpWrite::vp8_patch`

it measures both egress-only fanout and full-stack relay fanout from encrypted
inbound RTP through `Event::RtpPacket` to encrypted outbound RTP

this benchmark uses `--no-default-features --features vp8-rewrite,jemalloc`
because the default benchmark harness targets the older meta-type str0m branch

the RX lookup cleanup benchmark compares an unfixed checkout with the fix
worktree:

```bash
scripts/run-rx-lookup-cleanup-bench.sh ../str0m ../str0m.worktrees/rx-lookup-cleanup logs/rx-lookup-cleanup
scripts/run-rx-lookup-cleanup-callgrind.sh ../str0m ../str0m.worktrees/rx-lookup-cleanup logs/rx-lookup-cleanup-callgrind
```

by default it seeds 512 RTP-mode receive streams and measures 256 receives
inside one cleanup interval

use the Criterion runner for wall-time sampling and the Callgrind runner for
deterministic instruction counts

use `RX_LOOKUP_STREAMS` and `RX_LOOKUP_PACKETS` to change those sizes

the `rx lookup cleanup callgrind` GitHub Actions workflow runs the same
Callgrind comparison on `ubuntu-24.04`, installs Valgrind and uploads the raw
Gungraun artifacts

the default dispatch checks out `ThanhDodeurOdoo/str0m@main` twice and applies
the one-line cleanup timestamp fix to the after checkout. set
`apply_cleanup_fix` to false to compare against a real fixed branch instead
