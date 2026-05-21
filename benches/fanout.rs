use std::env;
use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use str0m_benchmarks::{
    EnqueueSharedHarness, EnqueueVecHarness, FullEgressSharedHarness, FullEgressVecHarness,
    FullRelaySharedHarness, FullRelayVecHarness, ReceiveMediaSharedHarness, ReceiveMediaVecHarness,
    ReceiveRtpSharedHarness, ReceiveRtpVecHarness, benchmark_fanouts, forward_shared, forward_vec,
    packet_template_shared, packet_template_vec, shared_payload,
};

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

struct PayloadScenario {
    label: &'static str,
    size: usize,
}

// Payload sizes chosen to stay close to realistic RTP payload sizes:
// - 160B  : common small Opus packet size.
// - 1350B : larger video RTP payload size that is still realistic on the wire.
const PAYLOAD_SCENARIOS: &[PayloadScenario] = &[
    PayloadScenario {
        label: "audio-160B",
        size: 160,
    },
    PayloadScenario {
        label: "video-1350B",
        size: 1350,
    },
];

const ENQUEUE_ROUNDS: usize = 64;
const FULL_EGRESS_ROUNDS: usize = 64;
const DEFAULT_FULL_RELAY_ROUNDS: usize = 64;
const RECEIVE_ROUNDS: usize = 64;

fn full_relay_rounds() -> usize {
    env::var("FULL_RELAY_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|rounds| *rounds > 0)
        .unwrap_or(DEFAULT_FULL_RELAY_ROUNDS)
}

fn bench_packet_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("packet_fanout");
    let fanouts = benchmark_fanouts();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let vec_template = packet_template_vec(payload_size);
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    let mut out = Vec::with_capacity(fanout);
                    b.iter(|| {
                        forward_vec(black_box(&vec_template), black_box(&targets), &mut out);
                        black_box(&out);
                    });
                },
            );
            let shared_template = packet_template_shared(payload_size);
            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
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
    let fanouts = benchmark_fanouts();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let payload_vec = str0m_benchmarks::make_payload(payload_size);
            let throughput = Throughput::Bytes((payload_size * fanout * ENQUEUE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || EnqueueVecHarness::new(fanout),
                        |mut harness| {
                            harness.enqueue_vec(black_box(&payload_vec), ENQUEUE_ROUNDS);
                            black_box(harness);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
            let payload_shared = shared_payload(payload_size);
            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || EnqueueSharedHarness::new(fanout),
                        |mut harness| {
                            harness.enqueue_shared(black_box(&payload_shared), ENQUEUE_ROUNDS);
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
    let fanouts = benchmark_fanouts();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let payload_vec = str0m_benchmarks::make_payload(payload_size);
            let throughput = Throughput::Bytes((payload_size * fanout * FULL_EGRESS_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullEgressVecHarness::new(fanout),
                        |mut harness| {
                            let transmit_count =
                                harness.egress_vec(black_box(&payload_vec), FULL_EGRESS_ROUNDS);
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );
            let payload_shared = shared_payload(payload_size);
            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullEgressSharedHarness::new(fanout),
                        |mut harness| {
                            let transmit_count = harness
                                .egress_shared(black_box(&payload_shared), FULL_EGRESS_ROUNDS);
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
    let fanouts = benchmark_fanouts();
    let full_relay_rounds = full_relay_rounds();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let throughput = Throughput::Bytes((payload_size * fanout * full_relay_rounds) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullRelayVecHarness::new(fanout, payload_size, full_relay_rounds),
                        |mut harness| {
                            let transmit_count = harness.relay_vec();
                            black_box(transmit_count);
                        },
                        criterion::BatchSize::SmallInput,
                    );
                },
            );

            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || FullRelaySharedHarness::new(fanout, payload_size, full_relay_rounds),
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
            BenchmarkId::new("base_vec", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveRtpVecHarness::new(payload_size, RECEIVE_ROUNDS),
                    |mut harness| {
                        let payload_bytes = harness.receive_events();
                        black_box(payload_bytes);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new("arc_meta", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveRtpSharedHarness::new(payload_size, RECEIVE_ROUNDS),
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
    let fanouts = benchmark_fanouts();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout * RECEIVE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveRtpVecHarness::new(payload_size, RECEIVE_ROUNDS),
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

            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveRtpSharedHarness::new(payload_size, RECEIVE_ROUNDS),
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
            BenchmarkId::new("base_vec", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveMediaVecHarness::new(payload_size, RECEIVE_ROUNDS),
                    |mut harness| {
                        let payload_bytes = harness.receive_events();
                        black_box(payload_bytes);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new("arc_meta", scenario.label),
            &payload_size,
            |b, _| {
                b.iter_batched(
                    || ReceiveMediaSharedHarness::new(payload_size, RECEIVE_ROUNDS),
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
    let fanouts = benchmark_fanouts();

    for scenario in PAYLOAD_SCENARIOS {
        let payload_size = scenario.size;
        for &fanout in &fanouts {
            let targets = str0m_benchmarks::benchmark_targets(fanout);
            let throughput = Throughput::Bytes((payload_size * fanout * RECEIVE_ROUNDS) as u64);

            group.throughput(throughput);
            group.bench_with_input(
                BenchmarkId::new("base_vec", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveMediaVecHarness::new(payload_size, RECEIVE_ROUNDS),
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

            group.bench_with_input(
                BenchmarkId::new("arc_meta", format!("{}-{fanout}dst", scenario.label)),
                &(payload_size, fanout),
                |b, _| {
                    b.iter_batched(
                        || ReceiveMediaSharedHarness::new(payload_size, RECEIVE_ROUNDS),
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
