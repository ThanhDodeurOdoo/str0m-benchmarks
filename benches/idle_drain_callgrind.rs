//! deterministic Callgrind benchmark for idle RTP-mode `Rtc::poll_output`
//!
//! the fixture builds already-created RTP-mode sessions outside the measured
//! function so the reported instruction count focuses on the idle drain path

#![cfg(feature = "idle-drain")]
#![allow(
    clippy::exit,
    reason = "Gungraun's generated benchmark harness exits with the measured runner status"
)]
#![allow(
    non_local_definitions,
    reason = "Gungraun generates benchmark registration code outside the local item scope"
)]

use std::env;
use std::hint::black_box;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Once;
use std::time::Instant;

use gungraun::{
    Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, library_benchmark,
    library_benchmark_group, main,
};
use str0m::bwe::Bitrate as Str0mBitrate;
use str0m::{Candidate, Event, Output, Rtc};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_IDLE_DRAIN_SESSIONS: usize = 128;
const DEFAULT_IDLE_DRAIN_ROUNDS: usize = 1;
const INITIAL_BWE_BPS: u64 = 2_500_000;

struct IdleDrainFixture {
    sessions: Vec<Rtc>,
    rounds: usize,
}

impl IdleDrainFixture {
    fn new(session_count: usize, rounds: usize) -> Self {
        install_default_crypto_provider();

        let now = Instant::now();
        let mut sessions = Vec::with_capacity(session_count);
        for idx in 0..session_count {
            sessions.push(build_idle_session(now, idx));
        }

        Self { sessions, rounds }
    }

    fn drain(&mut self) -> usize {
        let mut outputs = 0;
        for _ in 0..self.rounds {
            for rtc in &mut self.sessions {
                loop {
                    outputs += 1;
                    match rtc.poll_output().expect("poll idle output") {
                        Output::Transmit(_) => {}
                        Output::Event(Event::RtpPacket(_)) => {}
                        Output::Event(_) => {}
                        Output::Timeout(_) => break,
                    }
                }
            }
        }
        outputs
    }
}

fn build_idle_session(now: Instant, idx: usize) -> Rtc {
    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_opus(true)
        .enable_pcmu(true)
        .enable_pcma(true)
        .enable_bwe(Some(Str0mBitrate::bps(INITIAL_BWE_BPS)))
        .set_rtp_mode(true)
        .set_ice_lite(true)
        .build(now);
    let candidate = Candidate::host(socket_addr(idx), "udp").expect("host candidate");
    rtc.add_local_candidate(candidate).expect("local candidate");
    rtc
}

fn socket_addr(idx: usize) -> SocketAddr {
    let third = ((idx / 250) % 250 + 1) as u8;
    let fourth = (idx % 250 + 1) as u8;
    let port = 10_000 + (idx % 50_000) as u16;
    (Ipv4Addr::new(10, 0, third, fourth), port).into()
}

fn install_default_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        str0m::crypto::from_feature_flags().install_process_default();
    });
}

fn configured_sessions() -> usize {
    env::var("IDLE_DRAIN_SESSIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_IDLE_DRAIN_SESSIONS)
}

fn configured_rounds() -> usize {
    env::var("IDLE_DRAIN_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_IDLE_DRAIN_ROUNDS)
}

fn one_idle_session() -> IdleDrainFixture {
    IdleDrainFixture::new(1, configured_rounds())
}

fn configured_idle_sessions() -> IdleDrainFixture {
    IdleDrainFixture::new(configured_sessions(), configured_rounds())
}

fn callgrind_config() -> LibraryBenchmarkConfig {
    let mut callgrind = Callgrind::default();
    callgrind.fail_fast(false);
    callgrind.format([CallgrindMetrics::All]);

    let mut config = LibraryBenchmarkConfig::default();
    config.tool(callgrind);
    config.pass_through_envs(["IDLE_DRAIN_SESSIONS", "IDLE_DRAIN_ROUNDS"]);
    config
}

#[library_benchmark(config = callgrind_config())]
#[bench::one_session(setup = one_idle_session)]
#[bench::configured_sessions(setup = configured_idle_sessions)]
fn idle_poll_sessions(mut fixture: IdleDrainFixture) -> usize {
    black_box(fixture.drain())
}

library_benchmark_group!(
    name = idle_drain_callgrind;
    benchmarks = idle_poll_sessions
);

main!(library_benchmark_groups = idle_drain_callgrind);
