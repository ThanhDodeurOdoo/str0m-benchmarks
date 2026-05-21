use std::collections::VecDeque;
use std::env;
use std::net::{Ipv4Addr, SocketAddr};
#[cfg(feature = "arc-payload")]
use std::sync::Arc;
use std::sync::Once;
use std::time::{Duration, Instant};

use str0m::format::Codec;
use str0m::media::{MediaKind, Pt};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{ExtensionValues, Ssrc};
use str0m::{Candidate, Event, Input, Output, Rtc};

const DEFAULT_CONFIGURED_FANOUT: usize = 30;

#[cfg(feature = "arc-payload")]
pub type SharedPayload = Arc<[u8]>;

#[cfg(not(feature = "arc-payload"))]
pub type SharedPayload = Vec<u8>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchPacketVec {
    pub destination: Ssrc,
    pub payload_type: Pt,
    pub ext_vals: ExtensionValues,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchPacketShared {
    pub destination: Ssrc,
    pub payload_type: Pt,
    pub ext_vals: ExtensionValues,
    pub payload: SharedPayload,
}

pub fn benchmark_targets(fanout: usize) -> Vec<Ssrc> {
    (0..fanout)
        .map(|idx| (10_000 + idx as u32).into())
        .collect()
}

pub fn configured_fanout() -> usize {
    env::var("FANOUT_USERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|fanout| *fanout > 0)
        .unwrap_or(DEFAULT_CONFIGURED_FANOUT)
}

pub fn benchmark_fanouts() -> Vec<usize> {
    let configured = configured_fanout();
    if configured == 1 {
        vec![1]
    } else {
        vec![1, configured]
    }
}

pub fn make_payload(size: usize) -> Vec<u8> {
    (0..size).map(|idx| (idx % 251) as u8).collect()
}

pub fn packet_template_vec(size: usize) -> BenchPacketVec {
    BenchPacketVec {
        destination: 0.into(),
        payload_type: Pt::new_with_value(111),
        ext_vals: ExtensionValues {
            audio_level: Some(-42),
            voice_activity: Some(true),
            ..Default::default()
        },
        payload: make_payload(size),
    }
}

pub fn packet_template_shared(size: usize) -> BenchPacketShared {
    BenchPacketShared {
        destination: 0.into(),
        payload_type: Pt::new_with_value(111),
        ext_vals: ExtensionValues {
            audio_level: Some(-42),
            voice_activity: Some(true),
            ..Default::default()
        },
        payload: shared_payload(size),
    }
}

#[cfg(feature = "arc-payload")]
pub fn shared_payload(size: usize) -> SharedPayload {
    make_payload(size).into()
}

#[cfg(not(feature = "arc-payload"))]
pub fn shared_payload(size: usize) -> SharedPayload {
    make_payload(size)
}

#[cfg(feature = "arc-payload")]
fn payload_to_vec(payload: &SharedPayload) -> Vec<u8> {
    payload.as_ref().to_vec()
}

#[cfg(not(feature = "arc-payload"))]
fn payload_to_vec(payload: &SharedPayload) -> Vec<u8> {
    payload.clone()
}

pub fn forward_vec(template: &BenchPacketVec, targets: &[Ssrc], out: &mut Vec<BenchPacketVec>) {
    out.clear();
    out.reserve(targets.len().saturating_sub(out.capacity()));
    for &destination in targets {
        out.push(BenchPacketVec {
            destination,
            payload_type: template.payload_type,
            ext_vals: template.ext_vals.clone(),
            payload: template.payload.clone(),
        });
    }
}

pub fn forward_shared(
    template: &BenchPacketShared,
    targets: &[Ssrc],
    out: &mut Vec<BenchPacketShared>,
) {
    out.clear();
    out.reserve(targets.len().saturating_sub(out.capacity()));
    for &destination in targets {
        out.push(BenchPacketShared {
            destination,
            payload_type: template.payload_type,
            ext_vals: template.ext_vals.clone(),
            payload: template.payload.clone(),
        });
    }
}

pub fn forward_payload_vec(
    payload_type: Pt,
    ext_vals: &ExtensionValues,
    payload: &[u8],
    targets: &[Ssrc],
    out: &mut Vec<BenchPacketVec>,
) {
    out.clear();
    out.reserve(targets.len().saturating_sub(out.capacity()));
    for &destination in targets {
        out.push(BenchPacketVec {
            destination,
            payload_type,
            ext_vals: ext_vals.clone(),
            payload: payload.to_vec(),
        });
    }
}

#[cfg(feature = "arc-payload")]
pub fn forward_payload_shared(
    payload_type: Pt,
    ext_vals: &ExtensionValues,
    payload: &SharedPayload,
    targets: &[Ssrc],
    out: &mut Vec<BenchPacketShared>,
) {
    out.clear();
    out.reserve(targets.len().saturating_sub(out.capacity()));
    for &destination in targets {
        out.push(BenchPacketShared {
            destination,
            payload_type,
            ext_vals: ext_vals.clone(),
            payload: payload.clone(),
        });
    }
}

pub struct EnqueueHarness {
    rtc: Rtc,
    targets: Vec<Ssrc>,
    payload_type: Pt,
    wallclock: Instant,
    ext_vals: ExtensionValues,
    seq_no: u64,
    timestamp: u32,
}

pub struct FullEgressHarness {
    senders: Vec<EgressSender>,
    payload_type: Pt,
    ext_vals: ExtensionValues,
    seq_no: u64,
    timestamp: u32,
}

pub struct FullRelayHarness {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    senders: Vec<EgressSender>,
    now: Instant,
    seq_no: u64,
    timestamp: u32,
}

pub struct ReceiveRtpHarness {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    now: Instant,
}

pub struct ReceiveMediaHarness {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    now: Instant,
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

impl EnqueueHarness {
    pub fn new(fanout: usize) -> Self {
        let now = Instant::now();
        let mut rtc = Rtc::builder().set_rtp_mode(true).build(now);
        let targets = benchmark_targets(fanout);

        for (idx, ssrc) in targets.iter().copied().enumerate() {
            let mid = format!("m{idx:02}");
            let mid = mid.as_str().into();
            let mut direct = rtc.direct_api();
            direct.declare_media(mid, MediaKind::Audio);
            direct.declare_stream_tx(ssrc, None, mid, None);
        }

        Self {
            rtc,
            targets,
            payload_type: Pt::new_with_value(111),
            wallclock: now,
            ext_vals: ExtensionValues {
                audio_level: Some(-9),
                voice_activity: Some(true),
                ..Default::default()
            },
            seq_no: 1,
            timestamp: 48_000,
        }
    }

    pub fn enqueue_vec(&mut self, payload: &[u8], rounds: usize) {
        for _ in 0..rounds {
            let mut direct = self.rtc.direct_api();
            for idx in 0..self.targets.len() {
                let ssrc = self.targets[idx];
                let stream = direct.stream_tx(&ssrc).expect("declared stream");
                stream
                    .write_rtp(
                        self.payload_type,
                        self.seq_no.into(),
                        self.timestamp,
                        self.wallclock,
                        false,
                        self.ext_vals.clone(),
                        false,
                        payload.to_vec(),
                    )
                    .expect("enqueue vec payload");
                self.seq_no += 1;
                self.timestamp = self.timestamp.wrapping_add(960);
            }
        }
    }

    pub fn enqueue_bytes(&mut self, payload: &SharedPayload, rounds: usize) {
        for _ in 0..rounds {
            let mut direct = self.rtc.direct_api();
            for idx in 0..self.targets.len() {
                let ssrc = self.targets[idx];
                let stream = direct.stream_tx(&ssrc).expect("declared stream");
                stream
                    .write_rtp(
                        self.payload_type,
                        self.seq_no.into(),
                        self.timestamp,
                        self.wallclock,
                        false,
                        self.ext_vals.clone(),
                        false,
                        payload.clone(),
                    )
                    .expect("enqueue bytes payload");
                self.seq_no += 1;
                self.timestamp = self.timestamp.wrapping_add(960);
            }
        }
    }
}

impl FullEgressHarness {
    pub fn new(fanout: usize) -> Self {
        install_default_crypto_provider();

        let mut senders = Vec::with_capacity(fanout);
        for idx in 0..fanout {
            let ConnectedPair {
                mut left,
                right: _right,
                now,
                ..
            } = connect_pair(idx);
            let ssrc = (10_000 + idx as u32).into();
            let mid = format!("m{idx:02}");
            let mid = mid.as_str().into();

            let mut direct = left.direct_api();
            direct.declare_media(mid, MediaKind::Audio);
            direct.declare_stream_tx(ssrc, None, mid, None);

            senders.push(EgressSender {
                rtc: left,
                ssrc,
                now,
            });
        }

        Self {
            senders,
            payload_type: Pt::new_with_value(111),
            ext_vals: ExtensionValues {
                audio_level: Some(-9),
                voice_activity: Some(true),
                ..Default::default()
            },
            seq_no: 1,
            timestamp: 48_000,
        }
    }

    pub fn egress_vec(&mut self, payload: &[u8], rounds: usize) -> usize {
        self.egress(
            rounds,
            |seq_no, timestamp, sender, payload_type, ext_vals| {
                write_sender_packet(
                    sender,
                    payload_type,
                    seq_no,
                    timestamp,
                    ext_vals,
                    payload.to_vec(),
                )
            },
        )
    }

    pub fn egress_bytes(&mut self, payload: &SharedPayload, rounds: usize) -> usize {
        self.egress(
            rounds,
            |seq_no, timestamp, sender, payload_type, ext_vals| {
                write_sender_packet(
                    sender,
                    payload_type,
                    seq_no,
                    timestamp,
                    ext_vals,
                    payload.clone(),
                )
            },
        )
    }

    fn egress(
        &mut self,
        rounds: usize,
        mut write_packet: impl FnMut(u64, u32, &mut EgressSender, Pt, ExtensionValues),
    ) -> usize {
        let mut transmit_count = 0;
        for _ in 0..rounds {
            for sender in &mut self.senders {
                write_packet(
                    self.seq_no,
                    self.timestamp,
                    sender,
                    self.payload_type,
                    self.ext_vals.clone(),
                );
                sender
                    .rtc
                    .handle_input(Input::Timeout(sender.now))
                    .expect("sender timeout");
                transmit_count += drain_sender_transmits(&mut sender.rtc);
                self.seq_no += 1;
                self.timestamp = self.timestamp.wrapping_add(960);
                sender.now += Duration::from_millis(20);
            }
        }
        transmit_count
    }
}

impl FullRelayHarness {
    pub fn new(fanout: usize, payload_size: usize, rounds: usize) -> Self {
        let receiver = ReceiveHarnessBuilder::new(payload_size, rounds, true).build();
        let mut senders = Vec::with_capacity(fanout);

        for idx in 0..fanout {
            let ConnectedPair {
                mut left,
                right: _right,
                now,
                ..
            } = connect_pair(idx + 1);
            let ssrc = (20_000 + idx as u32).into();
            let mid = format!("o{idx:02}");
            let mid = mid.as_str().into();

            let mut direct = left.direct_api();
            direct.declare_media(mid, MediaKind::Audio);
            direct.declare_stream_tx(ssrc, None, mid, None);

            senders.push(EgressSender {
                rtc: left,
                ssrc,
                now,
            });
        }

        Self {
            receiver: receiver.receiver,
            datagrams: receiver.datagrams,
            senders,
            now: receiver.now,
            seq_no: 1,
            timestamp: 48_000,
        }
    }

    pub fn relay_vec(&mut self) -> usize {
        let mut transmit_count = 0;
        let mut relay_seq_no = self.seq_no;
        let mut relay_timestamp = self.timestamp;
        for packet in &self.datagrams {
            let receive = receive_from_datagram(packet);
            self.receiver
                .handle_input(Input::Receive(self.now, receive))
                .expect("receive relay packet");
            drain_receiver_events(&mut self.receiver, &mut |event| {
                if let Event::RtpPacket(packet) = event {
                    for sender in &mut self.senders {
                        write_sender_packet(
                            sender,
                            packet.header.payload_type,
                            relay_seq_no,
                            relay_timestamp,
                            packet.header.ext_vals.clone(),
                            payload_to_vec(&packet.payload),
                        );
                        sender
                            .rtc
                            .handle_input(Input::Timeout(sender.now))
                            .expect("relay sender timeout");
                        transmit_count += drain_sender_transmits(&mut sender.rtc);
                        sender.now += Duration::from_millis(20);
                        relay_seq_no += 1;
                    }
                    relay_timestamp = relay_timestamp.wrapping_add(960);
                }
            });
            self.now += Duration::from_millis(20);
        }
        self.seq_no = relay_seq_no;
        self.timestamp = relay_timestamp;
        transmit_count
    }

    #[cfg(feature = "arc-payload")]
    pub fn relay_shared(&mut self) -> usize {
        let mut transmit_count = 0;
        let mut relay_seq_no = self.seq_no;
        let mut relay_timestamp = self.timestamp;
        for packet in &self.datagrams {
            let receive = receive_from_datagram(packet);
            self.receiver
                .handle_input(Input::Receive(self.now, receive))
                .expect("receive relay packet");
            drain_receiver_events(&mut self.receiver, &mut |event| {
                if let Event::RtpPacket(packet) = event {
                    for sender in &mut self.senders {
                        write_sender_packet(
                            sender,
                            packet.header.payload_type,
                            relay_seq_no,
                            relay_timestamp,
                            packet.header.ext_vals.clone(),
                            packet.payload.clone(),
                        );
                        sender
                            .rtc
                            .handle_input(Input::Timeout(sender.now))
                            .expect("relay sender timeout");
                        transmit_count += drain_sender_transmits(&mut sender.rtc);
                        sender.now += Duration::from_millis(20);
                        relay_seq_no += 1;
                    }
                    relay_timestamp = relay_timestamp.wrapping_add(960);
                }
            });
            self.now += Duration::from_millis(20);
        }
        self.seq_no = relay_seq_no;
        self.timestamp = relay_timestamp;
        transmit_count
    }
}

impl ReceiveRtpHarness {
    pub fn new(payload_size: usize, rounds: usize) -> Self {
        let receiver = ReceiveHarnessBuilder::new(payload_size, rounds, true).build();

        Self {
            receiver: receiver.receiver,
            datagrams: receiver.datagrams,
            now: receiver.now,
        }
    }

    pub fn receive_events(&mut self) -> usize {
        let mut payload_bytes = 0;
        self.receive_each(|event| {
            if let Event::RtpPacket(packet) = event {
                payload_bytes += packet.payload.len();
            }
        });
        payload_bytes
    }

    pub fn fanout_vec(&mut self, targets: &[Ssrc], out: &mut Vec<BenchPacketVec>) -> usize {
        let mut forwarded = 0;
        self.receive_each(|event| {
            if let Event::RtpPacket(packet) = event {
                forward_payload_vec(
                    packet.header.payload_type,
                    &packet.header.ext_vals,
                    packet.payload.as_ref(),
                    targets,
                    out,
                );
                forwarded += out.len();
            }
        });
        forwarded
    }

    #[cfg(feature = "arc-payload")]
    pub fn fanout_shared(&mut self, targets: &[Ssrc], out: &mut Vec<BenchPacketShared>) -> usize {
        let mut forwarded = 0;
        self.receive_each(|event| {
            if let Event::RtpPacket(packet) = event {
                forward_payload_shared(
                    packet.header.payload_type,
                    &packet.header.ext_vals,
                    &packet.payload,
                    targets,
                    out,
                );
                forwarded += out.len();
            }
        });
        forwarded
    }

    fn receive_each(&mut self, mut on_event: impl FnMut(Event)) -> usize {
        receive_each(
            &mut self.receiver,
            &self.datagrams,
            &mut self.now,
            &mut on_event,
        )
    }
}

impl ReceiveMediaHarness {
    pub fn new(payload_size: usize, rounds: usize) -> Self {
        let receiver = ReceiveHarnessBuilder::new(payload_size, rounds, false).build();

        Self {
            receiver: receiver.receiver,
            datagrams: receiver.datagrams,
            now: receiver.now,
        }
    }

    pub fn receive_events(&mut self) -> usize {
        let mut payload_bytes = 0;
        self.receive_each(|event| {
            if let Event::MediaData(data) = event {
                payload_bytes += data.data.len();
            }
        });
        payload_bytes
    }

    pub fn fanout_vec(&mut self, targets: &[Ssrc], out: &mut Vec<BenchPacketVec>) -> usize {
        let mut forwarded = 0;
        self.receive_each(|event| {
            if let Event::MediaData(data) = event {
                forward_payload_vec(data.pt, &data.ext_vals, data.data.as_ref(), targets, out);
                forwarded += out.len();
            }
        });
        forwarded
    }

    #[cfg(feature = "arc-payload")]
    pub fn fanout_shared(&mut self, targets: &[Ssrc], out: &mut Vec<BenchPacketShared>) -> usize {
        let mut forwarded = 0;
        self.receive_each(|event| {
            if let Event::MediaData(data) = event {
                forward_payload_shared(data.pt, &data.ext_vals, &data.data, targets, out);
                forwarded += out.len();
            }
        });
        forwarded
    }

    fn receive_each(&mut self, mut on_event: impl FnMut(Event)) -> usize {
        receive_each(
            &mut self.receiver,
            &self.datagrams,
            &mut self.now,
            &mut on_event,
        )
    }
}

struct ReceiveHarnessBuilder {
    payload_size: usize,
    rounds: usize,
    rtp_mode_receiver: bool,
}

struct ReceiveHarnessParts {
    receiver: Rtc,
    datagrams: Vec<PendingDatagram>,
    now: Instant,
}

impl ReceiveHarnessBuilder {
    fn new(payload_size: usize, rounds: usize, rtp_mode_receiver: bool) -> Self {
        Self {
            payload_size,
            rounds,
            rtp_mode_receiver,
        }
    }

    fn build(self) -> ReceiveHarnessParts {
        install_default_crypto_provider();

        let now = Instant::now();
        let sender = Rtc::builder().set_rtp_mode(true).build(now);
        let receiver = if self.rtp_mode_receiver {
            Rtc::builder().set_rtp_mode(true).build(now)
        } else {
            Rtc::builder().set_reordering_size_audio(0).build(now)
        };
        let ConnectedPair {
            mut left,
            mut right,
            mut now,
            left_addr,
            right_addr,
        } = connect_pair_with(0, now, sender, receiver);

        let mid = "m00".into();
        let ssrc: Ssrc = 10_000.into();
        let payload_type = left
            .codec_config()
            .find(|params| params.spec().codec == Codec::Opus)
            .expect("Opus codec")
            .pt();

        left.direct_api().declare_media(mid, MediaKind::Audio);
        left.direct_api().declare_stream_tx(ssrc, None, mid, None);
        right.direct_api().declare_media(mid, MediaKind::Audio);
        if self.rtp_mode_receiver {
            right.direct_api().expect_stream_rx(ssrc, None, mid, None);
        }

        let mut sender = EgressSender {
            rtc: left,
            ssrc,
            now,
        };
        let ext_vals = ExtensionValues {
            audio_level: Some(-9),
            voice_activity: Some(true),
            ..Default::default()
        };
        let mut datagrams = Vec::with_capacity(self.rounds);

        for idx in 0..self.rounds {
            let seq_no = 1 + idx as u64;
            let timestamp = 48_000_u32.wrapping_add((idx as u32).wrapping_mul(960));
            write_sender_packet(
                &mut sender,
                payload_type,
                seq_no,
                timestamp,
                ext_vals.clone(),
                make_payload(self.payload_size),
            );
            sender
                .rtc
                .handle_input(Input::Timeout(sender.now))
                .expect("sender timeout");
            drain_transmit_datagrams(&mut sender.rtc, left_addr, right_addr, &mut datagrams);
            sender.now += Duration::from_millis(20);
            now = sender.now;
        }

        ReceiveHarnessParts {
            receiver: right,
            datagrams,
            now,
        }
    }
}

fn receive_each(
    receiver: &mut Rtc,
    datagrams: &[PendingDatagram],
    now: &mut Instant,
    on_event: &mut impl FnMut(Event),
) -> usize {
    let mut events = 0;
    for packet in datagrams {
        let receive = receive_from_datagram(packet);
        receiver
            .handle_input(Input::Receive(*now, receive))
            .expect("receive packet");
        events += drain_receiver_events(receiver, on_event);
        *now += Duration::from_millis(20);
    }
    events
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

fn drain_receiver_events(rtc: &mut Rtc, on_event: &mut impl FnMut(Event)) -> usize {
    let mut event_count = 0;
    loop {
        match rtc.poll_output().expect("poll receiver output") {
            Output::Transmit(_) => {}
            Output::Event(event) => {
                event_count += 1;
                on_event(event);
            }
            Output::Timeout(_) => return event_count,
        }
    }
}

#[cfg(feature = "arc-payload")]
fn write_sender_packet(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    ext_vals: ExtensionValues,
    payload: impl Into<SharedPayload>,
) {
    let mut direct = sender.rtc.direct_api();
    let stream = direct.stream_tx(&sender.ssrc).expect("declared stream");
    stream
        .write_rtp(
            payload_type,
            seq_no.into(),
            timestamp,
            sender.now,
            false,
            ext_vals,
            false,
            payload,
        )
        .expect("write RTP packet");
}

#[cfg(not(feature = "arc-payload"))]
fn write_sender_packet(
    sender: &mut EgressSender,
    payload_type: Pt,
    seq_no: u64,
    timestamp: u32,
    ext_vals: ExtensionValues,
    payload: Vec<u8>,
) {
    let mut direct = sender.rtc.direct_api();
    let stream = direct.stream_tx(&sender.ssrc).expect("declared stream");
    stream
        .write_rtp(
            payload_type,
            seq_no.into(),
            timestamp,
            sender.now,
            false,
            ext_vals,
            false,
            payload,
        )
        .expect("write RTP packet");
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

fn connect_pair(index: usize) -> ConnectedPair {
    let now = Instant::now();
    let left = Rtc::builder().set_rtp_mode(true).build(now);
    let right = Rtc::builder().set_rtp_mode(true).build(now);

    connect_pair_with(index, now, left, right)
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
    let mut left_queue = VecDeque::new();
    let mut right_queue = VecDeque::new();
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
    peer_queue: &mut VecDeque<PendingDatagram>,
) {
    loop {
        match rtc.poll_output().expect("poll pair output") {
            Output::Transmit(transmit) => {
                assert_eq!(transmit.source, source);
                assert_eq!(transmit.destination, destination);
                peer_queue.push_back(PendingDatagram {
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

fn deliver_packets(rtc: &mut Rtc, now: Instant, queue: &mut VecDeque<PendingDatagram>) {
    while let Some(packet) = queue.pop_front() {
        let receive = Receive {
            proto: Protocol::Udp,
            source: packet.source,
            destination: packet.destination,
            contents: packet
                .contents
                .as_slice()
                .try_into()
                .expect("datagram receive"),
        };
        rtc.handle_input(Input::Receive(now, receive))
            .expect("deliver pair packet");
    }
}

fn socket_addr(a: u8, b: u8, c: u8, d: u8, port: u16) -> SocketAddr {
    (Ipv4Addr::new(a, b, c, d), port).into()
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::*;

    #[test]
    fn forward_vec_clones_payload_bytes_per_target() {
        let template = packet_template_vec(32);
        let targets = benchmark_targets(3);
        let mut out = Vec::new();

        forward_vec(&template, &targets, &mut out);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0].payload, template.payload);
        assert!(!ptr::eq(out[0].payload.as_ptr(), template.payload.as_ptr()));
    }

    #[cfg(feature = "arc-payload")]
    #[test]
    fn forward_shared_shares_payload_storage_per_target() {
        let template = packet_template_shared(32);
        let targets = benchmark_targets(3);
        let mut out = Vec::new();

        forward_shared(&template, &targets, &mut out);

        assert_eq!(out.len(), 3);
        assert_eq!(out[0].payload, template.payload);
        assert!(ptr::eq(out[0].payload.as_ptr(), template.payload.as_ptr()));
    }

    #[test]
    fn enqueue_harness_accepts_vec_and_shared_payloads() {
        let payload_vec = make_payload(160);
        let payload_shared = shared_payload(160);
        let mut vec_harness = EnqueueHarness::new(2);
        let mut shared_harness = EnqueueHarness::new(2);

        vec_harness.enqueue_vec(&payload_vec, 2);
        shared_harness.enqueue_bytes(&payload_shared, 2);
    }

    #[test]
    fn full_egress_harness_emits_transmits() {
        let payload_vec = make_payload(160);
        let payload_shared = shared_payload(160);
        let mut vec_harness = FullEgressHarness::new(1);
        let mut shared_harness = FullEgressHarness::new(1);

        assert!(vec_harness.egress_vec(&payload_vec, 2) >= 2);
        assert!(shared_harness.egress_bytes(&payload_shared, 2) >= 2);
    }

    #[test]
    fn full_relay_harness_emits_transmits() {
        let mut harness = FullRelayHarness::new(2, 160, 2);

        assert!(harness.relay_vec() >= 4);
    }

    #[test]
    fn receive_rtp_harness_receives_payloads() {
        let mut harness = ReceiveRtpHarness::new(160, 2);

        assert_eq!(harness.receive_events(), 320);
    }

    #[test]
    fn receive_media_harness_receives_payloads() {
        let mut harness = ReceiveMediaHarness::new(160, 2);

        assert_eq!(harness.receive_events(), 320);
    }
}
