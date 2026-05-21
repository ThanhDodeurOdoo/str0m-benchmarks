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
use str0m_benchmarks::{
    BenchPacketShared, BenchPacketVec, FullRelaySharedHarness, FullRelayVecHarness,
    ReceiveRtpSharedHarness, ReceiveRtpVecHarness, benchmark_targets, configured_fanout,
};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_CALLGRIND_ROUNDS: usize = 128;

struct ReceiveFanoutVecFixture {
    harness: ReceiveRtpVecHarness,
    targets: Vec<str0m::rtp::Ssrc>,
    out: Vec<BenchPacketVec>,
}

struct ReceiveFanoutSharedFixture {
    harness: ReceiveRtpSharedHarness,
    targets: Vec<str0m::rtp::Ssrc>,
    out: Vec<BenchPacketShared>,
}

struct FullRelayVecFixture {
    harness: FullRelayVecHarness,
}

struct FullRelaySharedFixture {
    harness: FullRelaySharedHarness,
}

impl ReceiveFanoutVecFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: ReceiveRtpVecHarness::new(payload_size, callgrind_rounds()),
            targets: benchmark_targets(fanout),
            out: Vec::with_capacity(fanout),
        }
    }

    fn fanout(&mut self) -> usize {
        self.harness.fanout_vec(&self.targets, &mut self.out)
    }
}

impl ReceiveFanoutSharedFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: ReceiveRtpSharedHarness::new(payload_size, callgrind_rounds()),
            targets: benchmark_targets(fanout),
            out: Vec::with_capacity(fanout),
        }
    }

    fn fanout(&mut self) -> usize {
        self.harness.fanout_shared(&self.targets, &mut self.out)
    }
}

impl FullRelayVecFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: FullRelayVecHarness::new(fanout, payload_size, callgrind_rounds()),
        }
    }

    fn relay(&mut self) -> usize {
        self.harness.relay_vec()
    }
}

impl FullRelaySharedFixture {
    fn new(fanout: usize, payload_size: usize) -> Self {
        Self {
            harness: FullRelaySharedHarness::new(fanout, payload_size, callgrind_rounds()),
        }
    }

    fn relay(&mut self) -> usize {
        self.harness.relay_shared()
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

macro_rules! receive_vec_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> ReceiveFanoutVecFixture {
            ReceiveFanoutVecFixture::new($fanout, $payload_size)
        }
    };
}

macro_rules! receive_shared_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> ReceiveFanoutSharedFixture {
            ReceiveFanoutSharedFixture::new($fanout, $payload_size)
        }
    };
}

macro_rules! full_relay_vec_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> FullRelayVecFixture {
            FullRelayVecFixture::new($fanout, $payload_size)
        }
    };
}

macro_rules! full_relay_shared_setup {
    ($name:ident, $fanout:expr, $payload_size:expr) => {
        fn $name() -> FullRelaySharedFixture {
            FullRelaySharedFixture::new($fanout, $payload_size)
        }
    };
}

receive_vec_setup!(receive_vec_160b_1user, 1, 160);
receive_vec_setup!(receive_vec_1350b_1user, 1, 1350);
receive_shared_setup!(receive_shared_160b_1user, 1, 160);
receive_shared_setup!(receive_shared_1350b_1user, 1, 1350);

full_relay_vec_setup!(full_relay_vec_160b_1user, 1, 160);
full_relay_vec_setup!(full_relay_vec_1350b_1user, 1, 1350);
full_relay_shared_setup!(full_relay_shared_160b_1user, 1, 160);
full_relay_shared_setup!(full_relay_shared_1350b_1user, 1, 1350);

fn receive_vec_160b_configured_users() -> ReceiveFanoutVecFixture {
    ReceiveFanoutVecFixture::new(configured_fanout(), 160)
}

fn receive_vec_1350b_configured_users() -> ReceiveFanoutVecFixture {
    ReceiveFanoutVecFixture::new(configured_fanout(), 1350)
}

fn receive_shared_160b_configured_users() -> ReceiveFanoutSharedFixture {
    ReceiveFanoutSharedFixture::new(configured_fanout(), 160)
}

fn receive_shared_1350b_configured_users() -> ReceiveFanoutSharedFixture {
    ReceiveFanoutSharedFixture::new(configured_fanout(), 1350)
}

fn full_relay_vec_160b_configured_users() -> FullRelayVecFixture {
    FullRelayVecFixture::new(configured_fanout(), 160)
}

fn full_relay_vec_1350b_configured_users() -> FullRelayVecFixture {
    FullRelayVecFixture::new(configured_fanout(), 1350)
}

fn full_relay_shared_160b_configured_users() -> FullRelaySharedFixture {
    FullRelaySharedFixture::new(configured_fanout(), 160)
}

fn full_relay_shared_1350b_configured_users() -> FullRelaySharedFixture {
    FullRelaySharedFixture::new(configured_fanout(), 1350)
}

#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = receive_vec_160b_1user)]
#[bench::p160b_configured_users(setup = receive_vec_160b_configured_users)]
#[bench::p1350b_1user(setup = receive_vec_1350b_1user)]
#[bench::p1350b_configured_users(setup = receive_vec_1350b_configured_users)]
fn rtp_event_fanout_base_vec(mut fixture: ReceiveFanoutVecFixture) -> usize {
    black_box(fixture.fanout())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = receive_shared_160b_1user)]
#[bench::p160b_configured_users(setup = receive_shared_160b_configured_users)]
#[bench::p1350b_1user(setup = receive_shared_1350b_1user)]
#[bench::p1350b_configured_users(setup = receive_shared_1350b_configured_users)]
fn rtp_event_fanout_arc_meta(mut fixture: ReceiveFanoutSharedFixture) -> usize {
    black_box(fixture.fanout())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = full_relay_vec_160b_1user)]
#[bench::p160b_configured_users(setup = full_relay_vec_160b_configured_users)]
#[bench::p1350b_1user(setup = full_relay_vec_1350b_1user)]
#[bench::p1350b_configured_users(setup = full_relay_vec_1350b_configured_users)]
fn full_relay_base_vec(mut fixture: FullRelayVecFixture) -> usize {
    black_box(fixture.relay())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p160b_1user(setup = full_relay_shared_160b_1user)]
#[bench::p160b_configured_users(setup = full_relay_shared_160b_configured_users)]
#[bench::p1350b_1user(setup = full_relay_shared_1350b_1user)]
#[bench::p1350b_configured_users(setup = full_relay_shared_1350b_configured_users)]
fn full_relay_arc_meta(mut fixture: FullRelaySharedFixture) -> usize {
    black_box(fixture.relay())
}

library_benchmark_group!(
    name = relay_callgrind;
    benchmarks = rtp_event_fanout_base_vec, rtp_event_fanout_arc_meta, full_relay_base_vec,
        full_relay_arc_meta
);

main!(library_benchmark_groups = relay_callgrind);
