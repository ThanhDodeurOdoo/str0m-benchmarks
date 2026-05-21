//! deterministic Callgrind benchmarks for str0m relay fanout
//!
//! the fixtures build encrypted inputs and connected direct RTP senders outside
//! the measured functions so the reported instruction count covers packet relay
//! work rather than setup

#![allow(
    clippy::exit,
    reason = "iai-callgrind's generated benchmark harness exits with the measured runner status"
)]
#![allow(
    non_local_definitions,
    reason = "iai-callgrind generates benchmark registration code outside the local item scope"
)]

use std::env;
use std::hint::black_box;

use iai_callgrind::{
    Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main,
};
use str0m_benchmarks::{FullRelayHarness, ReceiveRtpHarness, benchmark_targets, configured_fanout};

#[cfg(feature = "arc-payload")]
use str0m_benchmarks::BenchPacketShared;
#[cfg(not(feature = "arc-payload"))]
use str0m_benchmarks::BenchPacketVec;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_CALLGRIND_ROUNDS: usize = 128;

struct ReceiveFanoutFixture {
    harness: ReceiveRtpHarness,
    targets: Vec<str0m::rtp::Ssrc>,
    #[cfg(not(feature = "arc-payload"))]
    out_vec: Vec<BenchPacketVec>,
    #[cfg(feature = "arc-payload")]
    out_shared: Vec<BenchPacketShared>,
}

struct FullRelayFixture {
    harness: FullRelayHarness,
}

impl ReceiveFanoutFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: ReceiveRtpHarness::new(payload_size, callgrind_rounds()),
            targets: benchmark_targets(fanout),
            #[cfg(not(feature = "arc-payload"))]
            out_vec: Vec::with_capacity(fanout),
            #[cfg(feature = "arc-payload")]
            out_shared: Vec::with_capacity(fanout),
        }
    }

    #[cfg(not(feature = "arc-payload"))]
    fn fanout_vec(&mut self) -> usize {
        self.harness.fanout_vec(&self.targets, &mut self.out_vec)
    }

    #[cfg(feature = "arc-payload")]
    fn fanout_shared(&mut self) -> usize {
        self.harness
            .fanout_shared(&self.targets, &mut self.out_shared)
    }
}

impl FullRelayFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: FullRelayHarness::new(fanout, payload_size, callgrind_rounds()),
        }
    }

    #[cfg(feature = "arc-payload")]
    fn relay_shared(&mut self) -> usize {
        self.harness.relay_shared()
    }

    #[cfg(not(feature = "arc-payload"))]
    fn relay_vec(&mut self) -> usize {
        self.harness.relay_vec()
    }
}

fn callgrind_rounds() -> usize {
    env::var("FULL_RELAY_CALLGRIND_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|rounds| *rounds > 0)
        .unwrap_or(DEFAULT_CALLGRIND_ROUNDS)
}

fn callgrind_config() -> LibraryBenchmarkConfig {
    let mut callgrind = Callgrind::default();
    callgrind.fail_fast(false);

    let mut config = LibraryBenchmarkConfig::default();
    config.tool(callgrind);
    config.pass_through_envs(["FANOUT_USERS", "FULL_RELAY_CALLGRIND_ROUNDS"]);
    config
}

macro_rules! receive_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> ReceiveFanoutFixture {
            ReceiveFanoutFixture::new($fanout, $payload_size)
        }
    };
}

macro_rules! full_relay_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> FullRelayFixture {
            FullRelayFixture::new($fanout, $payload_size)
        }
    };
}

receive_setup!(receive_160b_1user, 1, 160);
receive_setup!(receive_1350b_1user, 1, 1350);

full_relay_setup!(full_relay_160b_1user, 1, 160);
full_relay_setup!(full_relay_1350b_1user, 1, 1350);

fn receive_160b_configured_users() -> ReceiveFanoutFixture {
    ReceiveFanoutFixture::new(configured_fanout(), 160)
}

fn receive_1350b_configured_users() -> ReceiveFanoutFixture {
    ReceiveFanoutFixture::new(configured_fanout(), 1350)
}

fn full_relay_160b_configured_users() -> FullRelayFixture {
    FullRelayFixture::new(configured_fanout(), 160)
}

fn full_relay_1350b_configured_users() -> FullRelayFixture {
    FullRelayFixture::new(configured_fanout(), 1350)
}

#[cfg(not(feature = "arc-payload"))]
#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = receive_160b_1user)]
#[bench::p160b_configured_users(setup = receive_160b_configured_users)]
#[bench::p1350b_1user(setup = receive_1350b_1user)]
#[bench::p1350b_configured_users(setup = receive_1350b_configured_users)]
fn rtp_event_fanout(mut fixture: ReceiveFanoutFixture) -> usize {
    black_box(fixture.fanout_vec())
}

#[cfg(feature = "arc-payload")]
#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = receive_160b_1user)]
#[bench::p160b_configured_users(setup = receive_160b_configured_users)]
#[bench::p1350b_1user(setup = receive_1350b_1user)]
#[bench::p1350b_configured_users(setup = receive_1350b_configured_users)]
fn rtp_event_fanout(mut fixture: ReceiveFanoutFixture) -> usize {
    black_box(fixture.fanout_shared())
}

#[cfg(not(feature = "arc-payload"))]
#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = full_relay_160b_1user)]
#[bench::p160b_configured_users(setup = full_relay_160b_configured_users)]
#[bench::p1350b_1user(setup = full_relay_1350b_1user)]
#[bench::p1350b_configured_users(setup = full_relay_1350b_configured_users)]
fn full_relay(mut fixture: FullRelayFixture) -> usize {
    black_box(fixture.relay_vec())
}

#[cfg(feature = "arc-payload")]
#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = full_relay_160b_1user)]
#[bench::p160b_configured_users(setup = full_relay_160b_configured_users)]
#[bench::p1350b_1user(setup = full_relay_1350b_1user)]
#[bench::p1350b_configured_users(setup = full_relay_1350b_configured_users)]
fn full_relay(mut fixture: FullRelayFixture) -> usize {
    black_box(fixture.relay_shared())
}

library_benchmark_group!(
    name = relay_callgrind;
    benchmarks = rtp_event_fanout, full_relay
);

main!(library_benchmark_groups = relay_callgrind);
