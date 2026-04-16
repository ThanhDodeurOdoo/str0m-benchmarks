use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use str0m_benchmarks::{
    EnqueueHarness, FullEgressHarness, FullRelayHarness, ReceiveMediaHarness, ReceiveRtpHarness,
    forward_vec, packet_template_vec,
};

#[cfg(feature = "arc-payload")]
use str0m_benchmarks::{forward_shared, packet_template_shared, shared_payload};

struct PayloadScenario {
    label: &'static str,
    size: usize,
}

// Amount of destinations, for example users in a channel consuming a stream.
const FANOUTS: &[usize] = &[1, 2, 10, 50];

// Payload sizes chosen to stay close to realistic RTP payload sizes:
// - 160B  : common small Opus packet size.
// - 1200B : common video RTP payload size that stays comfortably below MTU.
// - 1350B : larger video RTP payload size that is still realistic on the wire.
const PAYLOAD_SCENARIOS: &[PayloadScenario] = &[
    PayloadScenario {
        label: "audio-160B",
        size: 160,
    },
    PayloadScenario {
        label: "video-1200B",
        size: 1200,
    },
    PayloadScenario {
        label: "video-1350B",
        size: 1350,
    },
];

const ENQUEUE_ROUNDS: usize = 64;
const FULL_EGRESS_ROUNDS: usize = 64;
const FULL_RELAY_ROUNDS: usize = 64;
const RECEIVE_ROUNDS: usize = 64;

fn bench_packet_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("packet_fanout");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let vec_template = packet_template_vec(payload_size);
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    let mut out = Vec::with_capacity(fanout);
                    b.iter(|| {
                        forward_vec(black_box(&vec_template), black_box(&targets), &mut out);
                        black_box(&out);
                    });
                },
            );
            #[cfg(feature = "arc-payload")]
            let shared_template = packet_template_shared(payload_size);
            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    let mut out = Vec::with_capacity(fanout);
                    b.iter(|| {
                        forward_shared(black_box(&shared_template), black_box(&targets), &mut out);
                        black_box(&out);
                    });
                },
            );
        }
    }

    group.finish();
}

fn bench_enqueue(c: &mut Criterion) {
    let mut group = c.benchmark_group("enqueue");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let payload_vec = str0m_benchmarks::make_payload(payload_size);
            let throughput = Throughput::Bytes((payload_size * fanout * ENQUEUE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || EnqueueHarness::new(fanout),
                        |mut harness| {
                            harness.enqueue_vec(black_box(&payload_vec), ENQUEUE_ROUNDS);
                            black_box(harness);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
            #[cfg(feature = "arc-payload")]
            let payload_shared = shared_payload(payload_size);
            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || EnqueueHarness::new(fanout),
                        |mut harness| {
                            harness.enqueue_bytes(black_box(&payload_shared), ENQUEUE_ROUNDS);
                            black_box(harness);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_full_egress(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_egress");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let payload_vec = str0m_benchmarks::make_payload(payload_size);
            let throughput = Throughput::Bytes((payload_size * fanout * FULL_EGRESS_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullEgressHarness::new(fanout),
                        |mut harness| {
                            let transmit_count =
                                harness.egress_vec(black_box(&payload_vec), FULL_EGRESS_ROUNDS);
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
            #[cfg(feature = "arc-payload")]
            let payload_shared = shared_payload(payload_size);
            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullEgressHarness::new(fanout),
                        |mut harness| {
                            let transmit_count = harness
                                .egress_bytes(black_box(&payload_shared), FULL_EGRESS_ROUNDS);
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_full_relay_rtp(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_relay_rtp");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let throughput = Throughput::Bytes((payload_size * fanout * FULL_RELAY_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullRelayHarness::new(fanout, payload_size, FULL_RELAY_ROUNDS),
                        |mut harness| {
                            let transmit_count = harness.relay_vec();
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );

            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullRelayHarness::new(fanout, payload_size, FULL_RELAY_ROUNDS),
                        |mut harness| {
                            let transmit_count = harness.relay_shared();
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_receive_rtp_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("receive_rtp_event");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        let throughput = Throughput::Bytes((payload_size * RECEIVE_ROUNDS) as u64);

        group.throughput(throughput);
        group.bench_with_input(
            BenchmarkId::new("event_only", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveRtpHarness::new(payload_size, RECEIVE_ROUNDS),
                    |mut harness| {
                        let payload_bytes = harness.receive_events();
                        black_box(payload_bytes);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_receive_rtp_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("receive_rtp_fanout");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout * RECEIVE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveRtpHarness::new(payload_size, RECEIVE_ROUNDS),
                        |mut harness| {
                            let mut out = Vec::with_capacity(fanout);
                            let forwarded = harness.fanout_vec(black_box(&targets), &mut out);
                            black_box(forwarded);
                            black_box(out);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );

            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveRtpHarness::new(payload_size, RECEIVE_ROUNDS),
                        |mut harness| {
                            let mut out = Vec::with_capacity(fanout);
                            let forwarded = harness.fanout_shared(black_box(&targets), &mut out);
                            black_box(forwarded);
                            black_box(out);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_receive_media_event(c: &mut Criterion) {
    let mut group = c.benchmark_group("receive_media_event");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        let throughput = Throughput::Bytes((payload_size * RECEIVE_ROUNDS) as u64);

        group.throughput(throughput);
        group.bench_with_input(
            BenchmarkId::new("event_only", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveMediaHarness::new(payload_size, RECEIVE_ROUNDS),
                    |mut harness| {
                        let payload_bytes = harness.receive_events();
                        black_box(payload_bytes);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_receive_media_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("receive_media_fanout");

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in FANOUTS {
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout * RECEIVE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("copied_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveMediaHarness::new(payload_size, RECEIVE_ROUNDS),
                        |mut harness| {
                            let mut out = Vec::with_capacity(fanout);
                            let forwarded = harness.fanout_vec(black_box(&targets), &mut out);
                            black_box(forwarded);
                            black_box(out);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );

            #[cfg(feature = "arc-payload")]
            group.bench_with_input(
                BenchmarkId::new("shared_arc", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveMediaHarness::new(payload_size, RECEIVE_ROUNDS),
                        |mut harness| {
                            let mut out = Vec::with_capacity(fanout);
                            let forwarded = harness.fanout_shared(black_box(&targets), &mut out);
                            black_box(forwarded);
                            black_box(out);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(200))
        .measurement_time(Duration::from_millis(500));
    targets = bench_packet_fanout, bench_enqueue, bench_full_egress, bench_receive_rtp_event,
        bench_receive_rtp_fanout, bench_receive_media_event, bench_receive_media_fanout,
        bench_full_relay_rtp
);
criterion_main!(benches);
