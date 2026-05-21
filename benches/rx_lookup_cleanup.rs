//! Criterion benchmark for the receive lookup cleanup cadence.
//!
//! Run the same benchmark against an unfixed str0m checkout and the fix worktree.

#![cfg(feature = "rx-lookup-cleanup")]

use std::env;
use std::hint::black_box;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Once;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use str0m::format::Codec;
use str0m::media::{MediaKind, Pt};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{ExtensionValues, RtpWrite, Ssrc};
use str0m::{Candidate, Event, Input, Output, Rtc};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_LOOKUP_STREAMS: usize = 512;
const DEFAULT_MEASURED_PACKETS: usize = 256;
const RTP_TIMESTAMP_STEP: u32 = 960;

struct RxLookupCleanupFixture {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    now: Instant,
}

struct EgressSender {
    rtc: Rtc,
    ssrcs: Vec<Ssrc>,
    now: Instant,
    payload_type: Pt,
    ext_vals: ExtensionValues,
    seq_no: u64,
    timestamp: u32,
}

struct ConnectedPair {
    left: Rtc,
    right: Rtc,
    now: Instant,
    left_addr: SocketAddr,
    right_addr: SocketAddr,
}

struct PendingDatagram {
    source: SocketAddr,
    destination: SocketAddr,
    contents: Vec<u8>,
}

impl RxLookupCleanupFixture {
    fn new(stream_count: usize, measured_packets: usize) -> Self {
        install_default_crypto_provider();

        let now = Instant::now();
        let left = Rtc::builder().set_rtp_mode(true).build(now);
        let right = Rtc::builder().set_rtp_mode(true).build(now);
        let ConnectedPair {
            mut left,
            mut right,
            mut now,
            left_addr,
            right_addr,
        } = connect_pair(now, left, right);

        let payload_type = left
            .codec_config()
            .find(|params| params.spec().codec == Codec::Opus)
            .expect("Opus codec")
            .pt();
        let ssrcs = declare_streams(&mut left, &mut right, stream_count);
        let mut sender = EgressSender {
            rtc: left,
            ssrcs,
            now,
            payload_type,
            ext_vals: ExtensionValues {
                audio_level: Some(-9),
                voice_activity: Some(true),
                ..Default::default()
            },
            seq_no: 1,
            timestamp: 48_000,
        };

        let seed_datagrams = build_datagrams(&mut sender, stream_count, left_addr, right_addr);
        let datagrams = build_datagrams(&mut sender, measured_packets, left_addr, right_addr);

        for packet in &seed_datagrams {
            deliver_one(&mut right, now, packet);
            drain_receiver_events(&mut right);
            now += Duration::from_millis(1);
        }

        Self {
            receiver: right,
            datagrams,
            now,
        }
    }

    fn receive(&mut self) -> usize {
        let mut event_count = 0;
        for packet in &self.datagrams {
            deliver_one(&mut self.receiver, self.now, packet);
            event_count += drain_receiver_events(&mut self.receiver);
            self.now += Duration::from_millis(1);
        }
        event_count
    }
}

fn declare_streams(sender: &mut Rtc, receiver: &mut Rtc, stream_count: usize) -> Vec<Ssrc> {
    let mut ssrcs = Vec::with_capacity(stream_count);
    for idx in 0..stream_count {
        let mid = format!("m{idx:04}");
        let mid = mid.as_str().into();
        let ssrc = (10_000 + idx as u32).into();

        sender.direct_api().declare_media(mid, MediaKind::Audio);
        sender.direct_api().declare_stream_tx(ssrc, None, mid, None);
        receiver.direct_api().declare_media(mid, MediaKind::Audio);
        receiver
            .direct_api()
            .expect_stream_rx(ssrc, None, mid, None);
        ssrcs.push(ssrc);
    }
    ssrcs
}

fn build_datagrams(
    sender: &mut EgressSender,
    count: usize,
    source: SocketAddr,
    destination: SocketAddr,
) -> Vec<PendingDatagram> {
    let mut datagrams = Vec::with_capacity(count);
    for idx in 0..count {
        let ssrc = sender.ssrcs[idx % sender.ssrcs.len()];
        write_sender_packet(sender, ssrc);
        sender
            .rtc
            .handle_input(Input::Timeout(sender.now))
            .expect("sender timeout");
        drain_transmit_datagrams(&mut sender.rtc, source, destination, &mut datagrams);
        sender.now += Duration::from_millis(20);
    }
    datagrams
}

fn write_sender_packet(sender: &mut EgressSender, ssrc: Ssrc) {
    let mut direct = sender.rtc.direct_api();
    let stream = direct.stream_tx(&ssrc).expect("declared stream");
    stream.write_rtp(
        RtpWrite::new(
            sender.payload_type,
            sender.seq_no.into(),
            sender.timestamp,
            sender.now,
            vec![0_u8; 160],
        )
        .ext_vals(sender.ext_vals.clone()),
    );
    sender.seq_no += 1;
    sender.timestamp = sender.timestamp.wrapping_add(RTP_TIMESTAMP_STEP);
}

fn deliver_one(receiver: &mut Rtc, now: Instant, packet: &PendingDatagram) {
    receiver
        .handle_input(Input::Receive(now, receive_from_datagram(packet)))
        .expect("receive packet");
}

fn receive_from_datagram(packet: &PendingDatagram) -> Receive<'_> {
    Receive {
        proto: Protocol::Udp,
        source: packet.source,
        destination: packet.destination,
        contents: packet
            .contents
            .as_slice()
            .try_into()
            .expect("datagram receive"),
    }
}

fn drain_receiver_events(rtc: &mut Rtc) -> usize {
    let mut event_count = 0;
    loop {
        match rtc.poll_output().expect("poll receiver output") {
            Output::Transmit(_) => {}
            Output::Event(Event::RtpPacket(_)) => event_count += 1,
            Output::Event(_) => {}
            Output::Timeout(_) => return event_count,
        }
    }
}

fn drain_transmit_datagrams(
    rtc: &mut Rtc,
    source: SocketAddr,
    destination: SocketAddr,
    out: &mut Vec<PendingDatagram>,
) -> usize {
    let mut transmit_count = 0;
    loop {
        match rtc.poll_output().expect("poll sender output") {
            Output::Transmit(transmit) => {
                assert_eq!(transmit.source, source);
                assert_eq!(transmit.destination, destination);
                transmit_count += 1;
                out.push(PendingDatagram {
                    source: transmit.source,
                    destination: transmit.destination,
                    contents: transmit.contents.to_vec(),
                });
            }
            Output::Event(_) => {}
            Output::Timeout(_) => return transmit_count,
        }
    }
}

fn install_default_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        str0m::crypto::from_feature_flags().install_process_default();
    });
}

fn connect_pair(now: Instant, left: Rtc, right: Rtc) -> ConnectedPair {
    let left_addr = socket_addr(1, 1, 1, 1, 10_000);
    let right_addr = socket_addr(2, 2, 2, 2, 20_000);
    let mut pair = ConnectedPair {
        left,
        right,
        now,
        left_addr,
        right_addr,
    };

    let left_candidate = Candidate::host(left_addr, "udp").expect("left candidate");
    let right_candidate = Candidate::host(right_addr, "udp").expect("right candidate");
    pair.left
        .add_local_candidate(left_candidate.clone())
        .expect("left local candidate");
    pair.left.add_remote_candidate(right_candidate.clone());
    pair.right
        .add_local_candidate(right_candidate)
        .expect("right local candidate");
    pair.right.add_remote_candidate(left_candidate);

    let left_fingerprint = pair.left.direct_api().local_dtls_fingerprint().clone();
    let right_fingerprint = pair.right.direct_api().local_dtls_fingerprint().clone();
    pair.left
        .direct_api()
        .set_remote_fingerprint(right_fingerprint);
    pair.right
        .direct_api()
        .set_remote_fingerprint(left_fingerprint);

    let left_credentials = pair.left.direct_api().local_ice_credentials();
    let right_credentials = pair.right.direct_api().local_ice_credentials();
    pair.left
        .direct_api()
        .set_remote_ice_credentials(right_credentials);
    pair.right
        .direct_api()
        .set_remote_ice_credentials(left_credentials);
    pair.left.direct_api().set_ice_controlling(true);
    pair.right.direct_api().set_ice_controlling(false);
    pair.left
        .direct_api()
        .start_dtls(true)
        .expect("left DTLS start");
    pair.right
        .direct_api()
        .start_dtls(false)
        .expect("right DTLS start");

    connect_rtc_pair(&mut pair);
    pair
}

fn connect_rtc_pair(pair: &mut ConnectedPair) {
    let mut left_queue = Vec::new();
    let mut right_queue = Vec::new();
    for _ in 0..10_000 {
        drain_pair_side(
            &mut pair.left,
            pair.left_addr,
            pair.right_addr,
            &mut right_queue,
        );
        drain_pair_side(
            &mut pair.right,
            pair.right_addr,
            pair.left_addr,
            &mut left_queue,
        );
        deliver_packets(&mut pair.left, pair.now, &mut left_queue);
        deliver_packets(&mut pair.right, pair.now, &mut right_queue);

        if pair.left.is_connected() && pair.right.is_connected() {
            return;
        }

        pair.left
            .handle_input(Input::Timeout(pair.now))
            .expect("left timeout");
        pair.right
            .handle_input(Input::Timeout(pair.now))
            .expect("right timeout");
        pair.now += Duration::from_millis(10);
    }
    panic!("connected RTP-mode pair did not complete ICE/DTLS setup");
}

fn drain_pair_side(
    rtc: &mut Rtc,
    source: SocketAddr,
    destination: SocketAddr,
    peer_queue: &mut Vec<PendingDatagram>,
) {
    loop {
        match rtc.poll_output().expect("poll pair output") {
            Output::Transmit(transmit) => {
                assert_eq!(transmit.source, source);
                assert_eq!(transmit.destination, destination);
                peer_queue.push(PendingDatagram {
                    source: transmit.source,
                    destination: transmit.destination,
                    contents: transmit.contents.to_vec(),
                });
            }
            Output::Event(_) => {}
            Output::Timeout(_) => return,
        }
    }
}

fn deliver_packets(rtc: &mut Rtc, now: Instant, queue: &mut Vec<PendingDatagram>) {
    for packet in queue.drain(..) {
        rtc.handle_input(Input::Receive(now, receive_from_datagram(&packet)))
            .expect("deliver pair packet");
    }
}

fn socket_addr(a: u8, b: u8, c: u8, d: u8, port: u16) -> SocketAddr {
    (Ipv4Addr::new(a, b, c, d), port).into()
}

fn configured_lookup_streams() -> usize {
    env::var("RX_LOOKUP_STREAMS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_LOOKUP_STREAMS)
}

fn configured_measured_packets() -> usize {
    env::var("RX_LOOKUP_PACKETS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MEASURED_PACKETS)
}

fn lookup_stream_counts() -> Vec<usize> {
    let configured = configured_lookup_streams();
    if configured == 1 {
        vec![1]
    } else {
        vec![1, configured]
    }
}

fn bench_rx_lookup_cleanup(c: &mut Criterion) {
    let measured_packets = configured_measured_packets();
    let mut group = c.benchmark_group("rx_lookup_cleanup_receive");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(2));
    group.throughput(Throughput::Elements(measured_packets as u64));

    for stream_count in lookup_stream_counts() {
        group.bench_with_input(
            BenchmarkId::new(
                "rtp_mode",
                format!("{stream_count}streams-{measured_packets}packets"),
            ),
            &stream_count,
            |b, stream_count| {
                b.iter_batched(
                    || RxLookupCleanupFixture::new(*stream_count, measured_packets),
                    |mut fixture| black_box(fixture.receive()),
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_rx_lookup_cleanup);
criterion_main!(benches);
