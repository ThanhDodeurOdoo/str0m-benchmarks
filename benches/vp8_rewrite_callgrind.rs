//! deterministic Callgrind benchmarks for VP8 RTP payload rewrites
//!
//! the fixtures model SFU VP8 fanout with direct RTP egress only and with a
//! full inbound decrypt to outbound encrypt packet path

#![cfg(feature = "vp8-rewrite")]
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
use std::sync::Arc;
use std::sync::Once;
use std::time::{Duration, Instant};

use gungraun::{
    Callgrind, CallgrindMetrics, LibraryBenchmarkConfig, library_benchmark,
    library_benchmark_group, main,
};
use str0m::media::{MediaKind, Pt};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{ExtensionValues, RtpWrite, Ssrc, Vp8Descriptor, Vp8Patch};
use str0m::{Candidate, Event, Input, Output, Rtc};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const DEFAULT_CONFIGURED_FANOUT: usize = 30;
const DEFAULT_CALLGRIND_ROUNDS: usize = 128;
const VP8_PAYLOAD_TYPE_VALUE: u8 = 96;
const VP8_PAYLOAD_SIZE: usize = 1_200;
const VP8_PICTURE_ID_OFFSET: usize = 2;
const VP8_PICTURE_ID_MAX: u16 = 0x7fff;
const VP8_PICTURE_ID_SHORT_MASK: u8 = 0x7f;
const VIDEO_TIMESTAMP_STEP: u32 = 3_000;

type SharedPayload = Arc<[u8]>;

struct Vp8EgressFixture {
    senders: Vec<EgressSender>,
    payload: SharedPayload,
    descriptor: Vp8Descriptor,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    rounds: usize,
}

struct Vp8FullStackFixture {
    receiver: Rtc,
    inbound_datagrams: Vec<PendingDatagram>,
    senders: Vec<EgressSender>,
    now: Instant,
    seq_no: u64,
    timestamp: u32,
}

struct EgressSender {
    rtc: Rtc,
    ssrc: Ssrc,
    now: Instant,
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

impl Vp8EgressFixture {
    fn new(fanout: usize) -> Self {
        install_default_crypto_provider();
        let payload: SharedPayload = vp8_payload_vec(VP8_PAYLOAD_SIZE).into();
        let descriptor = Vp8Descriptor::parse(&payload).expect("VP8 payload descriptor");

        Self {
            senders: video_egress_senders(fanout, 0, 10_000, "v"),
            payload,
            descriptor,
            payload_type: Pt::new_with_value(VP8_PAYLOAD_TYPE_VALUE),
            seq_no: 1,
            timestamp: 90_000,
            rounds: callgrind_rounds(),
        }
    }

    fn rewrite_with_copy(&mut self) -> usize {
        self.egress(
            |sender, payload_type, seq_no, timestamp, sender_idx, base_payload| {
                let picture_id = vp8_picture_id(seq_no, sender_idx);
                let rewritten = copied_payload_with_picture_id(base_payload, picture_id);

                write_sender_packet(sender, payload_type, seq_no, timestamp, rewritten);
            },
        )
    }

    fn rewrite_with_vp8_patch(&mut self) -> usize {
        let descriptor = self.descriptor;
        self.egress(
            |sender, payload_type, seq_no, timestamp, sender_idx, base_payload| {
                write_sender_packet_with_vp8_patch(
                    sender,
                    payload_type,
                    seq_no,
                    timestamp,
                    base_payload.clone(),
                    vp8_picture_id_patch(descriptor, vp8_picture_id(seq_no, sender_idx)),
                );
            },
        )
    }

    fn egress(
        &mut self,
        mut write_packet: impl FnMut(&mut EgressSender, Pt, u64, u32, usize, &SharedPayload),
    ) -> usize {
        let mut transmit_count = 0;
        for _ in 0..self.rounds {
            for (sender_idx, sender) in self.senders.iter_mut().enumerate() {
                write_packet(
                    sender,
                    self.payload_type,
                    self.seq_no,
                    self.timestamp,
                    sender_idx,
                    &self.payload,
                );
                sender
                    .rtc
                    .handle_input(Input::Timeout(sender.now))
                    .expect("sender timeout");
                transmit_count += drain_sender_transmits(&mut sender.rtc);
                self.seq_no += 1;
                self.timestamp = self.timestamp.wrapping_add(VIDEO_TIMESTAMP_STEP);
                sender.now += Duration::from_millis(33);
            }
        }

        transmit_count
    }
}

impl Vp8FullStackFixture {
    fn new(fanout: usize) -> Self {
        let inbound = build_inbound_vp8(callgrind_rounds());

        Self {
            receiver: inbound.receiver,
            inbound_datagrams: inbound.datagrams,
            senders: video_egress_senders(fanout, 1, 20_000, "o"),
            now: inbound.now,
            seq_no: 1,
            timestamp: 90_000,
        }
    }

    fn relay_with_copy(&mut self) -> usize {
        self.relay(
            |seq_no, timestamp, sender_idx, _descriptor, sender, packet| {
                let picture_id = vp8_picture_id(seq_no, sender_idx);
                let rewritten = copied_payload_with_picture_id(&packet.payload, picture_id);

                write_sender_packet_with_ext_vals(
                    sender,
                    packet.header.payload_type,
                    seq_no,
                    timestamp,
                    packet.header.ext_vals.clone(),
                    rewritten,
                );
            },
        )
    }

    fn relay_with_vp8_patch(&mut self) -> usize {
        self.relay(
            |seq_no, timestamp, sender_idx, descriptor, sender, packet| {
                write_sender_packet_with_ext_vals_and_vp8_patch(
                    sender,
                    packet.header.payload_type,
                    seq_no,
                    timestamp,
                    packet.header.ext_vals.clone(),
                    packet.payload.clone(),
                    vp8_picture_id_patch(descriptor, vp8_picture_id(seq_no, sender_idx)),
                );
            },
        )
    }

    fn relay(
        &mut self,
        mut write_packet: impl FnMut(
            u64,
            u32,
            usize,
            Vp8Descriptor,
            &mut EgressSender,
            &str0m::rtp::RtpPacket,
        ),
    ) -> usize {
        let mut transmit_count = 0;
        let mut relay_seq_no = self.seq_no;
        let mut relay_timestamp = self.timestamp;

        for datagram in &self.inbound_datagrams {
            let receive = receive_from_datagram(datagram);
            self.receiver
                .handle_input(Input::Receive(self.now, receive))
                .expect("receive inbound RTP packet");

            let mut rtp_packets = 0;
            drain_receiver_events(&mut self.receiver, &mut |event| {
                if let Event::RtpPacket(packet) = event {
                    let descriptor = Vp8Descriptor::parse(&packet.payload)
                        .expect("inbound VP8 payload descriptor");
                    let descriptor = black_box(descriptor);
                    for (sender_idx, sender) in self.senders.iter_mut().enumerate() {
                        write_packet(
                            relay_seq_no,
                            relay_timestamp,
                            sender_idx,
                            descriptor,
                            sender,
                            &packet,
                        );
                        sender
                            .rtc
                            .handle_input(Input::Timeout(sender.now))
                            .expect("relay sender timeout");
                        transmit_count += drain_sender_transmits(&mut sender.rtc);
                        relay_seq_no += 1;
                        sender.now += Duration::from_millis(33);
                    }
                    relay_timestamp = relay_timestamp.wrapping_add(VIDEO_TIMESTAMP_STEP);
                    rtp_packets += 1;
                }
            });
            assert_eq!(rtp_packets, 1);
            self.now += Duration::from_millis(33);
        }

        self.seq_no = relay_seq_no;
        self.timestamp = relay_timestamp;
        transmit_count
    }
}

struct InboundVp8Parts {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    now: Instant,
}

fn build_inbound_vp8(rounds: usize) -> InboundVp8Parts {
    install_default_crypto_provider();

    let now = Instant::now();
    let sender = Rtc::builder().set_rtp_mode(true).build(now);
    let receiver = Rtc::builder().set_rtp_mode(true).build(now);
    let ConnectedPair {
        mut left,
        mut right,
        mut now,
        left_addr,
        right_addr,
    } = connect_pair_with(0, now, sender, receiver);

    let mid = "in".into();
    let ssrc = 10_000.into();
    let payload_type = Pt::new_with_value(VP8_PAYLOAD_TYPE_VALUE);

    left.direct_api().declare_media(mid, MediaKind::Video);
    left.direct_api().declare_stream_tx(ssrc, None, mid, None);
    right.direct_api().declare_media(mid, MediaKind::Video);
    right.direct_api().expect_stream_rx(ssrc, None, mid, None);

    let mut source = EgressSender {
        rtc: left,
        ssrc,
        now,
    };
    let mut datagrams = Vec::with_capacity(rounds);

    for idx in 0..rounds {
        let seq_no = 1 + idx as u64;
        let timestamp = 90_000_u32.wrapping_add((idx as u32).wrapping_mul(VIDEO_TIMESTAMP_STEP));
        write_sender_packet(
            &mut source,
            payload_type,
            seq_no,
            timestamp,
            vp8_payload_vec(VP8_PAYLOAD_SIZE),
        );
        source
            .rtc
            .handle_input(Input::Timeout(source.now))
            .expect("inbound source timeout");
        assert_eq!(
            drain_vp8_rtp_datagrams(&mut source.rtc, left_addr, right_addr, &mut datagrams),
            1
        );
        source.now += Duration::from_millis(33);
        now = source.now;
    }

    InboundVp8Parts {
        receiver: right,
        datagrams,
        now,
    }
}

fn vp8_rewrite_1user() -> Vp8EgressFixture {
    Vp8EgressFixture::new(1)
}

fn vp8_rewrite_configured_users() -> Vp8EgressFixture {
    Vp8EgressFixture::new(configured_fanout())
}

fn vp8_full_stack_1user() -> Vp8FullStackFixture {
    Vp8FullStackFixture::new(1)
}

fn vp8_full_stack_configured_users() -> Vp8FullStackFixture {
    Vp8FullStackFixture::new(configured_fanout())
}

fn configured_fanout() -> usize {
    env::var("FANOUT_USERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|fanout| *fanout > 0)
        .unwrap_or(DEFAULT_CONFIGURED_FANOUT)
}

fn callgrind_rounds() -> usize {
    env::var("VP8_REWRITE_CALLGRIND_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|rounds| *rounds > 0)
        .unwrap_or(DEFAULT_CALLGRIND_ROUNDS)
}

fn video_egress_senders(
    fanout: usize,
    pair_index_base: usize,
    ssrc_base: u32,
    mid_prefix: &str,
) -> Vec<EgressSender> {
    let mut senders = Vec::with_capacity(fanout);
    for idx in 0..fanout {
        let now = Instant::now();
        let left = Rtc::builder().set_rtp_mode(true).build(now);
        let right = Rtc::builder().set_rtp_mode(true).build(now);
        let ConnectedPair {
            mut left,
            right: _right,
            now,
            ..
        } = connect_pair_with(pair_index_base + idx, now, left, right);
        let ssrc = (ssrc_base + idx as u32).into();
        let mid = format!("{mid_prefix}{idx:02}");
        let mid = mid.as_str().into();

        let mut direct = left.direct_api();
        direct.declare_media(mid, MediaKind::Video);
        direct.declare_stream_tx(ssrc, None, mid, None);

        senders.push(EgressSender {
            rtc: left,
            ssrc,
            now,
        });
    }

    senders
}

fn vp8_payload_vec(size: usize) -> Vec<u8> {
    assert!(size >= VP8_PICTURE_ID_OFFSET + 2);

    let mut payload: Vec<u8> = (0..size).map(|idx| (idx % 251) as u8).collect();
    payload[0] = 0x90;
    payload[1] = 0x80;
    payload[VP8_PICTURE_ID_OFFSET] = 0x92;
    payload[VP8_PICTURE_ID_OFFSET + 1] = 0x34;
    payload
}

fn vp8_picture_id(seq_no: u64, sender_idx: usize) -> u16 {
    seq_no.wrapping_add(sender_idx as u64) as u16 & VP8_PICTURE_ID_MAX
}

fn vp8_picture_id_bytes(picture_id: u16) -> [u8; 2] {
    [
        0x80 | ((picture_id >> 8) as u8 & VP8_PICTURE_ID_SHORT_MASK),
        picture_id as u8,
    ]
}

fn copied_payload_with_picture_id(payload: &[u8], picture_id: u16) -> Vec<u8> {
    let mut rewritten = payload.to_vec();
    let picture_id = vp8_picture_id_bytes(picture_id);
    rewritten[VP8_PICTURE_ID_OFFSET..VP8_PICTURE_ID_OFFSET + picture_id.len()]
        .copy_from_slice(&picture_id);
    rewritten
}

fn vp8_picture_id_patch(descriptor: Vp8Descriptor, picture_id: u16) -> Vp8Patch {
    descriptor
        .patch()
        .picture_id(picture_id)
        .build()
        .expect("valid VP8 PictureID patch")
}

fn write_sender_packet(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    payload: impl Into<SharedPayload>,
) {
    let rtp = RtpWrite::new(payload_type, seq_no.into(), timestamp, sender.now, payload);
    write_sender_rtp(sender, rtp);
}

fn write_sender_packet_with_ext_vals(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    ext_vals: ExtensionValues,
    payload: impl Into<SharedPayload>,
) {
    let rtp = RtpWrite::new(payload_type, seq_no.into(), timestamp, sender.now, payload)
        .ext_vals(ext_vals);
    write_sender_rtp(sender, rtp);
}

fn write_sender_packet_with_vp8_patch(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    payload: impl Into<SharedPayload>,
    patch: Vp8Patch,
) {
    let rtp =
        RtpWrite::new(payload_type, seq_no.into(), timestamp, sender.now, payload).vp8_patch(patch);
    write_sender_rtp(sender, rtp);
}

fn write_sender_packet_with_ext_vals_and_vp8_patch(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    ext_vals: ExtensionValues,
    payload: impl Into<SharedPayload>,
    patch: Vp8Patch,
) {
    let rtp = RtpWrite::new(payload_type, seq_no.into(), timestamp, sender.now, payload)
        .ext_vals(ext_vals)
        .vp8_patch(patch);
    write_sender_rtp(sender, rtp);
}

fn write_sender_rtp(sender: &mut EgressSender, rtp: RtpWrite) {
    let mut direct = sender.rtc.direct_api();
    let stream = direct.stream_tx(&sender.ssrc).expect("declared stream");
    stream.write_rtp(rtp);
}

fn drain_sender_transmits(rtc: &mut Rtc) -> usize {
    let mut transmit_count = 0;
    loop {
        match rtc.poll_output().expect("poll sender output") {
            Output::Transmit(_) => transmit_count += 1,
            Output::Event(_) => {}
            Output::Timeout(_) => return transmit_count,
        }
    }
}

fn drain_receiver_events(rtc: &mut Rtc, on_event: &mut impl FnMut(Event)) {
    loop {
        match rtc.poll_output().expect("poll receiver output") {
            Output::Transmit(_) => {}
            Output::Event(event) => on_event(event),
            Output::Timeout(_) => return,
        }
    }
}

fn install_default_crypto_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        str0m::crypto::from_feature_flags().install_process_default();
    });
}

fn connect_pair_with(index: usize, now: Instant, left: Rtc, right: Rtc) -> ConnectedPair {
    let left_addr = socket_addr(1, 1, 1, 1, 10_000 + index as u16);
    let right_addr = socket_addr(2, 2, 2, 2, 20_000 + index as u16);
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

fn drain_vp8_rtp_datagrams(
    rtc: &mut Rtc,
    source: SocketAddr,
    destination: SocketAddr,
    out: &mut Vec<PendingDatagram>,
) -> usize {
    let mut accepted = 0;
    loop {
        match rtc.poll_output().expect("poll sender output") {
            Output::Transmit(transmit) => {
                assert_eq!(transmit.source, source);
                assert_eq!(transmit.destination, destination);
                if is_vp8_rtp_datagram(&transmit.contents) {
                    accepted += 1;
                    out.push(PendingDatagram {
                        source: transmit.source,
                        destination: transmit.destination,
                        contents: transmit.contents.to_vec(),
                    });
                }
            }
            Output::Event(_) => {}
            Output::Timeout(_) => return accepted,
        }
    }
}

fn is_vp8_rtp_datagram(contents: &[u8]) -> bool {
    contents.len() >= 12
        && contents[0] & 0xc0 == 0x80
        && (contents[1] & 0x7f) == VP8_PAYLOAD_TYPE_VALUE
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

fn deliver_packets(rtc: &mut Rtc, now: Instant, queue: &mut Vec<PendingDatagram>) {
    for packet in queue.drain(..) {
        let receive = receive_from_datagram(&packet);
        rtc.handle_input(Input::Receive(now, receive))
            .expect("deliver pair packet");
    }
}

fn socket_addr(a: u8, b: u8, c: u8, d: u8, port: u16) -> SocketAddr {
    (Ipv4Addr::new(a, b, c, d), port).into()
}

fn callgrind_config() -> LibraryBenchmarkConfig {
    let mut callgrind = Callgrind::default();
    callgrind.fail_fast(false);
    callgrind.format([CallgrindMetrics::All]);

    let mut config = LibraryBenchmarkConfig::default();
    config.tool(callgrind);
    config.pass_through_envs(["FANOUT_USERS", "VP8_REWRITE_CALLGRIND_ROUNDS"]);
    config
}

#[library_benchmark(config = callgrind_config())]
#[bench::p1200b_1user(setup = vp8_rewrite_1user)]
#[bench::p1200b_configured_users(setup = vp8_rewrite_configured_users)]
fn vp8_rewrite_copy_then_write_rtp(mut fixture: Vp8EgressFixture) -> usize {
    black_box(fixture.rewrite_with_copy())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p1200b_1user(setup = vp8_rewrite_1user)]
#[bench::p1200b_configured_users(setup = vp8_rewrite_configured_users)]
fn vp8_rewrite_shared_payload(mut fixture: Vp8EgressFixture) -> usize {
    black_box(fixture.rewrite_with_vp8_patch())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p1200b_1user(setup = vp8_full_stack_1user)]
#[bench::p1200b_configured_users(setup = vp8_full_stack_configured_users)]
fn vp8_full_stack_copy_then_write_rtp(mut fixture: Vp8FullStackFixture) -> usize {
    black_box(fixture.relay_with_copy())
}

#[library_benchmark(config = callgrind_config())]
#[bench::p1200b_1user(setup = vp8_full_stack_1user)]
#[bench::p1200b_configured_users(setup = vp8_full_stack_configured_users)]
fn vp8_full_stack_shared_payload(mut fixture: Vp8FullStackFixture) -> usize {
    black_box(fixture.relay_with_vp8_patch())
}

library_benchmark_group!(
    name = vp8_rewrite_callgrind;
    benchmarks = vp8_rewrite_copy_then_write_rtp, vp8_rewrite_shared_payload,
        vp8_full_stack_copy_then_write_rtp, vp8_full_stack_shared_payload
);

main!(library_benchmark_groups = vp8_rewrite_callgrind);
